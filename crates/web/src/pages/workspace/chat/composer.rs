use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::api;
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::storage;

use super::dock::Dock;
use super::log::todo_list;
use super::state::{ACC_RUNTIMES, Attach, Chat, ModelSel, Queued, edit_request};

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

fn svg(path: &str) -> String {
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{path}</svg>"#
    )
}

fn icon(k: &str) -> String {
    svg(match k {
        "hand" => {
            r#"<path d="M8 12V6.5a1.5 1.5 0 0 1 3 0V11M11 10V5a1.5 1.5 0 0 1 3 0v6M14 10V6.5a1.5 1.5 0 0 1 3 0V14a6 6 0 0 1-6 6h-.5a6 6 0 0 1-4.9-2.5L3.8 14a1.5 1.5 0 0 1 2.4-1.8L8 14"/>"#
        }
        "code" => r#"<path d="M9 8l-4 4 4 4M15 8l4 4-4 4"/>"#,
        "plan" => r#"<path d="M4 5h16v14H4z"/><path d="M8 15l3-3 2 2 3-4"/>"#,
        "bolt" => r#"<path d="M13 3L5 14h6l-1 7 8-11h-6z"/>"#,
        "warn" => r#"<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17v.5"/>"#,
        "up" => r#"<path d="M12 16V4M7 9l5-5 5 5"/><path d="M4 16v4h16v-4"/>"#,
        "file" => r#"<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4M9 13h6M9 17h4"/>"#,
        "plus" => r#"<path d="M12 5v14M5 12h14"/>"#,
        "slash" => r#"<rect x="3.5" y="3.5" width="17" height="17" rx="3"/><path d="M14 8l-4 8"/>"#,
        "send" => r#"<path d="M12 19V5M5.5 11.5 12 5l6.5 6.5"/>"#,
        "plug" => r#"<path d="M9 7V3M15 7V3M7 7h10v4a5 5 0 0 1-10 0z"/><path d="M12 16v5"/>"#,
        _ => "",
    })
}

const EFF_LABEL: [(&str, &str); 7] = [
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra high"),
    ("max", "Max"),
    ("ultra", "Ultra"),
];

fn eff_label(e: &str) -> String {
    EFF_LABEL
        .iter()
        .find(|x| x.0 == e)
        .map_or_else(|| e.to_owned(), |x| x.1.to_owned())
}

fn fmt_k(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn fmt_dur(sec: u64) -> String {
    if sec < 60 {
        format!("{sec}s")
    } else if sec < 3600 {
        format!("{}m {}s", sec / 60, sec % 60)
    } else {
        format!("{}h {}m", sec / 3600, sec % 3600 / 60)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct UiPrefs {
    #[serde(rename = "sendKey")]
    send_key: String,
}

fn editor_url(node: &str, path: &str, line: Option<u32>) -> (String, String) {
    let k = storage::load_raw("blazar.editor").unwrap_or_else(|| "vscode".into());
    let (label, scheme) = match k.as_str() {
        "cursor" => ("Cursor", "cursor"),
        "windsurf" => ("Windsurf", "windsurf"),
        "insiders" => ("VS Code Insiders", "vscode-insiders"),
        _ => ("VS Code", "vscode"),
    };
    let p: String = path
        .split('/')
        .map(|s| String::from(js_sys::encode_uri_component(s)))
        .collect::<Vec<_>>()
        .join("/");
    let tail = line.map(|l| format!(":{l}")).unwrap_or_default();
    let url = if node == "local" {
        format!("{scheme}://file{p}{tail}")
    } else {
        format!(
            "{scheme}://vscode-remote/ssh-remote+{}{p}{tail}",
            js_sys::encode_uri_component(node)
        )
    };
    (label.to_owned(), url)
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
            ("new", "Clear conversation".to_owned()),
            ("model", "Switch model / effort".to_owned()),
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
            items.push(("mcp", "MCP servers".to_owned()));
        }
        items.push(("rewind", "Restore files to before a message".to_owned()));
        items.push(("usage", "Account & usage".to_owned()));
        if chat.running.get() {
            items.push(("interrupt", "Stop the current run".to_owned()));
        }
        items.push((
            "open",
            format!("Open in {}", editor_url("local", "", None).0),
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
                "Thinking on"
            } else {
                "Thinking off for the next messages"
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
            toast(if on { "Fast mode on" } else { "Fast mode off" });
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
            "No usage data yet. It appears after the first Claude turn.".to_owned()
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
            dialog::ask("Usage", &body, vec![Choice::plain("Close")]).await;
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
            toast("Nothing to rewind yet (workspaces outside git have no checkpoints)");
            return;
        }
        chat.spawn(async move {
            let mut ch = vec![Choice::plain("Cancel")];
            ch.extend(users.iter().map(|u| Choice::plain(u.1.clone())));
            if let Some(i) = dialog::ask(
                "Rewind to before…",
                "Pick a message. Files go back to how they were before it was sent.",
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
            toast("Only PNG, JPEG, GIF and WebP images are supported");
            return;
        }
        if f.size() > 5.0 * 1024.0 * 1024.0 {
            toast("Images must be under 5 MB");
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
            "Running".to_owned()
        } else if used > 0 {
            format!("Context {pct:.0}% · {} / {}", fmt_k(used), fmt_k(win))
        } else {
            "Context".to_owned()
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
                    <span>"Attaching "<b>{review_n()}</b>{move || if review_n() == 1 { " review comment" } else { " review comments" }}</span><span class="grow"></span>
                    <button class="linkbtn" on:click=move |_| chat.show_diff.run(())>"View"</button>
                    <button class="linkbtn" on:click=move |_| {
                        let n = review_n();
                        chat.spawn(async move {
                            if dialog::ask("Discard review comments", &format!("Discard {n} unsent review comment{}?", if n == 1 { "" } else { "s" }), vec![Choice::plain("Cancel"), Choice::danger("Discard")]).await == Some(1) {
                                chat.clear_review();
                            }
                        });
                    }>"Clear"</button>
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
                                    <button aria-label="Remove" on:click=move |_| chat.attach.update(|l| { l.remove(i); })>"×"</button>
                                </span>
                            }).collect_view()}
                        </div>
                    })
                }}
                <textarea node_ref=ta rows="1" class="prompt"
                    placeholder=move || if running.get() { "Queue a message…" } else { "Message the agent…" }
                    prop:value=move || chat.prompt.get()
                    on:input=on_input on:keydown=on_key
                    on:paste=move |e: ev::ClipboardEvent| {
                        let imgs = files_of(e.clipboard_data().and_then(|d| d.files()));
                        if !imgs.is_empty() { e.prevent_default(); imgs.into_iter().for_each(add_image); }
                    }></textarea>
                <div class="cb-bar">
                    <button class="cb-ic cb-trigger" title="Attach" inner_html=icon("plus") on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Plus) { None } else { Some(Pop::Plus) })></button>
                    <button class="cb-ic cb-trigger" title="Commands (/)" inner_html=icon("slash") on:click=move |_| {
                        if !chat.prompt.get_untracked().starts_with('/') { chat.prompt.set("/".into()); }
                        sel.set(0);
                        pop.set(Some(Pop::Slash));
                        focus();
                    }></button>
                    {move || (chat.runtime() == "claude").then(|| {
                        let (n, bad) = chat.transcript.with(|t| t.mcp.clone()).iter().fold((0, 0), |(n, b), m| (n + 1, b + usize::from(m.status == "failed")));
                        view! { <button class="cb-ic cb-trigger" data-bad=(bad > 0).to_string() title=format!("MCP servers{}", if n > 0 { format!(" ({n})") } else { String::new() })
                            inner_html=icon("plug") on:click=move |_| { chat.load_catalog(); pop.update(|p| *p = if *p == Some(Pop::Mcp) { None } else { Some(Pop::Mcp) }); }></button> }
                    })}
                    {move || { let (st, title) = ring(); view! { <span class="cb-ring" data-run=running.get().to_string() style=st title=title></span> } }}
                    <span class="cb-time">{timer}</span>
                    <button class="cb-chip cb-trigger" title="Model" on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Model) { None } else { Some(Pop::Model) })>
                        {move || { let (n, e) = model_label(); view! { {n}{e.map(|e| view! { " "<span class="muted">{e}</span> })} } }}
                    </button>
                    {move || chat.ctx_file().map(|f| {
                        let name = f.rsplit('/').next().unwrap_or(&f).to_owned();
                        let f2 = f.clone();
                        view! {
                            <span class="cb-div"></span>
                            <span class="cb-chip cb-ctx" title=format!("Current file: {f}")>
                                <span inner_html=icon("file")></span><span class="nm">{name}</span>
                                <button class="x" title="Don't include the current file" on:click=move |e| { e.stop_propagation(); chat.ctx_off.set(Some(f2.clone())); }>"×"</button>
                            </span>
                        }
                    })}
                    <Show when=move || chat.fresh.get() && chat.view.get().is_some()>
                        <span class="cb-chip cb-fresh" title="Next message starts a new conversation" on:click=move |_| chat.fresh.set(false)>"New conversation ×"</span>
                    </Show>
                    <span class="cb-grow"></span>
                    <span class="cb-right">
                    <button class="cb-chip cb-plain cb-trigger" title="Agent and account (switching keeps the conversation context)"
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
                        let from = if !chat.perm.get().is_empty() { "" } else if chat.profile().and_then(|p| p.permission_mode).is_some() { " (from Agent)" } else { " (default)" };
                        view! {
                            <button class="cb-chip cb-plain cb-trigger cb-mode" data-mode=m.0 title=format!("{}{from}", m.2)
                                on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Mode) { None } else { Some(Pop::Mode) })>
                                <span inner_html=icon(m.3)></span><span>{m.1}</span>
                            </button>
                        }
                    })}
                    <button class="cb-send" data-mode=move || if stop_mode() { "stop" } else { "send" } data-busy=move || (chat.busy.get() > 0).to_string()
                        data-pmode=move || chat.effective_mode().map(|m| m.0).unwrap_or("")
                        title=move || if stop_mode() { "Stop (esc)" } else if running.get() { "Queue (enter)" } else { "Send (enter)" }
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

fn row_btn(
    title: String,
    sub: Option<String>,
    right: Option<String>,
    on: bool,
    click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button class="cp-row" data-sel=on.to_string() on:click=move |_| click()>
            <span class="cp-t"><b>{title}</b>{sub.map(|s| view! { <span>{s}</span> })}</span>
            {right.map(|r| view! { <span class="cp-r">{r}</span> })}
            {on.then(|| view! { <span class="cp-ok">"✓"</span> })}
        </button>
    }
}

fn slash_pop(
    items: Vec<SlashItem>,
    sel: RwSignal<usize>,
    run: impl Fn(usize) + Copy + 'static,
    chat: Chat,
) -> impl IntoView {
    let cli_n = chat
        .catalog
        .with_untracked(|c| c.as_ref().map_or(0, |c| c.commands.len()));
    view! {
        <div class="cp-h">{if cli_n > 0 { format!("Commands · {cli_n} from Claude Code") } else { "Commands".to_owned() }}</div>
        {if items.is_empty() {
            view! { <div class="cp-none">"No matching commands"</div> }.into_any()
        } else {
            items.into_iter().enumerate().map(|(i, c)| view! {
                <button class="cp-row" data-sel=move || (sel.get() == i).to_string() on:click=move |_| run(i)>
                    <span class="cp-t"><b>{format!("/{}", c.name)}{(!c.hint.is_empty()).then(|| view! { " "<span class="hint">{c.hint.clone()}</span> })}</b>
                        {(!c.desc.is_empty()).then(|| view! { <span>{c.desc.clone()}</span> })}</span>
                </button>
            }).collect_view().into_any()
        }}
    }
}

#[component]
fn FilesPop(
    chat: Chat,
    tree_files: Signal<Vec<String>>,
    insert_ref: Callback<String>,
    insert_at_caret: Callback<String>,
    pop: RwSignal<Option<Pop>>,
) -> impl IntoView {
    let q = RwSignal::new(String::new());
    let input = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if let Some(i) = input.get() {
            let _ = i.focus();
        }
    });
    let snippets = move || {
        let k = q.get().to_lowercase();
        chat.snippets
            .get()
            .into_iter()
            .filter(|s| {
                k.is_empty()
                    || s.name.to_lowercase().contains(&k)
                    || s.body.to_lowercase().contains(&k)
            })
            .take(8)
            .collect::<Vec<_>>()
    };
    let files = move || {
        let k = q.get().to_lowercase();
        tree_files
            .get()
            .into_iter()
            .filter(|f| k.is_empty() || f.to_lowercase().contains(&k))
            .take(40)
            .collect::<Vec<_>>()
    };
    let insert_snip = move |body: String| {
        let pre_nl = chat
            .prompt
            .with_untracked(|v| !v.is_empty() && !v.ends_with(char::is_whitespace));
        insert_at_caret.run(format!("{}{body}\n", if pre_nl { "\n" } else { "" }));
    };
    view! {
        <div class="cp-h">"Add context"</div>
        <input class="cp-q mono" node_ref=input placeholder="Search files and snippets…" prop:value=move || q.get()
            on:input=move |e| q.set(event_target_value(&e))
            on:keydown=move |e| {
                if e.key() == "Enter" && !e.is_composing() {
                    e.prevent_default();
                    if let Some(s) = snippets().into_iter().next() { insert_snip(s.body); } else if let Some(f) = files().into_iter().next() { insert_ref.run(f); }
                }
                if e.key() == "Escape" { pop.set(None); }
            }/>
        <div class="cp-list">
            {move || snippets().into_iter().map(|s| {
                let body = s.body.clone();
                let preview: String = s.body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(70).collect();
                view! { <button class="cp-row" on:click=move |_| insert_snip(body.clone())><span class="cp-t"><b>{format!("@{}", s.name)}</b><span>{preview}</span></span><span class="cp-r">"snippet"</span></button> }
            }).collect_view()}
            {move || files().into_iter().map(|f| {
                let f2 = f.clone();
                view! { <button class="cp-row cp-file" on:click=move |_| insert_ref.run(f2.clone())>{f}</button> }
            }).collect_view()}
            {move || (snippets().is_empty() && files().is_empty()).then(|| view! { <div class="cp-none">"No matching files or snippets"</div> })}
        </div>
    }
}

fn plus_pop(
    chat: Chat,
    pop: RwSignal<Option<Pop>>,
    file_in: NodeRef<html::Input>,
) -> impl IntoView {
    let cur = chat.files.current.get_untracked();
    view! {
        <button class="cp-row" on:click=move |_| { pop.set(None); if let Some(i) = file_in.get_untracked() { i.click(); } }>
            <span class="cp-ico" inner_html=icon("up")></span><span class="cp-t"><b>"Upload from computer"</b></span><span class="cp-r">"or paste / drop"</span>
        </button>
        <button class="cp-row" on:click=move |_| { chat.load_snippets(); pop.set(Some(Pop::Files)); }>
            <span class="cp-ico" inner_html=icon("file")></span><span class="cp-t"><b>"Add context"</b></span><span class="cp-r">"@"</span>
        </button>
        {cur.map(|f| {
            let on = chat.ctx_off.get_untracked().as_deref() != Some(f.as_str());
            let f2 = f.clone();
            view! {
                <button class="cp-row" on:click=move |_| { chat.ctx_off.set(if on { Some(f2.clone()) } else { None }); pop.set(None); }>
                    <span class="cp-ico" inner_html=icon("file")></span>
                    <span class="cp-t"><b>{if on { "Remove current file" } else { "Add current file" }}</b><span>{f}</span></span>
                </button>
            }
        })}
    }
}

fn effort_row(chat: Chat) -> impl IntoView {
    move || {
        let rt = chat.runtime();
        let s = chat.model_sel(&rt);
        let list = chat.models_for(&rt);
        let m = list
            .iter()
            .find(|x| x.id == s.model.clone().unwrap_or_default())
            .or_else(|| list.first())
            .cloned();
        let effs = m.map(|m| m.efforts).unwrap_or_default();
        (!effs.is_empty()).then(|| {
            let cur_i = s.effort.as_ref().and_then(|e| effs.iter().position(|x| x == e));
            let label = s.effort.as_deref().map_or_else(|| "Default".to_owned(), eff_label);
            view! {
                <div class="cp-eff">
                    <span>"Effort "<span class="muted">{format!("({label})")}</span></span>
                    <span class="eff-track">
                        {effs.iter().enumerate().map(|(i, e)| {
                            let e = e.clone();
                            let rt = rt.clone();
                            let title = eff_label(&e);
                            view! {
                                <button class="eff-dot" title=title data-on=cur_i.is_some_and(|c| c >= i).to_string() data-cur=(cur_i == Some(i)).to_string()
                                    on:click=move |ev| {
                                        ev.stop_propagation();
                                        let mut s = chat.model_sel(&rt);
                                        s.effort = if s.effort.as_deref() == Some(e.as_str()) { None } else { Some(e.clone()) };
                                        chat.set_model_sel(&rt, &s);
                                    }></button>
                            }
                        }).collect_view()}
                    </span>
                </div>
            }
        })
    }
}

fn mode_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    let cur = chat.effective_mode().map(|m| m.0);
    view! {
        <div class="cp-sec flex">"Modes"<span class="cp-hk"><kbd>"⇧"</kbd>" + "<kbd>"tab"</kbd>" to switch"</span></div>
        {chat.modes().iter().map(|&(v, t, d, ic)| view! {
            <button class="cp-row" data-sel=(Some(v) == cur).to_string() on:click=move |_| { chat.set_mode(v); pop.set(None); }>
                <span class="cp-ico" inner_html=icon(ic)></span>
                <span class="cp-t"><b>{t}</b><span>{d}</span></span>
                {(Some(v) == cur).then(|| view! { <span class="cp-ok">"✓"</span> })}
            </button>
        }).collect_view()}
        {effort_row(chat)}
    }
}

fn model_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    let rt = chat.runtime();
    view! {
        <div class="cp-h">"Select a model"</div>
        {move || {
            let rt = rt.clone();
            let s = chat.model_sel(&rt);
            let cur = s.model.clone().unwrap_or_default();
            chat.models_for(&rt).into_iter().map(|m| {
                let rt = rt.clone();
                let id = m.id.clone();
                let on = m.id == cur;
                row_btn(m.label.clone(), (!m.desc.is_empty()).then(|| m.desc.clone()), None, on, move || {
                    let mut next = chat.model_sel(&rt);
                    next.model = Some(id.clone()).filter(|x| !x.is_empty());
                    let effs = chat.models_for(&rt).into_iter().find(|x| x.id == id).map(|x| x.efforts).unwrap_or_default();
                    if next.effort.as_ref().is_some_and(|e| !effs.contains(e)) {
                        next.effort = None;
                    }
                    chat.set_model_sel(&rt, &ModelSel { model: next.model, effort: next.effort });
                    pop.set(None);
                })
            }).collect_view()
        }}
        {effort_row(chat)}
    }
}

fn agent_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    let cur = chat.agent.get_untracked();
    let profiles = chat.profiles.get_untracked();
    let rts = untrack(move || chat.runtimes());
    let remote = chat.remote();
    let node = chat.node.get_value();
    let pick = move |v: String, acc: Option<String>| {
        pop.set(None);
        chat.set_agent(v);
        if let Some(a) = acc {
            chat.set_acc_sel(&a, chat.view.get_untracked().as_deref());
            if chat.running.get_untracked() {
                toast("The account changes from the next message, after this turn finishes");
            }
        }
    };
    let acc_rows = move |rt: String, current: bool| -> AnyView {
        if remote && rt == "codex" {
            let st = match chat.remote_codex.get_untracked() {
                Some(true) => "logged in",
                Some(false) => "not logged in · log in",
                None => "log in",
            };
            let n2 = node.clone();
            return view! {
                <button class="cp-row cp-sub" on:click=move |_| {
                    pop.set(None);
                    crate::pages::runtimes::node_login(n2.clone(), Callback::new(move |()| chat.load_catalogs()));
                }><span class="cp-t"><b>{format!("Codex login on {node}")}</b></span><span class="cp-r">{st}</span></button>
            }.into_any();
        }
        let list: Vec<crate::api::Account> = chat.accounts.with_untracked(|a| {
            a.as_ref()
                .map(|a| {
                    a.accounts
                        .iter()
                        .filter(|x| x.provider == rt)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        });
        let on_id = if current {
            untrack(move || chat.current_account(&rt))
        } else {
            None
        };
        list.into_iter().map(|a| {
            let away = remote && a.kind != "token";
            let off = !a.usable() || away;
            let note = if away {
                "needs a setup token for remote".to_owned()
            } else if off {
                if a.disabled { "disabled".to_owned() } else if a.kind == "token" { "invalid token".to_owned() } else { "not logged in".to_owned() }
            } else {
                a.windows.iter().filter(|w| w.name != "blocked").map(|w| format!("{} {}%", match w.name.as_str() { "five_hour" => "5h", "seven_day" => "7d", n => n }, (w.utilization * 100.0).round())).collect::<Vec<_>>().join(" · ")
            };
            let on = on_id.as_deref() == Some(a.id.as_str());
            let v = format!("r:{}", a.provider);
            let id = a.id.clone();
            view! {
                <button class="cp-row cp-sub" disabled=off on:click=move |_| pick(v.clone(), Some(id.clone()))>
                    <span class="cp-t"><b>{a.label.clone()}</b></span>
                    {(!note.is_empty()).then(|| view! { <span class="cp-r">{note}</span> })}
                    {on.then(|| view! { <span class="cp-ok">"✓"</span> })}
                </button>
            }
        }).collect_view().into_any()
    };
    let none = profiles.is_empty() && rts.is_empty();
    view! {
        {(!profiles.is_empty()).then(|| view! { <div class="cp-sec">"My Agents"</div> })}
        {profiles.into_iter().map(|p| {
            let v = format!("p:{}", p.id);
            let on = v == cur;
            let v2 = v.clone();
            view! {
                <button class="cp-row" on:click=move |_| pick(v2.clone(), None)>
                    <span class="rt-dot" data-rt=p.runtime.clone()></span>
                    <span class="cp-t"><b>{format!("{} · {}", p.name, p.runtime_label)}</b></span>
                    {on.then(|| view! { <span class="cp-ok">"✓"</span> })}
                </button>
            }
        }).collect_view()}
        {(!rts.is_empty()).then(|| view! { <div class="cp-sec">"Runtimes"</div> })}
        {rts.into_iter().map(|r| {
            let v = format!("r:{}", r.id);
            let on = v == cur;
            let has_acc = ACC_RUNTIMES.contains(&r.id.as_str());
            let v2 = v.clone();
            view! {
                <button class="cp-row" on:click=move |_| pick(v2.clone(), None)>
                    <span class="rt-dot" data-rt=r.id.clone()></span>
                    <span class="cp-t"><b>{r.label.clone()}</b></span>
                    {(on && !has_acc).then(|| view! { <span class="cp-ok">"✓"</span> })}
                </button>
                {has_acc.then(|| acc_rows(r.id.clone(), on))}
            }
        }).collect_view()}
        {none.then(|| view! {
            <div class="cp-none">{if remote { "No runtime available. Log in to Claude Code or Codex on the Runtimes page (remote workspaces need one of these)." } else { "No runtime available. Log in to Claude Code or Codex on the Runtimes page." }}</div>
        })}
    }
}

fn mcp_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    move || {
        let live = chat.running.get();
        let mut list = chat.transcript.with(|t| t.mcp.clone());
        if list.is_empty() {
            list = chat.catalog.with(|c| {
                c.as_ref()
                    .map(|c| c.mcp_servers.clone())
                    .unwrap_or_default()
            });
        }
        let order = |s: &str| match s {
            "failed" => 0,
            "needs-auth" => 1,
            "pending" => 2,
            "connected" => 3,
            "disabled" => 4,
            _ => 5,
        };
        list.sort_by(|a, b| {
            order(&a.status)
                .cmp(&order(&b.status))
                .then_with(|| a.name.cmp(&b.name))
        });
        let ok = list.iter().filter(|m| m.status == "connected").count();
        let n = list.len();
        view! {
            <div class="cp-h">{format!("MCP servers · {ok}/{n} connected")}</div>
            {if list.is_empty() {
                view! { <div class="cp-none">"No MCP servers"</div> }.into_any()
            } else {
                list.into_iter().map(|m| {
                    let st = match m.status.as_str() { "connected" => "connected", "pending" => "connecting", "failed" => "failed", "needs-auth" => "needs auth", "disabled" => "disabled", s => s }.to_owned();
                    let name = m.name.clone();
                    let name2 = m.name.clone();
                    let disabled = m.status == "disabled";
                    view! {
                        <div class="mcp-row">
                            <span class="mcp-dot" data-s=m.status.clone()></span>
                            <span class="nm" title=m.name.clone()>{m.name.clone()}</span>
                            <span class="mcp-st">{st}</span>
                            {(live && m.status == "failed").then(|| view! {
                                <button class="mcp-act" on:click=move |_| { chat.live(json!({ "mcp_reconnect": name }), "MCP reconnect"); pop.set(None); }>"Reconnect"</button>
                            })}
                            {(live && (m.status == "connected" || disabled)).then(|| view! {
                                <button class="mcp-act" on:click=move |_| { chat.live(json!({ "mcp_toggle": { "name": name2, "enabled": disabled } }), if disabled { "MCP enable" } else { "MCP disable" }); pop.set(None); }>
                                    {if disabled { "Enable" } else { "Disable" }}
                                </button>
                            })}
                        </div>
                    }
                }).collect_view().into_any()
            }}
            <div class="cp-none">"Servers that need a login: run claude in a terminal and use /mcp. Reconnect and disable only apply to a running session."</div>
        }
    }
}

fn style_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    move || {
        let mut styles = chat.catalog.with(|c| {
            c.as_ref()
                .map(|c| c.output_styles.clone())
                .unwrap_or_default()
        });
        if styles.is_empty() {
            styles = vec!["default".into(), "Explanatory".into(), "Learning".into()];
        }
        let cur = {
            let p = chat.prefs.get();
            if p.style.is_empty() {
                "default".to_owned()
            } else {
                p.style
            }
        };
        view! {
            <div class="cp-sec">"Output style"</div>
            {styles.into_iter().map(|x| {
                let on = x == cur;
                let label = if x == "default" { "Default".to_owned() } else { x.clone() };
                row_btn(label, None, None, on, move || {
                    let v = x.clone();
                    chat.set_prefs(|p| p.style = if v == "default" { String::new() } else { v.clone() }, Some((json!({ "output_style": v }), "output style")));
                    pop.set(None);
                })
            }).collect_view()}
        }
    }
}

#[component]
fn Todos(chat: Chat) -> impl IntoView {
    let open = RwSignal::new(storage::load_raw("blazar.todosOpen").as_deref() != Some("0"));
    move || {
        let todos = chat
            .transcript
            .with(|t| t.todos.clone())
            .unwrap_or_default();
        let done = todos.iter().filter(|t| t.status == "completed").count();
        if todos.is_empty() || (done == todos.len() && !chat.running.get()) {
            return None;
        }
        let cur = todos
            .iter()
            .find(|t| t.status == "in_progress")
            .map(|t| t.content.clone());
        Some(view! {
            <div class="cc-todobar">
                <button class="td-h" on:click=move |_| { open.update(|o| *o = !*o); storage::save_raw("blazar.todosOpen", if open.get_untracked() { "1" } else { "0" }); }>
                    {move || if open.get() { "▾ " } else { "▸ " }}<b>{format!("Todos {done}/{}", todos.len())}</b>
                    {cur.map(|c| view! { <span class="td-cur">{c}</span> })}
                </button>
                {move || open.get().then(|| todo_list(&todos))}
            </div>
        })
    }
}

#[derive(Serialize, Deserialize, Default)]
struct SavedRate {
    #[serde(default)]
    windows: Vec<blazar_core_types::RateLimitWindow>,
}

#[component]
fn RateBanner(chat: Chat) -> impl IntoView {
    let off = RwSignal::new(storage::load_raw("blazar.rate.off").unwrap_or_default());
    Effect::new(move |_| {
        let w = chat.transcript.with(|t| t.rate.clone());
        if !w.is_empty() {
            storage::save(
                "blazar.rate",
                &serde_json::json!({ "at": js_sys::Date::now(), "windows": w }),
            );
        }
    });
    move || {
        let mut ws = chat.transcript.with(|t| t.rate.clone());
        if ws.is_empty() {
            ws = storage::load::<SavedRate>("blazar.rate")
                .unwrap_or_default()
                .windows;
        }
        let now = js_sys::Date::now();
        let hot = ws
            .into_iter()
            .filter(|w| {
                w.utilization >= 0.8
                    && w.resets_at
                        .is_none_or(|r| (r.timestamp_millis() as f64) > now)
            })
            .max_by(|a, b| a.utilization.total_cmp(&b.utilization))?;
        let key = format!(
            "{}:{}",
            hot.name,
            hot.resets_at.map(|r| r.to_rfc3339()).unwrap_or_default()
        );
        if off.get() == key {
            return None;
        }
        let short = match hot.name.as_str() {
            "five_hour" => "session",
            "seven_day" => "weekly",
            "seven_day_opus" => "weekly Opus",
            "seven_day_sonnet" => "weekly Sonnet",
            n => n,
        }
        .to_owned();
        let reset = hot
            .resets_at
            .map(|r| crate::fmt::resets(Some(&r.to_rfc3339())))
            .filter(|r| !r.is_empty())
            .map(|r| format!(" · {r}"))
            .unwrap_or_default();
        Some(view! {
            <div class="cb-band rate">
                <span>{format!("You've used {}% of your {short} limit{reset}", (hot.utilization * 100.0).round())}</span>
                <span class="grow"></span>
                <button class="x" aria-label="Dismiss" on:click=move |_| { storage::save_raw("blazar.rate.off", &key); off.set(key.clone()); }>"×"</button>
            </div>
        })
    }
}

#[component]
fn QueueBand(chat: Chat) -> impl IntoView {
    move || {
        let view_id = chat.view.get();
        let q = chat.queue.get();
        let mine = q.iter().find(|x| x.thread_id == view_id).cloned();
        let others = q.len() - usize::from(mine.is_some());
        if mine.is_none() && others == 0 {
            return None;
        }
        let running = chat.running.get();
        Some(match mine {
            Some(m) => {
                let (m1, m2, m3) = (m.clone(), m.clone(), m.clone());
                view! {
                    <div class="cb-band queue">
                        <div class="cq-h">
                            <span class="cq-dot" data-held=m.held.is_some().to_string()></span>
                            <b>{if m.held.is_some() { "Held" } else { "Queued" }}</b>
                            <span class="muted">{m.held.clone().unwrap_or_else(|| "Sends when this turn ends".into())}</span>
                            <span class="grow"></span>
                            {if running {
                                view! { <button class="linkbtn" title="Send it into the current turn now" on:click=move |_| chat.queue_act("steer", m1.clone())>"Steer"</button> }.into_any()
                            } else {
                                view! { <button class="linkbtn" on:click=move |_| chat.queue_act("send", m1.clone())>"Send"</button> }.into_any()
                            }}
                            <button class="linkbtn" on:click=move |_| chat.queue_act("edit", m2.clone())>"Edit"</button>
                            <button class="linkbtn" on:click=move |_| chat.queue_act("drop", m3.clone())>"Remove"</button>
                        </div>
                        <div class="cq-t">{m.text.clone()}{(m.images > 0).then(|| view! { <span class="muted">{format!(" [{} image{}]", m.images, if m.images == 1 { "" } else { "s" })}</span> })}</div>
                        {(others > 0).then(|| view! { <div class="cq-o muted">{format!("{others} more queued in other conversations")}</div> })}
                    </div>
                }.into_any()
            }
            None => view! { <div class="cb-band queue"><div class="cq-o muted">{format!("{others} message{} queued in other conversations", if others == 1 { "" } else { "s" })}</div></div> }.into_any(),
        })
    }
}

#[component]
fn QueueEditor(chat: Chat, queued: Queued) -> impl IntoView {
    let text = RwSignal::new(queued.text.clone());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let queued = StoredValue::new(queued);
    let ws = chat.ws_id();
    let close = move || {
        if !busy.get_untracked() {
            chat.editing_queue.set(None);
        }
    };
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        let queued = queued.get_value();
        let Some(original) = queued.request else {
            return;
        };
        let request = edit_request(&original, text.get_untracked());
        if request["text"].as_str().is_none_or(|t| t.trim().is_empty())
            && request["images"].as_array().is_none_or(Vec::is_empty)
        {
            error.set("请输入消息".into());
            return;
        }
        let ws = ws.clone();
        busy.set(true);
        error.set(String::new());
        chat.spawn(async move {
            let result = api::send::<Vec<Queued>>(
                "PUT",
                &format!("/api/workspaces/{ws}/queue"),
                &json!({ "thread_id": queued.thread_id, "request": request, "expected": original }),
            )
            .await;
            let _ = busy.try_set(false);
            match result {
                Ok(queue) => {
                    let _ = chat.queue.try_set(queue);
                    let _ = chat.editing_queue.try_set(None);
                }
                Err(e) => {
                    let _ = error.try_set(e.to_string());
                }
            }
        });
    };
    view! {
        <div class="dlg-mask" on:click=move |_| close() on:keydown=move |e| if e.key() == "Escape" { e.stop_propagation(); close(); }>
            <div class="dlg" role="dialog" aria-modal="true" aria-label="编辑排队消息" on:click=|e| e.stop_propagation()>
                <h3>"编辑排队消息"</h3>
                <textarea autofocus prop:value=move || text.get() disabled=move || busy.get() on:input=move |e| text.set(event_target_value(&e))></textarea>
                <p class="muted">"附件、文件上下文和原发送设置会保留。"</p>
                <Show when=move || !error.get().is_empty()><div class="err" role="alert">{move || error.get()}</div></Show>
                <div class="dlg-foot"><button class="btn" disabled=move || busy.get() on:click=move |_| close()>"取消"</button><button class="btn primary" disabled=move || busy.get() on:click=save>{move || if busy.get() { "保存中…" } else { "保存" }}</button></div>
            </div>
        </div>
    }
}

#[component]
fn FailedEditor(chat: Chat, id: u32, request: Value, on_close: Callback<()>) -> impl IntoView {
    let text = RwSignal::new(request["text"].as_str().unwrap_or_default().to_owned());
    let images = RwSignal::new(request["images"].as_array().cloned().unwrap_or_default());
    let original = StoredValue::new(request);
    let error = RwSignal::new(String::new());
    let save = move |_| {
        let text = text.get_untracked();
        let images = images.get_untracked();
        if text.trim().is_empty() && images.is_empty() {
            error.set("请输入消息或保留附件".into());
            return;
        }
        let mut request = edit_request(&original.get_value(), text);
        request["images"] = json!(images);
        if chat.deliveries.write().edit_failed(id, request) {
            on_close.run(());
        } else {
            error.set("消息已重试或丢弃，无法保存这次编辑".into());
        }
    };
    view! {
        <div class="dlg-mask" on:click=move |_| on_close.run(()) on:keydown=move |e| if e.key() == "Escape" { e.stop_propagation(); on_close.run(()); }>
            <div class="dlg" role="dialog" aria-modal="true" aria-label="编辑未发送的消息" on:click=|e| e.stop_propagation()>
                <h3>"编辑未发送的消息"</h3>
                <textarea autofocus prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e))></textarea>
                {move || images.get().iter().enumerate().map(|(index, _)| view! {
                    <div class="row"><span>{format!("附件 {}", index + 1)}</span><button class="linkbtn" on:click=move |_| images.update(|images| { if index < images.len() { images.remove(index); } })>"移除"</button></div>
                }).collect_view()}
                <p class="muted">"文件上下文和原发送设置会保留。保存后可重试。"</p>
                <Show when=move || !error.get().is_empty()><div class="err" role="alert">{move || error.get()}</div></Show>
                <div class="dlg-foot"><button class="btn" on:click=move |_| on_close.run(())>"取消"</button><button class="btn primary" on:click=save>"保存"</button></div>
            </div>
        </div>
    }
}
