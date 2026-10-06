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
use presentation::{eff_label, fmt_dur, fmt_k, icon, svg};
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
    local: bool,
}

impl SlashItem {
    fn local(name: &str, desc: &str) -> Self {
        Self {
            name: name.to_owned(),
            desc: desc.to_owned(),
            hint: String::new(),
            local: true,
        }
    }
}

const CLAUDE_LOCAL: &[(&str, &str)] = &[
    ("clear", "Start a new session with empty context"),
    ("effort", "Set effort level for model usage"),
    ("fast", "Toggle fast mode"),
    ("mcp", "Manage MCP servers"),
    ("model", "Set the AI model for Claude Code"),
    ("output-style", "List output styles or switch to one"),
    (
        "rewind",
        "Restore the code and/or conversation to a previous point",
    ),
    (
        "usage",
        "Show session cost, plan usage, and what's contributing to your limits",
    ),
];

const CODEX_LOCAL: &[(&str, &str)] = &[
    ("clear", "clear the terminal and start a new chat"),
    ("mention", "mention a file"),
    ("model", "choose what model and reasoning effort to use"),
    ("new", "start a new chat during a conversation"),
    ("permissions", "choose what Codex is allowed to do"),
];

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
    Effect::new(move |_| {
        let key = format!("blazar.draft.{}", chat.ws_id());
        chat.prompt.with(|p| {
            if p.is_empty() {
                storage::remove(&key);
            } else {
                storage::save_raw(&key, p);
            }
        });
    });
    let recall = StoredValue::new(None::<(usize, String)>);
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
        let mut out: Vec<SlashItem> = if chat.runtime() == "claude" {
            chat.load_catalog();
            let mut cli: Vec<SlashItem> = chat.catalog.with(|c| {
                c.as_ref()
                    .map(|c| c.commands.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| !c.name.starts_with('_'))
                    .map(|c| SlashItem {
                        local: CLAUDE_LOCAL.iter().any(|(n, _)| *n == c.name),
                        name: c.name,
                        desc: c.description,
                        hint: c.argument_hint,
                    })
                    .collect()
            });
            for (n, d) in CLAUDE_LOCAL {
                if !cli.iter().any(|c| c.name == *n) {
                    cli.push(SlashItem::local(n, d));
                }
            }
            cli
        } else {
            CODEX_LOCAL
                .iter()
                .map(|(n, d)| SlashItem::local(n, d))
                .collect()
        };
        out.retain(|c| {
            k.is_empty() || c.name.to_lowercase().contains(&k) || c.desc.to_lowercase().contains(&k)
        });
        out.sort_by(|a, b| {
            let rank = |c: &SlashItem| {
                let n = c.name.to_lowercase();
                if n.starts_with(&k) {
                    0
                } else if n.contains(&k) {
                    1
                } else {
                    2
                }
            };
            rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name))
        });
        out
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
    let usage = move || {
        let windows = chat.transcript.with_untracked(|t| t.rate.clone());
        let body = if windows.is_empty() {
            "No usage data yet. It appears after the first Claude turn.".to_owned()
        } else {
            windows
                .iter()
                .map(|w| {
                    format!(
                        "{}: {}%",
                        crate::fmt::window_label_en(&w.name),
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
            toast("Nothing to rewind. Checkpoints are only created in Git workspaces.");
            return;
        }
        chat.spawn(async move {
            let mut ch = vec![Choice::plain("Cancel")];
            ch.extend(users.iter().map(|u| Choice::plain(u.1.clone())));
            if let Some(i) = dialog::ask(
                "Rewind",
                "Pick a message to restore files to the state before it was sent.",
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
        if c.local {
            chat.prompt.set(String::new());
            match c.name.as_str() {
                "clear" | "new" => chat.fresh.set(true),
                "mention" => {
                    chat.load_snippets();
                    pop.set(Some(Pop::Files));
                }
                "model" | "effort" => pop.set(Some(Pop::Model)),
                "permissions" => pop.set(Some(Pop::Mode)),
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
            toast("Images must be 5 MB or smaller");
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
        recall.set_value(None);
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
                    request_animation_frame(|| {
                        if let Ok(Some(row)) =
                            document().query_selector(".cb-pop .cp-row[data-sel=\"true\"]")
                        {
                            let opts = web_sys::ScrollIntoViewOptions::new();
                            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
                            row.scroll_into_view_with_scroll_into_view_options(&opts);
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
        if (k == "ArrowUp" || k == "ArrowDown")
            && !composing
            && pop.get_untracked().is_none()
            && !(e.shift_key() || e.alt_key() || e.meta_key() || e.ctrl_key())
            && let Some(t) = ta.try_get_untracked().flatten()
        {
            let v = chat.prompt.get_untracked();
            let at = t.selection_start().ok().flatten().unwrap_or(0) as usize;
            let end = t.selection_end().ok().flatten().unwrap_or(0) as usize;
            let u: Vec<u16> = v.encode_utf16().collect();
            let nl = u16::from(b'\n');
            let edge = at == end
                && if k == "ArrowUp" {
                    !u[..at.min(u.len())].contains(&nl)
                } else {
                    !u[end.min(u.len())..].contains(&nl)
                };
            if edge {
                let history = chat.prompt_history();
                let next = match (recall.get_value(), k.as_str()) {
                    (None, "ArrowUp") if !history.is_empty() => {
                        Some((history.len() - 1, v.clone()))
                    }
                    (Some((i, draft)), "ArrowUp") if i > 0 => Some((i - 1, draft)),
                    (Some((i, draft)), "ArrowDown") if i + 1 < history.len() => {
                        Some((i + 1, draft))
                    }
                    (Some((_, draft)), "ArrowDown") => {
                        recall.set_value(None);
                        e.prevent_default();
                        chat.prompt.set(draft);
                        return;
                    }
                    _ => None,
                };
                if let Some((i, draft)) = next {
                    e.prevent_default();
                    let text = history[i].clone();
                    let len = text.encode_utf16().count() as u32;
                    recall.set_value(Some((i, draft)));
                    chat.prompt.set(text);
                    request_animation_frame(move || {
                        if let Some(t) = ta.try_get_untracked().flatten() {
                            let _ = t.set_selection_range(len, len);
                        }
                    });
                    return;
                }
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
        (format!("--p:{pct:.1}"), title, !running.get())
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
                        Some(Pop::Slash) => slash_pop(slash_items(), sel, run_slash).into_any(),
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
                    <span><b>{review_n()}</b>" review comments attached"</span><span class="grow"></span>
                    <button class="linkbtn" on:click=move |_| chat.show_diff.run(())>"View"</button>
                    <button class="linkbtn" on:click=move |_| {
                        let n = review_n();
                        chat.spawn(async move {
                            if dialog::ask("Discard review comments", &format!("Discard {n} unsent review comments?"), vec![Choice::plain("Cancel"), Choice::danger("Discard")]).await == Some(1) {
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
                view! { <div class="cb-band queue"><div class="cq-h"><b>"Failed to send"</b><span class="grow"></span><button class="linkbtn" on:click=move |_| chat.retry_failed(id)>"Retry"</button><button class="linkbtn" on:click=move |_| failed_edit.set(Some((id, request.clone())))>"Edit"</button><button class="linkbtn" on:click=move |_| chat.spawn(async move {
                    if dialog::ask("Discard message?", "This message was not sent. Discarding it cannot be undone.", vec![Choice::plain("Cancel"), Choice::danger("Discard")]).await == Some(1) {
                        let _ = chat.deliveries.try_update(|deliveries| deliveries.take_failed(id));
                    }
                })>"Discard"</button></div><div class="cq-t">{text}{(images > 0).then(|| format!(" · {images} attachments"))}</div></div> }
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
                <textarea node_ref=ta rows="1" class="prompt" aria-label="Message"
                    placeholder=move || if running.get() { "Queue another message…" } else { "⌘ Esc to focus or unfocus Claude" }
                    prop:value=move || chat.prompt.get()
                    on:input=on_input on:keydown=on_key
                    on:paste=move |e: ev::ClipboardEvent| {
                        let imgs = files_of(e.clipboard_data().and_then(|d| d.files()));
                        if !imgs.is_empty() { e.prevent_default(); imgs.into_iter().for_each(add_image); }
                    }></textarea>
                <div class="cb-bar">
                    <button class="cb-ic cb-trigger" aria-label="Add context" title="Add context" inner_html=icon("plus") on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Plus) { None } else { Some(Pop::Plus) })></button>
                    <button class="cb-ic cb-trigger" aria-label="Slash commands" title="Slash commands (/)" inner_html=icon("slash") on:click=move |_| {
                        if !chat.prompt.get_untracked().starts_with('/') { chat.prompt.set("/".into()); }
                        sel.set(0);
                        pop.set(Some(Pop::Slash));
                        focus();
                    }></button>
                    {move || (chat.runtime() == "claude").then(|| {
                        let (n, bad) = chat.transcript.with(|t| t.mcp.clone()).iter().fold((0, 0), |(n, b), m| (n + 1, b + usize::from(m.status == "failed")));
                        view! { <button class="cb-ic cb-trigger" data-bad=(bad > 0).to_string() aria-label="MCP servers" title=format!("MCP servers{}", if n > 0 { format!(" ({n})") } else { String::new() })
                            inner_html=icon("plug") on:click=move |_| { chat.load_catalog(); pop.update(|p| *p = if *p == Some(Pop::Mcp) { None } else { Some(Pop::Mcp) }); }></button> }
                    })}
                    <button class="cb-chip cb-trigger cb-model" title="Switch model" on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Model) { None } else { Some(Pop::Model) })>
                        <span class="cb-tx">{move || { let (n, e) = model_label(); view! { {n}{e.map(|e| view! { " "<span class="muted">{e}</span> })} } }}</span>
                    </button>
                {move || chat.ctx_file().map(|f| {
                    let name = f.rsplit('/').next().unwrap_or(&f).to_owned();
                    let f2 = f.clone();
                    view! {
                        <span class="cb-chip cb-ctx" title=format!("Current file: {f}")>
                            <span inner_html=icon("file")></span><span class="nm cb-tx">{name}</span>
                            <button class="x" aria-label="Remove current file" title="Remove current file" on:click=move |e| { e.stop_propagation(); chat.ctx_off.set(Some(f2.clone())); }>"×"</button>
                        </span>
                    }
                })}
                <Show when=move || chat.fresh.get() && chat.view.get().is_some()>
                    <button type="button" class="cb-chip cb-fresh" aria-label="Keep the current conversation" title="The next message starts a new conversation" on:click=move |_| chat.fresh.set(false)>"New conversation ×"</button>
                </Show>
                    {move || { let (st, title, empty) = ring(); view! { <span class="cb-ring" data-empty=empty.to_string() data-run=running.get().to_string() style=st title=title></span> } }}
                    <span class="cb-time">{timer}</span>
                    <span class="cb-grow"></span>
                    <span class="cb-right">
                    <button class="cb-chip cb-plain cb-trigger cb-agent" title="Switch agent or account, keeping the conversation"
                        on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Agent) { None } else { Some(Pop::Agent) })>
                        <span class="rt-dot" data-rt=move || chat.runtime()></span>
                        <span class="cb-tx">{move || chat.agent_label()}
                        {move || {
                            let rt = chat.runtime();
                            (ACC_RUNTIMES.contains(&rt.as_str()) && chat.agent.get().starts_with("r:")).then(|| {
                                chat.current_account(&rt).and_then(|id| chat.account(&id)).map(|a| view! { <span class="muted">{format!(" · {}", a.label)}</span> })
                            }).flatten()
                        }}</span>
                    </button>
                    {move || chat.effective_mode().map(|m| {
                        let from = if !chat.perm.get().is_empty() { "" } else if chat.profile().and_then(|p| p.permission_mode).is_some() { " (from agent)" } else { " (default)" };
                        view! {
                            <button class="cb-chip cb-plain cb-trigger cb-mode" data-mode=m.0 title=format!("{}{from}", m.2)
                                on:click=move |_| pop.update(|p| *p = if *p == Some(Pop::Mode) { None } else { Some(Pop::Mode) })>
                                <span inner_html=icon(m.3)></span><span>{m.1}</span>
                            </button>
                        }
                    })}
                    <button class="cb-send" data-mode=move || if stop_mode() { "stop" } else { "send" } data-busy=move || (chat.busy.get() > 0).to_string()
                        data-pmode=move || chat.effective_mode().map(|m| m.0).unwrap_or("")
                        aria-label=move || if stop_mode() { "Stop" } else if running.get() { "Queue" } else { "Send" }
                        title=move || if stop_mode() { "Stop (Esc)" } else if running.get() { "Queue (Enter)" } else { "Send (Enter)" }
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
