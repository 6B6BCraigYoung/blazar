use leptos::ev;
use leptos::html;
use leptos::prelude::*;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use blazar_core_types::failure::FailureCause;

use crate::chat_model::Body;
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::failure::Failure;
use crate::storage;

use super::dock::Dock;
use super::state::{ACC_RUNTIMES, Attach, Chat, mode_label};

mod pickers;
mod presentation;
mod queue;
mod status;

use pickers::{FilesPop, agent_pop, mcp_pop, mode_pop, model_pop, plus_pop, slash_pop, style_pop};
use presentation::{eff_label, icon, svg};
use queue::{FailedEditor, QueueBand, QueueEditor};
use status::{RateBanner, Tray};

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

const CODEX_SENT: &[(&str, &str, &str)] = &[
    (
        "compact",
        "summarize conversation to prevent hitting the context limit",
        "",
    ),
    (
        "review",
        "review my current changes and find issues",
        "[instructions]",
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
fn FailedCard(
    chat: Chat,
    key: String,
    text: String,
    failure: Failure,
    dismissed: RwSignal<Option<String>>,
) -> impl IntoView {
    let app = crate::app_state::use_app();
    let headline = failure.headline_en();
    let primary = failure.primary_en();
    let node_href = failure.node_href();
    let cause = failure.cause.as_str();
    let raw = text
        .split_once('\n')
        .map_or(text.clone(), |(_, d)| d.trim().to_owned());
    let detail = Some(raw).filter(|r| !r.is_empty() && *r != headline);
    let f = StoredValue::new(failure);
    let primary_btn = NodeRef::<html::Button>::new();
    let act = move || {
        let f = f.get_value();
        match f.cause {
            FailureCause::Network | FailureCause::Busy => chat.continue_thread(),
            FailureCause::Node => crate::failure::probe_node(app, f.node, true),
            FailureCause::Login => crate::failure::login(f.runtime, f.account, true),
            FailureCause::Quota => chat.spawn(async move {
                if let Some(id) =
                    crate::failure::pick_other_account(&f.runtime, f.account.as_deref(), true).await
                {
                    chat.continue_on_another(id);
                }
            }),
            FailureCause::Resume | FailureCause::Other => chat.retry(f.session, None, 0),
        }
    };
    let handled = move || {
        let f = f.get_value();
        dismissed.set(Some(key.clone()));
        crate::failure::mark_handled(app, f.workspace, Some(f.session), true);
        chat.spawn(async move { chat.load_threads().await });
    };
    gloo_timers::callback::Timeout::new(300, move || {
        let typing = document().active_element().is_some_and(|a| {
            a.closest("input, textarea, [contenteditable]")
                .ok()
                .flatten()
                .is_some()
        });
        if !typing && let Some(b) = primary_btn.try_get_untracked().flatten() {
            let _ = b.focus();
        }
    })
    .forget();
    view! {
        <div
            class="cc-failed"
            role="alert"
            data-cause=cause
            on:keydown=move |e: ev::KeyboardEvent| {
                if e.key() == "Enter" && !e.is_composing() {
                    e.prevent_default();
                    act();
                }
            }
        >
            <div class="cf-head">
                <span class="dot" aria-hidden="true"></span>
                <span>{headline}</span>
            </div>
            {detail
                .map(|d| {
                    view! {
                        <details class="ui-detail">
                            <summary>"Error details"</summary>
                            <pre>{d}</pre>
                        </details>
                    }
                })}
            <div class="cf-acts">
                <button type="button" class="linkbtn" on:click=move |_| handled()>
                    "Mark as handled"
                </button>
                <span class="grow"></span>
                {node_href
                    .map(|href| {
                        view! {
                            <a class="btn" href=href>
                                "Open machine"
                            </a>
                        }
                    })}
                <button
                    type="button"
                    class="btn primary"
                    node_ref=primary_btn
                    on:click=move |_| act()
                >
                    {primary}
                    <kbd>"↩"</kbd>
                </button>
            </div>
        </div>
    }
}

#[component]
pub fn Composer(
    chat: Chat,
    tree_files: Signal<Vec<String>>,
    log_h: RwSignal<f64>,
) -> impl IntoView {
    let pop = RwSignal::new(None::<Pop>);
    let sel = RwSignal::new(0usize);
    let ta = NodeRef::<html::Textarea>::new();
    ta.on_load(|t| {
        let idle = document()
            .active_element()
            .is_none_or(|a| a.tag_name() == "BODY");
        if idle {
            let _ = t.focus();
        }
    });
    let file_in = NodeRef::<html::Input>::new();

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
            let mut items: Vec<SlashItem> = CODEX_LOCAL
                .iter()
                .map(|(n, d)| SlashItem::local(n, d))
                .collect();
            if chat.runtime() == "codex" {
                for (n, d, h) in CODEX_SENT {
                    items.push(SlashItem {
                        name: (*n).to_owned(),
                        desc: (*d).to_owned(),
                        hint: (*h).to_owned(),
                        local: false,
                    });
                }
            }
            items
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
        let name = Some(f.name())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "image".into());
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
                        name,
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
    let stop_mode = move || running.get() && chat.prompt.with(|p| p.trim().is_empty());

    let model_label = move || {
        let rt = chat.runtime();
        let s = chat.model_sel(&rt);
        let list = chat.models_for(&rt);
        let m = list
            .iter()
            .find(|x| x.id == s.model.clone().unwrap_or_default())
            .or_else(|| list.first());
        let name = m
            .map(|m| {
                m.label
                    .replace(" (recommended)", "")
                    .replace(" (Recommended)", "")
            })
            .filter(|n| !n.is_empty() && n != "Default")
            .unwrap_or_else(|| "Default".to_owned());
        (name, s.effort.map(|e| eff_label(&e)))
    };

    let review_n = move || chat.diff.comments.with(Vec::len);
    let app = crate::app_state::use_app();
    let node = chat.node.get_value();
    let unreachable = Memo::new(move |_| app.node_unreachable(&node));
    let lay = use_context::<super::super::layout::LayoutState>();
    let maxed = move || lay.is_some_and(|l| l.lay.get().max);
    let deciding = Memo::new(move |_| {
        chat.transcript.with(|t| {
            chat.decided
                .with(|d| crate::chat_model::first_open(&t.pending, |id| d.contains(id)).is_some())
        })
    });
    let dismissed = RwSignal::new(None::<String>);
    let failed = Memo::new(move |_| {
        if chat.running.get() || chat.pending.with(|p| !p.is_empty()) || unreachable.get() {
            return None;
        }
        let view = chat.view.get();
        let thread_meta = chat.threads.with(|t| {
            t.iter()
                .find(|x| Some(&x.id) == view.as_ref())
                .map(|x| (x.handled, x.account.clone()))
        });
        if thread_meta.as_ref().is_some_and(|(handled, _)| *handled) {
            return None;
        }
        chat.transcript.with(|t| {
            let last = t.items.last()?;
            let Body::Error { text, cause, .. } = &last.body else {
                return None;
            };
            if dismissed.get().as_deref() == Some(last.key.as_str()) {
                return None;
            }
            let sid = t.items.iter().rev().find_map(|i| match &i.body {
                Body::User { sid, .. } => Some(sid.clone()),
                _ => None,
            })?;
            let ws = chat.ws_id();
            let thread = view.clone().unwrap_or_else(|| sid.clone());
            let failure = app
                .workspace(&ws)
                .and_then(|w| Failure::of_workspace(&w))
                .filter(|f| f.thread == thread)
                .unwrap_or_else(|| Failure {
                    cause: *cause,
                    message: text
                        .split_once('\n')
                        .map_or(text.clone(), |(_, d)| d.trim().to_owned()),
                    workspace: ws,
                    session: sid,
                    thread,
                    node: chat.node.get_value(),
                    runtime: chat.runtime(),
                    account: thread_meta.and_then(|(_, a)| a),
                    resets_at: None,
                });
            Some((last.key.clone(), text.clone(), failure))
        })
    });
    let failed_edit = RwSignal::new(None::<(u32, Value)>);

    view! {
        <div class="composer">
            <Show when=move || pop.get().is_some()>
                <div class="cb-pop">
                    {move || match pop.get() {
                        Some(Pop::Slash) => slash_pop(slash_items(), sel, run_slash).into_any(),
                        Some(Pop::Files) => {
                            view! {
                                <FilesPop
                                    chat
                                    tree_files
                                    insert_ref=Callback::new(insert_ref)
                                    insert_at_caret=Callback::new(insert_at_caret)
                                    pop
                                />
                            }
                                .into_any()
                        }
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
            <RateBanner chat />
            {move || {
                (review_n() > 0)
                    .then(|| {
                        view! {
                            <div class="cb-band review">
                                <span>
                                    <b>{review_n()}</b>
                                    " review comments attached"
                                </span>
                                <span class="grow"></span>
                                <button class="linkbtn" on:click=move |_| chat.show_diff.run(())>
                                    "View"
                                </button>
                                <button
                                    class="linkbtn"
                                    on:click=move |_| {
                                        let n = review_n();
                                        chat.spawn(async move {
                                            if dialog::ask(
                                                    "Discard review comments",
                                                    &format!("Discard {n} unsent review comments?"),
                                                    vec![Choice::plain("Cancel"), Choice::danger("Discard")],
                                                )
                                                .await == Some(1)
                                            {
                                                chat.clear_review();
                                            }
                                        });
                                    }
                                >
                                    "Clear"
                                </button>
                            </div>
                        }
                    })
            }}
            {move || {
                let list = chat.shells.get();
                let busy = chat.shell_busy.get();
                (!list.is_empty() || busy)
                    .then(|| {
                        view! {
                            <div class="cb-band queue cc-shells">
                                <div class="cq-h">
                                    <b>"Shell"</b>
                                    <span class="muted">" · included with your next message"</span>
                                    <span class="grow"></span>
                                    {(!list.is_empty())
                                        .then(|| {
                                            view! {
                                                <button
                                                    class="linkbtn"
                                                    on:click=move |_| chat.shells.set(Vec::new())
                                                >
                                                    "Clear"
                                                </button>
                                            }
                                        })}
                                </div>
                                {list
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, r)| {
                                        let lines = r.stdout.lines().count()
                                            + r.stderr.lines().count();
                                        let body = format!(
                                            "{}{}{}",
                                            r.stdout.trim_end(),
                                            if r.stderr.trim().is_empty() || r.stdout.trim().is_empty()
                                            {
                                                ""
                                            } else {
                                                "\n"
                                            },
                                            r.stderr.trim_end(),
                                        );
                                        view! {
                                            <details class="cc-shell">
                                                <summary>
                                                    <code>{format!("! {}", r.command)}</code>
                                                    <span class="muted">
                                                        {format!(
                                                            " · exit {} · {lines} lines{}",
                                                            r.code,
                                                            if r.truncated { " · truncated" } else { "" },
                                                        )}
                                                    </span>
                                                    <button
                                                        class="x"
                                                        title="Remove"
                                                        on:click=move |e| {
                                                            e.prevent_default();
                                                            chat.shells
                                                                .update(|s| {
                                                                    if i < s.len() {
                                                                        s.remove(i);
                                                                    }
                                                                });
                                                        }
                                                    >
                                                        "×"
                                                    </button>
                                                </summary>
                                                <pre>
                                                    {if body.is_empty() {
                                                        "(no output)".to_owned()
                                                    } else {
                                                        body
                                                    }}
                                                </pre>
                                            </details>
                                        }
                                    })
                                    .collect_view()}
                                {busy
                                    .then(|| {
                                        view! { <div class="muted cc-shell-run">"Running…"</div> }
                                    })}
                            </div>
                        }
                    })
            }}
            <QueueBand chat />
            {move || {
                let thread = chat.view.get();
                chat.deliveries
                    .with(|d| d.failed())
                    .into_iter()
                    .filter(|(_, request)| request["resume_session"].as_str() == thread.as_deref())
                    .map(|(id, request)| {
                        let text = request["text"].as_str().unwrap_or_default().to_owned();
                        let images = request["images"].as_array().map_or(0, Vec::len);
                        view! {
                            <div class="cb-band queue">
                                <div class="cq-h">
                                    <b>"Failed to send"</b>
                                    <span class="grow"></span>
                                    <button class="linkbtn" on:click=move |_| chat.retry_failed(id)>
                                        "Retry"
                                    </button>
                                    <button
                                        class="linkbtn"
                                        on:click=move |_| {
                                            failed_edit.set(Some((id, request.clone())))
                                        }
                                    >
                                        "Edit"
                                    </button>
                                    <button
                                        class="linkbtn"
                                        on:click=move |_| {
                                            chat.spawn(async move {
                                                if dialog::ask(
                                                        "Discard message?",
                                                        "This message was not sent. Discarding it cannot be undone.",
                                                        vec![Choice::plain("Cancel"), Choice::danger("Discard")],
                                                    )
                                                    .await == Some(1)
                                                {
                                                    let _ = chat
                                                        .deliveries
                                                        .try_update(|deliveries| deliveries.take_failed(id));
                                                }
                                            })
                                        }
                                    >
                                        "Discard"
                                    </button>
                                </div>
                                <div class="cq-t">
                                    {text}
                                    {(images > 0).then(|| format!(" · {images} attachments"))}
                                </div>
                            </div>
                        }
                    })
                    .collect_view()
            }}
            {move || chat.editing_queue.get().map(|queued| view! { <QueueEditor chat queued /> })}
            {move || {
                failed_edit
                    .get()
                    .map(|(id, request)| {
                        view! {
                            <FailedEditor
                                chat
                                id
                                request
                                on_close=Callback::new(move |_| failed_edit.set(None))
                            />
                        }
                    })
            }}
            {move || (!maxed()).then(|| view! { <Dock chat /> })}
            {move || {
                failed
                    .get()
                    .map(|(key, text, failure)| {
                        view! { <FailedCard chat key text failure dismissed /> }
                    })
            }}
            <div
                class="cc-box"
                hidden=move || deciding.get() || failed.with(Option::is_some)
                data-off=move || unreachable.get().to_string()
                data-bash=move || chat.prompt.with(|p| p.trim_start().starts_with('!')).to_string()
                on:dragover=move |e: ev::DragEvent| {
                    if e
                        .data_transfer()
                        .is_some_and(|d| {
                            (0..d.items().length())
                                .any(|i| {
                                    d.items()
                                        .get(i)
                                        .is_some_and(|x| x.type_().starts_with("image/"))
                                })
                        })
                    {
                        e.prevent_default();
                    }
                }
                on:drop=move |e: ev::DragEvent| {
                    let imgs = files_of(e.data_transfer().and_then(|d| d.files()));
                    if !imgs.is_empty() {
                        e.prevent_default();
                        e.stop_propagation();
                        imgs.into_iter().for_each(add_image);
                    }
                }
            >
                <Tray chat log_h />
                {move || {
                    let list = chat.attach.get();
                    (!list.is_empty())
                        .then(|| {
                            view! {
                                <div class="cb-atts">
                                    {list
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, a)| {
                                            view! {
                                                <span class="cb-att" title=a.name.clone()>
                                                    <img class="ic" src=a.url alt="" />
                                                    <span class="nm">{a.name.clone()}</span>
                                                    <button
                                                        class="x"
                                                        aria-label=format!("Remove {}", a.name)
                                                        title="Remove"
                                                        on:click=move |_| {
                                                            chat.attach
                                                                .update(|l| {
                                                                    if i < l.len() {
                                                                        l.remove(i);
                                                                    }
                                                                })
                                                        }
                                                    >
                                                        "×"
                                                    </button>
                                                </span>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            }
                        })
                }}
                <textarea
                    node_ref=ta
                    rows="1"
                    class="prompt"
                    disabled=move || unreachable.get()
                    aria-label="Message"
                    placeholder=move || {
                        if unreachable.get() {
                            "Reconnect the machine to send"
                        } else if running.get() {
                            if matches!(chat.runtime().as_str(), "claude" | "codex") {
                                "Message the agent while it works…"
                            } else {
                                "Queue another message…"
                            }
                        } else {
                            "⌘ Esc to focus or unfocus Claude"
                        }
                    }
                    prop:value=move || chat.prompt.get()
                    on:input=on_input
                    on:keydown=on_key
                    on:paste=move |e: ev::ClipboardEvent| {
                        let imgs = files_of(e.clipboard_data().and_then(|d| d.files()));
                        if !imgs.is_empty() {
                            e.prevent_default();
                            imgs.into_iter().for_each(add_image);
                        }
                    }
                ></textarea>
                <div class="cb-bar">
                    <button
                        class="cb-btn cb-trigger cb-add"
                        aria-label="Add"
                        title="Add images, mention project files or the current file"
                        on:click=move |_| {
                            pop.update(|p| {
                                *p = if *p == Some(Pop::Plus) { None } else { Some(Pop::Plus) };
                            })
                        }
                    >
                        <span class="ic" inner_html=icon("plus")></span>
                        <span class="cb-tx">"Add"</span>
                    </button>
                    <button
                        class="cb-btn cb-trigger cb-slash"
                        aria-label="Commands"
                        title="Slash commands (/)"
                        on:click=move |_| {
                            if !chat.prompt.get_untracked().starts_with('/') {
                                chat.prompt.set("/".into());
                            }
                            sel.set(0);
                            pop.set(Some(Pop::Slash));
                            focus();
                        }
                    >
                        <span class="ic" inner_html=icon("slash")></span>
                        <span class="cb-tx">"Commands"</span>
                    </button>
                    {move || {
                        (chat.runtime() == "claude")
                            .then(|| {
                                let (n, bad) = chat
                                    .transcript
                                    .with(|t| t.mcp.clone())
                                    .iter()
                                    .fold(
                                        (0, 0),
                                        |(n, b), m| (n + 1, b + usize::from(m.status == "failed")),
                                    );
                                let tip = match (n, bad) {
                                    (0, _) => "MCP servers".to_owned(),
                                    (n, 0) => format!("MCP servers ({n})"),
                                    (n, bad) => format!("MCP servers ({n}, {bad} failed)"),
                                };
                                view! {
                                    <button
                                        class="cb-btn cb-trigger cb-mcp"
                                        data-bad=(bad > 0).to_string()
                                        aria-label="MCP servers"
                                        title=tip
                                        on:click=move |_| {
                                            chat.load_catalog();
                                            pop.update(|p| {
                                                *p = if *p == Some(Pop::Mcp) {
                                                    None
                                                } else {
                                                    Some(Pop::Mcp)
                                                };
                                            });
                                        }
                                    >
                                        <span class="ic" inner_html=icon("plug")></span>
                                        <span class="cb-tx">
                                            "MCP"
                                            {(n > 0).then(|| view! { <span class="muted">{format!(" {n}")}</span> })}
                                        </span>
                                    </button>
                                }
                            })
                    }}
                    <button
                        class="cb-btn cb-trigger cb-model"
                        aria-label="Model"
                        title="Switch model and effort"
                        on:click=move |_| {
                            pop.update(|p| {
                                *p = if *p == Some(Pop::Model) { None } else { Some(Pop::Model) };
                            })
                        }
                    >
                        <span class="ic" inner_html=icon("chip")></span>
                        <span class="cb-tx">{move || model_label().0}</span>
                        {move || {
                            model_label()
                                .1
                                .map(|e| view! { <span class="muted">{format!("· {e} effort")}</span> })
                        }}
                    </button>
                    <Show when=move || chat.fresh.get() && chat.view.get().is_some()>
                        <button
                            type="button"
                            class="cb-btn cb-fresh"
                            aria-label="Keep the current conversation"
                            title="The next message starts a new conversation. Click to cancel"
                            on:click=move |_| chat.fresh.set(false)
                        >
                            <span class="ic" inner_html=icon("chat")></span>
                            <span class="cb-tx">"New conversation"</span>
                            <span class="muted">"×"</span>
                        </button>
                    </Show>
                    {move || {
                        chat.files
                            .current
                            .get()
                            .map(|f| {
                                let on = chat.ctx_on.get().as_deref() == Some(f.as_str());
                                let name = f.rsplit('/').next().unwrap_or(&f).to_owned();
                                let tip = if on {
                                    format!("{f}\nIn this message's context. Click to remove")
                                } else {
                                    format!("{f}\nAdd the open file to this message's context")
                                };
                                let f2 = f.clone();
                                view! {
                                    <button
                                        type="button"
                                        class="cb-btn cb-ctx"
                                        data-on=on.to_string()
                                        aria-label=if on { "Remove current file" } else { "Add current file" }
                                        title=tip
                                        on:click=move |_| {
                                            chat.ctx_on.set(if on { None } else { Some(f2.clone()) })
                                        }
                                    >
                                        <span class="ic" inner_html=icon(if on { "tick" } else { "plus" })></span>
                                        <span class="cb-tx">{name}</span>
                                    </button>
                                }
                            })
                    }}
                    <span class="cb-grow"></span>
                    <span class="cb-right">
                        <button
                            class="cb-btn cb-trigger cb-agent"
                            aria-label="Agent and runtime"
                            title="Switch agent, runtime or account, keeping the conversation"
                            on:click=move |_| {
                                pop.update(|p| {
                                    *p = if *p == Some(Pop::Agent) {
                                        None
                                    } else {
                                        Some(Pop::Agent)
                                    };
                                })
                            }
                        >
                            <span
                                class="rt-ic"
                                inner_html=move || crate::rt_logo::glyph(&chat.runtime())
                            ></span>
                            <span class="cb-tx">
                                {move || chat.agent_label()}
                                {move || {
                                    let rt = chat.runtime();
                                    (ACC_RUNTIMES.contains(&rt.as_str())
                                        && chat.agent.get().starts_with("r:"))
                                        .then(|| {
                                            let label = match chat.current_account(&rt) {
                                                Some(id) => chat.account(&id).map(|a| a.label)?,
                                                None => "Auto-balance".to_owned(),
                                            };
                                            Some(view! { <span class="muted">{format!(" · {label}")}</span> })
                                        })
                                        .flatten()
                                }}
                            </span>
                            <span class="chev" inner_html=icon("chev")></span>
                        </button>
                        {move || {
                            chat.effective_mode()
                                .map(|m| {
                                    let from = if !chat.perm.get().is_empty() {
                                        ""
                                    } else if chat
                                        .profile()
                                        .and_then(|p| p.permission_mode)
                                        .is_some()
                                    {
                                        " (from agent)"
                                    } else {
                                        " (default)"
                                    };
                                    view! {
                                        <button
                                            class="cb-btn cb-trigger cb-mode"
                                            data-ui-guard=""
                                            data-mode=m.0
                                            aria-label="Permission mode"
                                            title=format!(
                                                "Permission mode: {}{from}\n{}\nShift+Tab to switch",
                                                mode_label(m.0),
                                                m.2,
                                            )
                                            on:click=move |_| {
                                                pop.update(|p| {
                                                    *p = if *p == Some(Pop::Mode) {
                                                        None
                                                    } else {
                                                        Some(Pop::Mode)
                                                    };
                                                })
                                            }
                                        >
                                            <span class="ic" inner_html=icon(m.3)></span>
                                            <span class="cb-tx">{mode_label(m.0)}</span>
                                        </button>
                                    }
                                })
                        }}
                        <button
                            class="cb-send"
                            data-mode=move || if stop_mode() { "stop" } else { "send" }
                            data-busy=move || (chat.busy.get() > 0).to_string()
                            data-pmode=move || chat.effective_mode().map(|m| m.0).unwrap_or("")
                            aria-label=move || {
                                if stop_mode() {
                                    "Stop"
                                } else if running.get()
                                    && !matches!(chat.runtime().as_str(), "claude" | "codex")
                                {
                                    "Queue"
                                } else {
                                    "Send"
                                }
                            }
                            title=move || {
                                if stop_mode() {
                                    "Stop (Esc)"
                                } else if running.get()
                                    && !matches!(chat.runtime().as_str(), "claude" | "codex")
                                {
                                    "Queue (Enter)"
                                } else {
                                    "Send (Enter)"
                                }
                            }
                            disabled=move || unreachable.get() && !stop_mode()
                            on:click=move |_| if stop_mode() { chat.stop() } else { chat.send() }
                        >
                            <span
                                class="i-send"
                                inner_html=svg(r#"<path d="M12 19V5M5.5 11.5 12 5l6.5 6.5"/>"#)
                            ></span>
                            <span class="i-stop"></span>
                        </button>
                    </span>
                    <input
                        type="file"
                        node_ref=file_in
                        accept="image/png,image/jpeg,image/gif,image/webp"
                        multiple
                        hidden
                        on:change=move |e| {
                            let input: web_sys::HtmlInputElement = event_target(&e);
                            files_of(input.files()).into_iter().for_each(add_image);
                            input.set_value("");
                        }
                    />
                </div>
            </div>
        </div>
    }
}
