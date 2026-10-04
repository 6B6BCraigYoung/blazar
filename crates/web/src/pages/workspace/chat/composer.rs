use leptos::ev;
use leptos::html;
use leptos::prelude::*;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::storage;

use super::dock::Dock;
use super::state::{ACC_RUNTIMES, Attach, Chat};

mod pickers;
mod presentation;
mod queue;
mod status;

use pickers::{FilesPop, agent_pop, mcp_pop, mode_pop, model_pop, plus_pop, slash_pop, style_pop};
use presentation::{editor_url, eff_label, fmt_dur, fmt_k, icon, svg};
use queue::{FailedEditor, QueueBand, QueueEditor};
use status::{RateBanner, Todos};

#[wasm_bindgen(module = "/js/files.js")]
extern "C" {
    #[wasm_bindgen(js_name = readDataUrl)]
    fn read_data_url(file: &web_sys::File) -> js_sys::Promise;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pop {
    Slash,
    Files,
    Plus,
    Mode,
    Model,
    Agent,
    Mcp,
    Style,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct UiPrefs {
    #[serde(rename = "sendKey")]
    send_key: String,
}

#[derive(Clone)]
struct SlashItem {
    name: String,
    desc: String,
    hint: String,
    blazar: bool,
}

#[component]
pub fn Composer(chat: Chat, tick: RwSignal<u32>, tree_files: Signal<Vec<String>>) -> impl IntoView {
    let pop = RwSignal::new(None::<Pop>);
    let sel = RwSignal::new(0usize);
    let ta = NodeRef::<html::Textarea>::new();
    let file_in = NodeRef::<html::Input>::new();
    let run_since = StoredValue::new(None::<f64>);

    let grow = move || {
        if let Some(t) = ta.try_get_untracked().flatten() {
            let s = web_sys::HtmlElement::style(&t);
            let _ = s.set_property("height", "auto");
            let h = t.scroll_height().min(220);
            let _ = s.set_property("height", &format!("{h}px"));
        }
    };
    Effect::new(move |_| {
        chat.prompt.track();
        request_animation_frame(grow);
    });
    let focus = move || {
        if let Some(t) = ta.try_get_untracked().flatten() {
            let _ = t.focus();
        }
    };

    let closer = window_event_listener(ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| t.closest(".cb-pop, .cb-trigger").ok().flatten().is_some());
        if !inside {
            pop.set(None);
        }
    });
    on_cleanup(move || closer.remove());

    let slash_items = move || -> Vec<SlashItem> {
        let q = chat.prompt.get();
        let Some(k) = q
            .strip_prefix('/')
            .filter(|k| !k.contains(char::is_whitespace))
        else {
            return Vec::new();
        };
        let k = k.to_lowercase();
        let claude = chat.runtime() == "claude";
        let p = chat.prefs.get();
        let mut items = vec![
            ("new", "新建对话".to_owned()),
            ("model", "切换模型或 Effort".to_owned()),
            (
                "modes",
                chat.effective_mode()
                    .map_or_else(|| "—".to_owned(), |m| m.1.to_owned()),
            ),
        ];
        if claude {
            items.push((
                "thinking",
                if p.thinking {
                    "on → off"
                } else {
                    "off → on"
                }
                .to_owned(),
            ));
            items.push((
                "fast",
                if p.fast { "on → off" } else { "off → on" }.to_owned(),
            ));
            items.push((
                "output-style",
                if p.style.is_empty() {
                    "Default".to_owned()
                } else {
                    p.style.clone()
                },
            ));
            items.push(("mcp", "MCP 服务".to_owned()));
        }
        items.push(("rewind", "还原到消息发送前".to_owned()));
        items.push(("usage", "账号与用量".to_owned()));
        if chat.running.get() {
            items.push(("interrupt", "停止当前运行".to_owned()));
        }
        items.push((
            "open",
            format!("在 {} 中打开", editor_url("local", "", None).0),
        ));
        let mut out: Vec<SlashItem> = items
            .into_iter()
            .map(|(n, d)| SlashItem {
                name: n.to_owned(),
                desc: d,
                hint: String::new(),
                blazar: true,
            })
            .collect();
        if claude {
            chat.load_catalog();
            let cli = chat
                .catalog
                .with(|c| c.as_ref().map(|c| c.commands.clone()).unwrap_or_default());
            out.extend(cli.into_iter().map(|c| SlashItem {
                name: c.name,
                desc: c.description,
                hint: c.argument_hint,
                blazar: false,
            }));
        }
        out.into_iter()
            .filter(|c| {
                k.is_empty()
                    || c.name.to_lowercase().contains(&k)
                    || c.desc.to_lowercase().contains(&k)
            })
            .take(60)
            .collect()
    };

    let toggle_thinking = move || {
        let on = !chat.prefs.get_untracked().thinking;
        chat.set_prefs(
            |p| p.thinking = on,
            Some((
                json!({ "thinking": on }),
                if on { "thinking on" } else { "thinking off" },
            )),
        );
        if !chat.running.get_untracked() {
            toast(if on {
                "已开启 Thinking"
            } else {
                "后续消息关闭 Thinking"
            });
        }
    };
    let toggle_fast = move || {
        let on = !chat.prefs.get_untracked().fast;
        chat.set_prefs(
            |p| p.fast = on,
            Some((
                json!({ "fast_mode": on }),
                if on { "fast mode on" } else { "fast mode off" },
            )),
        );
        if !chat.running.get_untracked() {
            toast(if on {
                "已开启 Fast mode"
            } else {
                "已关闭 Fast mode"
            });
        }
    };
    let open_in_editor = move || {
        let root = chat.root.get_value();
        let target = chat.files.current.get_untracked().map_or_else(
            || root.clone(),
            |f| format!("{}/{f}", root.trim_end_matches('/')),
        );
        let (_, url) = editor_url(&chat.node.get_value(), &target, None);
        let _ = window().location().set_href(&url);
    };
    let usage = move || {
        let windows = chat.transcript.with_untracked(|t| t.rate.clone());
        let body = if windows.is_empty() {
            "暂无用量数据，完成首轮 Claude 对话后可查看。".to_owned()
        } else {
            windows
                .iter()
                .map(|w| {
                    format!(
                        "{}：{}%",
                        crate::fmt::window_label(&w.name),
                        (w.utilization * 100.0).round()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        chat.spawn(async move {
            dialog::ask("用量", &body, vec![Choice::plain("关闭")]).await;
        });
    };
    let rewind_list = move || {
        let users: Vec<(String, String)> = chat.transcript.with_untracked(|t| {
            t.items
                .iter()
                .rev()
                .filter_map(|i| match &i.body {
                    crate::chat_model::Body::User { sid, seq, text, .. } => chat
                        .checkpoints
                        .with_untracked(|c| c.get(&format!("{sid}:{seq}")).cloned())
                        .map(|cp| (cp, text.chars().take(60).collect())),
                    _ => None,
                })
                .take(20)
                .collect()
        });
        if users.is_empty() {
            toast("暂无可还原记录；非 Git 工作区不创建检查点。");
            return;
        }
        chat.spawn(async move {
            let mut ch = vec![Choice::plain("取消")];
            ch.extend(users.iter().map(|u| Choice::plain(u.1.clone())));
            if let Some(i) = dialog::ask(
                "还原到消息之前",
                "选择一条消息，将文件还原到发送前的状态。",
                ch,
            )
            .await
                && i > 0
            {
                chat.rewind(users[i - 1].0.clone(), true);
            }
        });
    };

    let run_slash = move |i: usize| {
        let Some(c) = slash_items().get(i).cloned() else {
            return;
        };
        pop.set(None);
        if c.blazar
            || matches!(
                c.name.as_str(),
                "mcp" | "fast" | "output-style" | "config" | "model"
            )
        {
            chat.prompt.set(String::new());
            match c.name.as_str() {
                "new" => chat.fresh.set(true),
                "model" | "config" => pop.set(Some(Pop::Model)),
                "modes" => chat.cycle_mode(),
                "thinking" => toggle_thinking(),
                "fast" => toggle_fast(),
                "output-style" => {
                    chat.load_catalog();
                    pop.set(Some(Pop::Style));
                }
                "mcp" => {
                    chat.load_catalog();
                    pop.set(Some(Pop::Mcp));
                }
                "rewind" => rewind_list(),
                "usage" => usage(),
                "interrupt" => chat.stop(),
                "open" => open_in_editor(),
                _ => {}
            }
            focus();
            return;
        }
        chat.prompt.set(format!("/{} ", c.name));
        focus();
        if c.hint.is_empty() {
            chat.send();
        }
    };

    let add_image = move |f: web_sys::File| {
        let t = f.type_();
        if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&t.as_str()) {
            toast("仅支持 PNG、JPEG、GIF 和 WebP 图片");
            return;
        }
        if f.size() > 5.0 * 1024.0 * 1024.0 {
            toast("图片大小不能超过 5 MB");
            return;
        }
        if chat.attach.with_untracked(Vec::len) >= 8 {
            toast("Up to 8 images per message");
            return;
        }
        chat.spawn(async move {
            if let Ok(v) = wasm_bindgen_futures::JsFuture::from(read_data_url(&f)).await
                && let Some(url) = v.as_string()
            {
                let data = url
                    .split_once(',')
                    .map(|x| x.1.to_owned())
                    .unwrap_or_default();
                chat.attach.update(|a| {
                    a.push(Attach {
                        media_type: t,
                        data,
                        url,
                    })
                });
            }
        });
    };
    let files_of = move |list: Option<web_sys::FileList>| -> Vec<web_sys::File> {
        list.map(|l| {
            (0..l.length())
                .filter_map(|i| l.get(i))
                .filter(|f| f.type_().starts_with("image/"))
                .collect()
        })
        .unwrap_or_default()
    };

    let insert_at_caret = move |ins: String| {
        let Some(t) = ta.try_get_untracked().flatten() else {
            return;
        };
        let v = t.value();
        let at = t
            .selection_start()
            .ok()
            .flatten()
            .map_or(v.len(), |n| n as usize);
        let chars: Vec<u16> = v.encode_utf16().collect();
        let at = at.min(chars.len());
        let pre = String::from_utf16_lossy(&chars[..at]);
        let post = String::from_utf16_lossy(&chars[at..]);
        let new = format!("{pre}{ins}{post}");
        let caret = (pre.encode_utf16().count() + ins.encode_utf16().count()) as u32;
        chat.prompt.set(new);
        pop.set(None);
        request_animation_frame(move || {
            if let Some(t) = ta.try_get_untracked().flatten() {
                let _ = t.focus();
                let _ = t.set_selection_range(caret, caret);
            }
        });
    };
    let insert_ref = move |p: String| {
        let pre_space = chat
            .prompt
            .with_untracked(|v| !v.is_empty() && !v.ends_with(char::is_whitespace));
        insert_at_caret(format!("{}@{p} ", if pre_space { " " } else { "" }));
    };

    let send_key_mod = storage::load::<UiPrefs>("blazar.ui").is_some_and(|p| p.send_key == "mod");
    let on_input = move |e: ev::Event| {
        let v = event_target_value(&e);
        chat.prompt.set(v.clone());
        grow();
        let is_slash = v.starts_with('/') && !v.contains(char::is_whitespace);
        if is_slash {
            sel.set(0);
            pop.set(Some(Pop::Slash));
        } else if pop.get_untracked() == Some(Pop::Slash) {
            pop.set(None);
        }
        if let Some(t) = ta.try_get_untracked().flatten() {
            let at = t.selection_start().ok().flatten().unwrap_or(0) as usize;
            let u: Vec<u16> = v.encode_utf16().collect();
            if at > 0
                && at <= u.len()
                && u[at - 1] == u16::from(b'@')
                && (at == 1
                    || char::from_u32(u32::from(u[at - 2])).is_some_and(char::is_whitespace))
            {
                let mut nu = u.clone();
                nu.remove(at - 1);
                chat.prompt.set(String::from_utf16_lossy(&nu));
                request_animation_frame(move || {
                    if let Some(t) = ta.try_get_untracked().flatten() {
                        let _ = t.set_selection_range(at as u32 - 1, at as u32 - 1);
                    }
                });
                chat.load_snippets();
                pop.set(Some(Pop::Files));
            }
        }
    };
    let on_key = move |e: ev::KeyboardEvent| {
        let composing = e.is_composing() || e.key_code() == 229;
        let k = e.key();
        if pop.get_untracked() == Some(Pop::Slash)
            && chat.prompt.with_untracked(|p| p.starts_with('/'))
        {
            let n = slash_items().len().max(1);
            match k.as_str() {
                "ArrowDown" | "ArrowUp" => {
                    e.prevent_default();
                    sel.update(|s| {
                        *s = if k == "ArrowDown" {
                            (*s + 1) % n
                        } else {
                            (*s + n - 1) % n
                        }
                    });
                    return;
                }
                "Enter" | "Tab" if !composing => {
                    e.prevent_default();
                    run_slash(sel.get_untracked());
                    return;
                }
                _ => {}
            }
        }
        let send_combo = if send_key_mod {
            e.meta_key() || e.ctrl_key()
        } else {
            !e.shift_key()
        };
        if k == "Enter" && !composing && send_combo {
            e.prevent_default();
            chat.send();
        } else if k == "Tab" && e.shift_key() {
            e.prevent_default();
            chat.cycle_mode();
        } else if k == "Escape" && pop.get_untracked().is_some() {
            e.prevent_default();
            pop.set(None);
        } else if k == "Escape" && chat.running.get_untracked() {
            e.prevent_default();
            e.stop_propagation();
            chat.stop();
        }
    };

    let running = chat.running;
    let ring = move || {
        let (used, model) = chat.transcript.with(|t| {
            (
                t.usage
                    .as_ref()
                    .map_or(0, |u| u.input + u.cache_read + u.cache_creation),
                t.model.clone(),
            )
        });
        let win: u64 = if model.to_lowercase().contains("[1m]") {
            1_000_000
        } else {
            200_000
        };
        let pct = (used as f64 / win as f64 * 100.0).min(100.0);
        let title = if running.get() {
            "运行中".to_owned()
        } else if used > 0 {
            format!("上下文 {pct:.0}% · {} / {}", fmt_k(used), fmt_k(win))
        } else {
            "上下文".to_owned()
        };
        (format!("--p:{pct:.1}"), title)
    };
    let timer = move || {
        tick.track();
        if running.get() {
            let now = js_sys::Date::now();
            let since = run_since.get_value().unwrap_or_else(|| {
                run_since.set_value(Some(now));
                now
            });
            fmt_dur(((now - since) / 1000.0) as u64)
        } else {
            run_since.set_value(None);
            String::new()
        }
    };
    let stop_mode = move || running.get() && chat.prompt.with(|p| p.trim().is_empty());

    let model_label = move || {
        let rt = chat.runtime();
        let s = chat.model_sel(&rt);
        let list = chat.models_for(&rt);
        let m = list
            .iter()
            .find(|x| x.id == s.model.clone().unwrap_or_default())
            .or_else(|| list.first());
        let name = m.map_or_else(
            || "Default".to_owned(),
            |m| {
                m.label
                    .replace(" (recommended)", "")
                    .replace(" (Recommended)", "")
            },
        );
        (name, s.effort.map(|e| eff_label(&e)))
    };

    let review_n = move || chat.diff.comments.with(Vec::len);
    let failed_edit = RwSignal::new(None::<(u32, Value)>);

    view! {
        <div class="composer">
            <Show when=move || pop.get().is_some()>
                <div class="cb-pop">
                    {move || match pop.get() {
                        Some(Pop::Slash) => slash_pop(slash_items(), sel, run_slash, chat).into_any(),
                        Some(Pop::Files) => view! { <FilesPop chat tree_files insert_ref=Callback::new(insert_ref) insert_at_caret=Callback::new(insert_at_caret) pop/> }.into_any(),
                        Some(Pop::Plus) => plus_pop(chat, pop, file_in).into_any(),
                        Some(Pop::Mode) => mode_pop(chat, pop).into_any(),
                        Some(Pop::Model) => model_pop(chat, pop).into_any(),
                        Some(Pop::Agent) => agent_pop(chat, pop).into_any(),
                        Some(Pop::Mcp) => mcp_pop(chat, pop).into_any(),
                        Some(Pop::Style) => style_pop(chat, pop).into_any(),
                        None => ().into_any(),
                    }}
                </div>
            </Show>
            <Todos chat/>
            <RateBanner chat/>
            {move || (review_n() > 0).then(|| view! {
                <div class="cb-band review">
                    <span>"附带 "<b>{review_n()}</b>" 条审阅意见"</span><span class="grow"></span>
                    <button class="linkbtn" on:click=move |_| chat.show_diff.run(())>"查看"</button>
                    <button class="linkbtn" on:click=move |_| {
                        let n = review_n();
                        chat.spawn(async move {
                            if dialog::ask("丢弃审阅意见", &format!("丢弃 {n} 条尚未发送的审阅意见？"), vec![Choice::plain("取消"), Choice::danger("丢弃")]).await == Some(1) {
                                chat.clear_review();
                            }
                        });
                    }>"清空"</button>
                </div>
            })}
            <QueueBand chat/>
            {move || {
                let thread = chat.view.get();
                chat.deliveries.with(|d| d.failed()).into_iter().filter(|(_, request)| request["resume_session"].as_str() == thread.as_deref()).map(|(id, request)| {
                let text = request["text"].as_str().unwrap_or_default().to_owned();
                let images = request["images"].as_array().map_or(0, Vec::len);
                view! { <div class="cb-band queue"><div class="cq-h"><b>"发送失败"</b><span class="grow"></span><button class="linkbtn" on:click=move |_| chat.retry_failed(id)>"重试"</button><button class="linkbtn" on:click=move |_| failed_edit.set(Some((id, request.clone())))>"编辑"</button><button class="linkbtn" on:click=move |_| chat.spawn(async move {
                    if dialog::ask("丢弃消息？", "这条消息尚未发送，丢弃后无法恢复。", vec![Choice::plain("取消"), Choice::danger("丢弃")]).await == Some(1) {
                        let _ = chat.deliveries.try_update(|deliveries| deliveries.take_failed(id));
                    }
                })>"丢弃"</button></div><div class="cq-t">{text}{(images > 0).then(|| format!(" · {images} 张附件"))}</div></div> }
            }).collect_view()}}
            {move || chat.editing_queue.get().map(|queued| view! { <QueueEditor chat queued/> })}
            {move || failed_edit.get().map(|(id, request)| view! { <FailedEditor chat id request on_close=Callback::new(move |_| failed_edit.set(None))/> })}
            <Dock chat/>
            <div class="cc-box" on:dragover=move |e: ev::DragEvent| {
                    if e.data_transfer().is_some_and(|d| (0..d.items().length()).any(|i| d.items().get(i).is_some_and(|x| x.type_().starts_with("image/")))) { e.prevent_default(); }
                }
                on:drop=move |e: ev::DragEvent| {
                    let imgs = files_of(e.data_transfer().and_then(|d| d.files()));
                    if !imgs.is_empty() { e.prevent_default(); e.stop_propagation(); imgs.into_iter().for_each(add_image); }
                }>
                {move || {
                    let list = chat.attach.get();
                    (!list.is_empty()).then(|| view! {
                        <div class="cb-atts">
                            {list.into_iter().enumerate().map(|(i, a)| view! {
                                <span class="cb-att"><img src=a.url alt=""/>
                                    <button aria-label="移除" on:click=move |_| chat.attach.update(|l| { l.remove(i); })>"×"</button>
                                </span>
                            }).collect_view()}
                        </div>
                    })
                }}
                <textarea node_ref=ta rows="1" class="prompt" aria-label="消息"
                    placeholder=move || if running.get() { "添加下一条消息…" } else { "发送消息…" }
                    prop:value=move || chat.prompt.get()
                    on:input=on_input on:keydown=on_key
                    on:paste=move |e: ev::ClipboardEvent| {
                        let imgs = files_of(e.clipboard_data().and_then(|d| d.files()));
                        if !imgs.is_empty() { e.prevent_default(); imgs.into_iter().for_each(add_image); }
                    }></textarea>
                <div class="cb-bar">
                    <button class="cb-ic cb-trigger" aria-label="添加附件" title="添加附件" inner_html=icon("plus") on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Plus) { None } else { Some(Pop::Plus) })></button>
                    <button class="cb-ic cb-trigger" aria-label="斜杠命令" title="斜杠命令（/）" inner_html=icon("slash") on:click=move |_| {
                        if !chat.prompt.get_untracked().starts_with('/') { chat.prompt.set("/".into()); }
                        sel.set(0);
                        pop.set(Some(Pop::Slash));
                        focus();
                    }></button>
                    {move || (chat.runtime() == "claude").then(|| {
                        let (n, bad) = chat.transcript.with(|t| t.mcp.clone()).iter().fold((0, 0), |(n, b), m| (n + 1, b + usize::from(m.status == "failed")));
                        view! { <button class="cb-ic cb-trigger" data-bad=(bad > 0).to_string() aria-label="MCP 服务" title=format!("MCP 服务{}", if n > 0 { format!(" ({n})") } else { String::new() })
                            inner_html=icon("plug") on:click=move |_| { chat.load_catalog(); pop.update(|p| *p = if *p == Some(Pop::Mcp) { None } else { Some(Pop::Mcp) }); }></button> }
                    })}
                    {move || { let (st, title) = ring(); view! { <span class="cb-ring" data-run=running.get().to_string() style=st title=title></span> } }}
                    <span class="cb-time">{timer}</span>
                    <button class="cb-chip cb-trigger" title="选择模型" on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Model) { None } else { Some(Pop::Model) })>
                        {move || { let (n, e) = model_label(); view! { {n}{e.map(|e| view! { " "<span class="muted">{e}</span> })} } }}
                    </button>
                    {move || chat.ctx_file().map(|f| {
                        let name = f.rsplit('/').next().unwrap_or(&f).to_owned();
                        let f2 = f.clone();
                        view! {
                            <span class="cb-div"></span>
                            <span class="cb-chip cb-ctx" title=format!("当前文件：{f}")>
                                <span inner_html=icon("file")></span><span class="nm">{name}</span>
                                <button class="x" aria-label="移除当前文件上下文" title="移除当前文件上下文" on:click=move |e| { e.stop_propagation(); chat.ctx_off.set(Some(f2.clone())); }>"×"</button>
                            </span>
                        }
                    })}
                    <Show when=move || chat.fresh.get() && chat.view.get().is_some()>
                        <button type="button" class="cb-chip cb-fresh" aria-label="取消新建对话，继续当前对话" title="下一条消息会新建对话" on:click=move |_| chat.fresh.set(false)>"新建对话 ×"</button>
                    </Show>
                    <span class="cb-grow"></span>
                    <span class="cb-right">
                    <button class="cb-chip cb-plain cb-trigger" title="选择智能体和账号，保留当前对话上下文"
                        on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Agent) { None } else { Some(Pop::Agent) })>
                        <span class="rt-dot" data-rt=move || chat.runtime()></span>
                        <span>{move || chat.agent_label()}</span>
                        {move || {
                            let rt = chat.runtime();
                            (ACC_RUNTIMES.contains(&rt.as_str()) && chat.agent.get().starts_with("r:")).then(|| {
                                chat.current_account(&rt).and_then(|id| chat.account(&id)).map(|a| view! { <span class="muted">{format!(" · {}", a.label)}</span> })
                            }).flatten()
                        }}
                    </button>
                    {move || chat.effective_mode().map(|m| {
                        let from = if !chat.perm.get().is_empty() { "" } else if chat.profile().and_then(|p| p.permission_mode).is_some() { "（来自智能体）" } else { "（默认）" };
                        view! {
                            <button class="cb-chip cb-plain cb-trigger cb-mode" data-mode=m.0 title=format!("{}{from}", m.2)
                                on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Mode) { None } else { Some(Pop::Mode) })>
                                <span inner_html=icon(m.3)></span><span>{m.1}</span>
                            </button>
                        }
                    })}
                    <button class="cb-send" data-mode=move || if stop_mode() { "stop" } else { "send" } data-busy=move || (chat.busy.get() > 0).to_string()
                        data-pmode=move || chat.effective_mode().map(|m| m.0).unwrap_or("")
                        aria-label=move || if stop_mode() { "停止" } else if running.get() { "加入队列" } else { "发送" }
                        title=move || if stop_mode() { "停止（Esc）" } else if running.get() { "加入队列（Enter）" } else { "发送（Enter）" }
                        on:click=move |_| if stop_mode() { chat.stop() } else { chat.send() }>
                        <span class="i-send" inner_html=svg(r#"<path d="M12 19V5M5.5 11.5 12 5l6.5 6.5"/>"#)></span>
                        <span class="i-stop"></span>
                    </button>
                    </span>
                    <input type="file" node_ref=file_in accept="image/png,image/jpeg,image/gif,image/webp" multiple hidden
                        on:change=move |e| {
                            let input: web_sys::HtmlInputElement = event_target(&e);
                            files_of(input.files()).into_iter().for_each(add_image);
                            input.set_value("");
                        }/>
                </div>
            </div>
        </div>
    }
}
