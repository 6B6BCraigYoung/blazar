use std::collections::HashSet;

use leptos::html;
use leptos::prelude::*;

use crate::components::status::{InlineError, LoadingState};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::chat_model::{
    Body, DeniedCall, Extra, Fold, Group, Item, Res, St, Step, Tool, ToolImage, fmt_tokens,
    group_sig, group_steps,
};
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::md;

use super::state::{Chat, SubLoad};
use super::turns::{Turn, group_turns};

#[derive(Clone, Copy)]
pub struct Opened(RwSignal<HashSet<String>>);

impl Opened {
    fn is(self, k: &str) -> bool {
        self.0.with_untracked(|s| s.contains(k))
    }
    fn set(self, k: &str, on: bool) {
        self.0.update(|s| {
            if on {
                s.insert(k.to_owned());
            } else {
                s.remove(k);
            }
        });
    }
}

const CHEVRON: &str = r#"<svg class="cc-fchev" viewBox="0 0 16 16"><path d="M6 4l4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#;
const COPY: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/></svg>"#;

fn details(
    opened: Opened,
    key: String,
    class: &'static str,
    summary: AnyView,
    body: impl FnOnce() -> AnyView + 'static,
) -> impl IntoView {
    let k = key.clone();
    let shown = RwSignal::new(opened.is(&key));
    let body = StoredValue::new_local(Some(body));
    view! {
        <details
            class=class
            open=opened.is(&key)
            on:toggle=move |e| {
                let open = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlDetailsElement>().ok())
                    .is_some_and(|d| d.open());
                opened.set(&k, open);
                if open && !shown.get_untracked() {
                    shown.set(true);
                }
            }
        >
            <summary>{summary}</summary>
            {move || {
                shown
                    .get()
                    .then(|| body.try_update_value(Option::take).flatten())
                    .flatten()
                    .map(untrack)
            }}
        </details>
    }
}

fn wrap_code(root: &web_sys::Element) {
    let Ok(list) = root.query_selector_all("pre") else {
        return;
    };
    let doc = document();
    for i in 0..list.length() {
        let Some(pre) = list
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let (Some(parent), Ok(wrap), Ok(button)) = (
            pre.parent_node(),
            doc.create_element("div"),
            doc.create_element("button"),
        ) else {
            continue;
        };
        if pre
            .parent_element()
            .is_some_and(|p| p.class_list().contains("cc-code"))
        {
            continue;
        }
        wrap.set_class_name("cc-code");
        button.set_class_name("cc-codecopy");
        let _ = button.set_attribute("type", "button");
        let _ = button.set_attribute("aria-label", "Copy code");
        let _ = button.set_attribute("title", "Copy");
        button.set_inner_html(COPY);
        if parent.insert_before(&wrap, Some(&pre)).is_ok() {
            let _ = wrap.append_child(&pre);
            let _ = wrap.append_child(&button);
        }
    }
}

fn copy_code(e: &leptos::ev::MouseEvent) {
    let Some(button) = e
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|t| t.closest(".cc-codecopy").ok().flatten())
    else {
        return;
    };
    let text = button
        .parent_element()
        .and_then(|w| w.query_selector("pre").ok().flatten())
        .and_then(|p| p.text_content())
        .unwrap_or_default();
    e.stop_propagation();
    let Some(w) = web_sys::window() else { return };
    let p = w.navigator().clipboard().write_text(&text);
    leptos::task::spawn_local(async move {
        if wasm_bindgen_futures::JsFuture::from(p).await.is_ok() {
            let _ = button.set_attribute("data-done", "true");
            gloo_timers::future::TimeoutFuture::new(1200).await;
            let _ = button.remove_attribute("data-done");
        } else {
            toast("Copy failed");
        }
    });
}

#[component]
pub fn Md(text: String, ws: String) -> impl IntoView {
    let el = NodeRef::<html::Div>::new();
    let html = md::render(&text, "", &ws);
    Effect::new(move |_| {
        let Some(root) = el.get() else { return };
        wrap_code(&root);
        crate::images_js::settle_images(&root);
        let Ok(list) = root.query_selector_all("pre code[class^=\"language-\"]:not([data-hl])")
        else {
            return;
        };
        for i in 0..list.length() {
            let Some(code) = list
                .item(i)
                .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
            else {
                continue;
            };
            let lang = code.class_name().trim_start_matches("language-").to_owned();
            let text = code.text_content().unwrap_or_default();
            let _ = code.set_attribute("data-hl", "");
            if text.len() > 20_000 {
                continue;
            }
            leptos::task::spawn_local(async move {
                if let Some(h) = crate::monaco::colorize(&text, &super::super::md_lang(&lang)).await
                    && code.is_connected()
                {
                    code.set_inner_html(&h);
                }
            });
        }
    });
    view! {
        <div
            class="cc-md md-body"
            node_ref=el
            inner_html=html
            on:click=move |e| copy_code(&e)
        ></div>
    }
}

fn images_view(images: &[ToolImage], ws: &str) -> Option<impl IntoView + use<>> {
    if images.is_empty() {
        return None;
    }
    let html: String = images
        .iter()
        .map(|i| md::figure(&i.src, &i.path, &i.name, "", ws, i.big))
        .collect();
    let el = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if let Some(root) = el.get() {
            crate::images_js::settle_images(&root);
        }
    });
    Some(view! { <div class="cc-imgs" node_ref=el inner_html=html></div> })
}

fn result_view(r: Res, key: String, opened: Opened) -> impl IntoView {
    let full = opened.is(&key);
    let show = RwSignal::new(full);
    let Res {
        shown,
        full: all,
        more,
        cls,
        wss,
    } = r;
    view! {
        <div class=format!("cc-out {cls}")>
            <span class="cc-elbow">"⎿"</span>
            <div class="cc-pre">
                {move || if show.get() { all.clone() } else { shown.clone() }}
                {move || {
                    (more > 0 && !show.get())
                        .then(|| {
                            let k = key.clone();
                            view! {
                                <button
                                    class="cc-more"
                                    on:click=move |_| {
                                        show.set(true);
                                        opened.set(&k, true);
                                    }
                                >
                                    {format!("Show {more} more lines")}
                                </button>
                            }
                        })
                }}
                {(!wss.is_empty())
                    .then(|| {
                        view! {
                            <div class="cc-wss">{wss.into_iter().map(ws_chip).collect_view()}</div>
                        }
                    })}
            </div>
        </div>
    }
}

const DIFF_PEEK: usize = 6;

fn number_width(last: usize) -> String {
    format!("--dn:{}ch", last.max(1).to_string().len().max(2))
}

pub(super) fn diff_style(rows: &[(char, String)], start: Option<u32>) -> Option<String> {
    let added = rows.iter().filter(|r| r.0 == '+').count();
    start.map(|s| number_width(s as usize + added.saturating_sub(1)))
}

pub(super) fn diff_rows(rows: &[(char, String)], start: Option<u32>) -> impl IntoView + use<> {
    let mut next = start;
    rows.iter()
        .map(|(c, l)| {
            let (cls, mark) = match c {
                '+' => ("dadd", "+"),
                '-' => ("ddel", "−"),
                _ => ("dgap", ""),
            };
            let num = (*c == '+').then_some(next).flatten().map(|n| n.to_string());
            if *c == '+' {
                next = next.map(|n| n + 1);
            }
            let text = if *c == '⋯' {
                "⋯".to_owned()
            } else {
                l.clone()
            };
            view! {
                <div class=cls>
                    {start.map(|_| view! { <span class="dn">{num}</span> })}
                    <span class="dm">{mark}</span> <span class="dt">{text}</span>
                </div>
            }
        })
        .collect_view()
}

pub(super) fn file_rows(rows: &[(char, String)]) -> impl IntoView + use<> {
    rows.iter()
        .enumerate()
        .map(|(i, (_, l))| {
            view! {
                <div class="dnew">
                    <span class="dn">{(i + 1).to_string()}</span>
                    <span class="dt">{l.clone()}</span>
                </div>
            }
        })
        .collect_view()
}

pub(super) fn file_head(added: usize) -> impl IntoView {
    view! {
        <div class="dnh">
            <span>"New file"</span>
            <i class="ok">{format!("+{added}")}</i>
        </div>
    }
}

pub(super) fn more_row(more: usize) -> Option<impl IntoView> {
    (more > 0).then(|| view! { <div class="dmore">{format!("… {more} more lines")}</div> })
}

fn diff_view(
    rows: Vec<(char, String)>,
    more: usize,
    start: Option<u32>,
    fresh: Option<usize>,
    opened: Opened,
    key: String,
) -> impl IntoView {
    let all = RwSignal::new(opened.is(&key));
    let hidden = rows.len().saturating_sub(DIFF_PEEK);
    let style = match fresh {
        Some(added) => Some(number_width(added)),
        None => diff_style(&rows, start),
    };
    let rows = StoredValue::new(rows);
    let shown = move || {
        let n = if all.get() { usize::MAX } else { DIFF_PEEK };
        rows.with_value(|r| r[..n.min(r.len())].to_vec())
    };
    let tail = move || (all.get() && more > 0).then(|| more_row(more));
    let block = match fresh {
        Some(added) => view! {
            <div class="cc-new" style=style title="View in Changes">
                {file_head(added)}
                {move || file_rows(&shown())}
                {tail}
            </div>
        }
        .into_any(),
        None => view! {
            <div
                class="cc-diff"
                style=style
                data-ln=start.is_some().to_string()
                title="View in Changes"
            >
                {move || diff_rows(&shown(), start)}
                {tail}
            </div>
        }
        .into_any(),
    };
    view! {
        {block}
        {move || {
            (hidden > 0 && !all.get())
                .then(|| {
                    let k = key.clone();
                    view! {
                        <button
                            class="cc-more"
                            on:click=move |_| {
                                all.set(true);
                                opened.set(&k, true);
                            }
                        >
                            {format!("Show {hidden} more lines")}
                        </button>
                    }
                })
        }}
    }
}

fn extra_view(e: &Extra, fresh: Option<usize>, opened: Opened, key: &str, ws: &str) -> AnyView {
    match e {
        Extra::None => ().into_any(),
        Extra::Cmd(c) => view! {
            <div class="cc-io">
                <div class="cc-pre">{c.clone()}</div>
            </div>
        }
        .into_any(),
        Extra::Todos(list) => todo_list(list).into_any(),
        Extra::Diff {
            rows, more, start, ..
        } => diff_view(
            rows.clone(),
            *more,
            *start,
            fresh,
            opened,
            format!("diff:{key}"),
        )
        .into_any(),
        Extra::Files(f) => view! {
            <div class="cc-diff">
                {f.iter().map(|x| view! { <div class="dh">{x.clone()}</div> }).collect_view()}
            </div>
        }
        .into_any(),
        Extra::Plan(p) => {
            let p = p.clone();
            let ws = ws.to_owned();
            details(
                opened,
                format!("plan:{key}"),
                "cc-planbody",
                view! { "Plan" }.into_any(),
                move || view! { <Md text=p ws /> }.into_any(),
            )
            .into_any()
        }
        Extra::Fleet(_) | Extra::Dispatch { .. } => ().into_any(),
    }
}

pub(super) fn ws_chip(id: String) -> impl IntoView {
    let app = crate::app_state::use_app();
    let wid = id.clone();
    let w = Memo::new(move |_| app.workspace(&wid));
    let href = format!("/w/{id}");
    let short: String = id.chars().take(8).collect();
    move || match w.get() {
        Some(w) => {
            let shown = app.shown_activity(&w);
            view! {
                <a class="cc-ws" href=href.clone() title=w.path.clone()>
                    <span class="dot" data-tone=crate::vocab::state(&shown).1.key()></span>
                    <span class="nm">{crate::fmt::ws_title(&w.name, &w.node)}</span>
                    {(w.node != "local")
                        .then(|| {
                            view! {
                                <span class="ws-machine">{crate::vocab_en::machine(&w.node)}</span>
                            }
                        })}
                </a>
            }
            .into_any()
        }
        None => {
            view! { <span class="cc-ws gone">{format!("workspace {short}…")}</span> }.into_any()
        }
    }
}

pub fn todo_list(list: &[crate::chat_model::Todo]) -> impl IntoView + use<> {
    view! {
        <div class="cc-todos">
            {list
                .iter()
                .map(|t| {
                    view! {
                        <div class=format!("cc-todo {}", t.status)>
                            <span>{if t.status == "completed" { "☒" } else { "☐" }}</span>
                            <span>{t.content.clone()}</span>
                        </div>
                    }
                })
                .collect_view()}
        </div>
    }
}

fn subagent_pending(chat: Chat, id: String) -> impl IntoView {
    let failed = {
        let id = id.clone();
        Memo::new(move |_| {
            chat.subagents
                .with(|s| s.get(&id) == Some(&SubLoad::Failed))
        })
    };
    view! {
        <div class="cc-subl">
            <div class="cc-subwait">
                {move || {
                    if failed.get() {
                        "Couldn't load the subagent transcript"
                    } else {
                        "Loading subagent transcript…"
                    }
                }}
                {move || {
                    failed
                        .get()
                        .then(|| {
                            let id = id.clone();
                            view! {
                                <button
                                    class="cc-subretry"
                                    on:click=move |_| chat.load_subagent(id.clone())
                                >
                                    "Retry"
                                </button>
                            }
                        })
                }}
            </div>
        </div>
    }
}

fn sub_summary(t: &Tool) -> String {
    if t.sub_calls == 0 {
        return "Subagent working…".to_owned();
    }
    let lead = match t.state {
        St::Run => "Running",
        St::Ok => "Done",
        St::Err => "Didn't finish",
    };
    format!("{lead} · {} steps · latest: {}", t.sub_calls, t.sub_last)
}

fn tool_view(t: Tool, chat: Chat, opened: Opened, ws: String) -> AnyView {
    if let Extra::Dispatch { ws: to, text } = &t.extra {
        return dispatch_view(&t, to.clone(), text.clone(), opened);
    }
    let sub = (!t.sub.is_empty() || t.sub_calls > 0).then(|| {
        let summary = sub_summary(&t);
        let steps = t.sub.clone();
        let ws = ws.clone();
        let elided = t.sub_elided;
        let id = t.id.clone();
        details(
            opened,
            format!("sub:{}", t.id),
            "cc-sub",
            view! { {summary} }.into_any(),
            move || {
                if elided {
                    if chat.subagents.with_untracked(|s| !s.contains_key(&id)) {
                        let load = id.clone();
                        queue_microtask(move || chat.load_subagent(load));
                    }
                    return subagent_pending(chat, id).into_any();
                }
                view! { <div class="cc-subl">{steps_view(steps, chat, opened, ws)}</div> }
                    .into_any()
            },
        )
    });
    let fleet = match &t.extra {
        Extra::Fleet(id) => Some(id.clone()),
        _ => None,
    };
    let counts = match (&t.extra, t.name.as_str()) {
        (Extra::Diff { added, removed, .. }, "Edit" | "MultiEdit" | "Write" | "NotebookEdit") => {
            Some((*added, *removed))
        }
        _ => None,
    };
    let fresh = match (&t.extra, t.name.as_str()) {
        (Extra::Diff { added, .. }, "Write") => Some(*added),
        _ => None,
    };
    let open = matches!(
        t.name.as_str(),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit"
    )
    .then(|| t.arg.clone())
    .filter(|a| !a.is_empty() && !a.starts_with('/') && !a.starts_with('~'));
    let open_tip = open.as_ref().map(|_| "Open in inspector");
    let locate = counts.and(open.clone());
    let line = match &t.extra {
        Extra::Diff { start, .. } => start.unwrap_or(0),
        _ => 0,
    };
    let tid = t.id.clone();
    let waiting = Memo::new(move |_| {
        chat.transcript.with(|tr| {
            tr.pending.iter().any(|p| {
                p.request["tool_use_id"].as_str() == Some(tid.as_str())
                    && !chat.decided.with(|d| d.contains(&p.id))
            })
        })
    });
    view! {
        <div
            class="cc-row cc-tool"
            data-st=t.state.as_str()
            data-name=t.name.clone()
            data-wait=move || waiting.get().to_string()
            data-path=locate.clone()
            data-line=locate.as_ref().map(|_| line.to_string())
        >
            <span class="cc-dot"></span>
            <div class="cc-main">
                <div class="cc-head">
                    <b>{t.label.clone()}</b>
                    {fleet.map(ws_chip)}
                    {(!t.tail.is_empty())
                        .then(|| view! { <span class="cc-tail">{t.tail.clone()}</span> })}
                    {(!t.arg.is_empty())
                        .then(|| {
                            view! {
                                <span class="cc-arg" data-open=open.clone() title=open_tip>
                                    {t.arg.clone()}
                                </span>
                            }
                        })}
                    {counts
                        .map(|(a, r)| {
                            view! {
                                <span class="cc-cnt">
                                    {(a > 0).then(|| view! { <i class="ok">{format!("+{a}")}</i> })}
                                    {(r > 0)
                                        .then(|| view! { <i class="bad">{format!("−{r}")}</i> })}
                                </span>
                            }
                        })}
                    {locate
                        .as_ref()
                        .map(|_| {
                            view! {
                                <span
                                    class="cc-go"
                                    title="Show in Changes"
                                    inner_html=CHEVRON
                                ></span>
                            }
                        })}
                </div>
                {extra_view(&t.extra, fresh, opened, &t.id, &ws)}
                {sub}
                <div class="cc-res">
                    {t.result.map(|r| result_view(r, format!("res:{}", t.id), opened))}
                </div>
                {images_view(&t.images, &ws)}
                {t
                    .raw
                    .clone()
                    .map(|raw| {
                        details(
                            opened,
                            format!("raw:{}", t.id),
                            "ui-detail cc-raw",
                            view! { "Details" }.into_any(),
                            move || view! { <pre>{raw}</pre> }.into_any(),
                        )
                    })}
            </div>
        </div>
    }
    .into_any()
}

/// Work handed to another workspace reads as a small card: who it went to, then the
/// prompt itself in body type, instead of one squeezed tool line.
fn dispatch_view(t: &Tool, to: String, text: String, opened: Opened) -> AnyView {
    let lead = match t.state {
        St::Run => "Sending to",
        St::Ok => "Sent to",
        St::Err => "Not sent to",
    };
    // "Dispatched" just repeats the header; only a refusal or error is worth a line.
    let result = t.result.clone().filter(|r| r.shown != "Dispatched");
    let raw = t.raw.clone();
    let id = t.id.clone();
    view! {
        <div class="cc-row cc-tool cc-dispatch" data-st=t.state.as_str() data-name=t.name.clone()>
            <span class="cc-dot"></span>
            <div class="cc-main">
                <div class="cc-dcard">
                    <div class="cc-dhead">
                        <span>{lead}</span>
                        {ws_chip(to)}
                    </div>
                    <div class="cc-dtext">{text}</div>
                </div>
                <div class="cc-res">
                    {result.map(|r| result_view(r, format!("res:{id}"), opened))}
                </div>
                {raw
                    .map(|raw| {
                        details(
                            opened,
                            format!("raw:{}", t.id),
                            "ui-detail cc-raw",
                            view! { "Details" }.into_any(),
                            move || view! { <pre>{raw}</pre> }.into_any(),
                        )
                    })}
            </div>
        </div>
    }
    .into_any()
}

fn reads_view(
    tools: Vec<Tool>,
    summary: String,
    state: St,
    chat: Chat,
    opened: Opened,
    ws: String,
) -> AnyView {
    let key = format!(
        "reads:{}",
        tools.first().map(|t| t.id.as_str()).unwrap_or_default()
    );
    let error = tools
        .iter()
        .find(|t| t.state == St::Err)
        .and_then(|t| t.result.as_ref())
        .map(|r| r.shown.lines().next().unwrap_or_default().to_owned())
        .filter(|l| !l.is_empty());
    view! {
        <div class="cc-row cc-reads" data-st=state.as_str()>
            <span class="cc-dot"></span>
            <div class="cc-main">
                {details(
                    opened,
                    key,
                    "cc-fd",
                    view! {
                        <span class="cc-fl">{summary}</span>
                        <span inner_html=CHEVRON></span>
                    }
                        .into_any(),
                    move || {
                        view! {
                            <div class="cc-fb">
                                {tools
                                    .into_iter()
                                    .map(|t| tool_view(t, chat, opened, ws.clone()))
                                    .collect_view()}
                            </div>
                        }
                            .into_any()
                    },
                )}
                {error
                    .map(|e| {
                        view! {
                            <div class="cc-out err">
                                <span class="cc-elbow">"⎿"</span>
                                <div class="cc-pre">{e}</div>
                            </div>
                        }
                    })}
            </div>
        </div>
    }
    .into_any()
}

fn group_view(g: Group, chat: Chat, opened: Opened, ws: String) -> AnyView {
    match g {
        Group::Reads {
            tools,
            summary,
            state,
        } => reads_view(tools, summary, state, chat, opened, ws),
        Group::One(s) => step_view(s, chat, opened, ws),
    }
}

fn steps_view(steps: Vec<Step>, chat: Chat, opened: Opened, ws: String) -> impl IntoView {
    group_steps(&steps)
        .into_iter()
        .map(|(_, g)| group_view(g, chat, opened, ws.clone()))
        .collect_view()
}

fn step_view(s: Step, chat: Chat, opened: Opened, ws: String) -> AnyView {
    match s {
        Step::Thinking { text, secs } => {
            let head = if secs > 0 {
                format!("Thought for {secs}s")
            } else {
                "Thinking".to_owned()
            };
            let body = if text.trim().is_empty() {
                view! { <span class="cc-fl">{head}</span> }.into_any()
            } else {
                let key = format!(
                    "think:{secs}:{}:{}",
                    text.len(),
                    text.chars().take(24).collect::<String>()
                );
                details(
                    opened,
                    key,
                    "cc-fd cc-think",
                    view! {
                        <span class="cc-fl">{head}</span>
                        <span inner_html=CHEVRON></span>
                    }
                    .into_any(),
                    move || view! { <Md text ws /> }.into_any(),
                )
                .into_any()
            };
            view! {
                <div class="cc-row cc-thought">
                    <span class="cc-dot" aria-hidden="true">
                        "✻"
                    </span>
                    <div class="cc-main">{body}</div>
                </div>
            }
            .into_any()
        }
        Step::Tool(t) => tool_view(*t, chat, opened, ws),
        Step::Text(t) => view! {
            <div class="cc-row cc-msg sub">
                <span class="cc-dot"></span>
                <div class="cc-main">
                    <Md text=t ws />
                </div>
            </div>
        }
        .into_any(),
        Step::Bg(t) => view! {
            <div class="cc-row cc-bg">
                <span class="cc-dot"></span>
                <div class="cc-main">
                    <div class="cc-head">
                        <b>"Background task"</b>
                        <span class="cc-arg">{t}</span>
                    </div>
                </div>
            </div>
        }
        .into_any(),
        Step::Note(t) => view! { <div class="cc-meta">{t}</div> }.into_any(),
        Step::Orphan(r) => {
            view! { <div class="cc-row cc-orphan">{result_view(r, String::new(), opened)}</div> }
                .into_any()
        }
    }
}

fn fold_view(f: Fold, chat: Chat, opened: Opened, ws: String) -> AnyView {
    view! {
        <div class="cc-fold" data-live=f.live.to_string()>
            {steps_view(f.steps, chat, opened, ws)}
        </div>
    }
    .into_any()
}

fn copy(text: String, done: RwSignal<bool>) {
    let Some(w) = web_sys::window() else { return };
    let p = w.navigator().clipboard().write_text(&text);
    leptos::task::spawn_local(async move {
        if wasm_bindgen_futures::JsFuture::from(p).await.is_ok() {
            let _ = done.try_set(true);
            gloo_timers::future::TimeoutFuture::new(1200).await;
            let _ = done.try_set(false);
        } else {
            toast("Copy failed");
        }
    });
}

pub fn split_fences(text: &str) -> Vec<(bool, String)> {
    let mut out: Vec<(bool, String)> = Vec::new();
    let mut fence: Option<usize> = None;
    let mut buf = String::new();
    for line in text.split_inclusive('\n') {
        let ticks = line.trim_start().chars().take_while(|c| *c == '`').count();
        let marker = ticks >= 3 && line.trim_start().starts_with("```");
        match fence {
            None if marker => {
                if !buf.is_empty() {
                    out.push((false, std::mem::take(&mut buf)));
                }
                fence = Some(ticks);
            }
            Some(n) if marker && ticks >= n && line.trim().chars().all(|c| c == '`') => {
                out.push((true, std::mem::take(&mut buf)));
                fence = None;
            }
            _ => buf.push_str(line),
        }
    }
    if !buf.is_empty() {
        out.push((fence.is_some(), buf));
    }
    out
}

fn user_segments(text: &str) -> AnyView {
    let parts = split_fences(text);
    if parts.len() == 1 && !parts[0].0 {
        return text.to_owned().into_any();
    }
    parts
        .into_iter()
        .map(|(code, body)| {
            if code {
                view! { <pre class="cc-ucode">{body.trim_end_matches('\n').to_owned()}</pre> }
                    .into_any()
            } else {
                body.into_any()
            }
        })
        .collect_view()
        .into_any()
}

#[component]
fn UserMsg(
    chat: Chat,
    sid: String,
    seq: u64,
    text: String,
    first: bool,
    long: bool,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let cp_key = format!("{sid}:{seq}");
    let cp = Memo::new(move |_| chat.checkpoints.with(|c| c.get(&cp_key).cloned()));
    let sid2 = sid.clone();
    let t2 = text.clone();
    let later = move || {
        chat.transcript.with_untracked(|t| {
            let mut after = false;
            let mut n = 0;
            for it in &t.items {
                if let Body::User {
                    sid: s,
                    seq: q,
                    first: f,
                    ..
                } = &it.body
                {
                    if after && *f {
                        n += 1;
                    }
                    if *s == sid2 && *q == seq {
                        after = true;
                    }
                }
            }
            n
        })
    };
    let later2 = later.clone();
    let sid_edit = sid.clone();
    let do_retry = move || {
        let t = draft.get_untracked().trim().to_owned();
        if t.is_empty() {
            return;
        }
        editing.set(false);
        chat.retry(sid_edit.clone(), Some(t), later2());
    };
    let do_retry2 = do_retry.clone();
    let menu = RwSignal::new(None::<(i32, i32)>);
    let sid_l = sid.clone();
    let latest = Memo::new(move |_| {
        chat.transcript.with(|t| {
            t.items
                .iter()
                .rev()
                .find_map(|it| match &it.body {
                    Body::User { sid: s, seq: q, .. } => Some(*s == sid_l && *q == seq),
                    _ => None,
                })
                .unwrap_or(false)
        })
    });
    let edit = Callback::new(move |()| {
        draft.set(t2.clone());
        editing.set(true);
    });
    let sid_r = sid.clone();
    let retry = Callback::new(move |()| chat.retry(sid_r.clone(), None, later()));
    let rewind = Callback::new(move |()| {
        if let Some(c) = cp.get_untracked() {
            chat.rewind(c, false);
        }
    });
    view! {
        <div
            class="cc-user"
            on:click=move |e: leptos::ev::MouseEvent| {
                let Some(u) = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|t| t.closest(".cc-user[data-stuck]").ok().flatten()) else {
                    return;
                };
                let on_control = e
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|t| { t.closest("button, a, textarea").ok().flatten().is_some() });
                if on_control {
                    return;
                }
                let log = u
                    .closest(".cc-log")
                    .ok()
                    .flatten()
                    .and_then(|l| l.dyn_into::<web_sys::HtmlElement>().ok());
                if let (Some(turn), Some(log)) = (u.closest(".cc-turn").ok().flatten(), log) {
                    let opts = web_sys::ScrollIntoViewOptions::new();
                    opts.set_block(web_sys::ScrollLogicalPosition::Start);
                    turn.scroll_into_view_with_scroll_into_view_options(&opts);
                    log.set_scroll_top(log.scroll_top() - 24);
                    mark_stuck(&log);
                }
            }
            on:contextmenu=move |e: leptos::ev::MouseEvent| {
                if editing.get_untracked() || (!first && cp.get_untracked().is_none()) {
                    return;
                }
                e.prevent_default();
                menu.set(Some((e.client_x(), e.client_y())));
            }
            data-first=first.to_string()
            data-long=long.to_string()
            data-expanded=move || open.get().to_string()
        >
            <Show
                when=move || !editing.get()
                fallback=move || {
                    let go = do_retry.clone();
                    let go2 = do_retry2.clone();
                    view! {
                        <div class="cc-edit">
                            <textarea
                                rows="3"
                                aria-label="Edit message and retry"
                                prop:value=move || draft.get()
                                on:input=move |e| draft.set(event_target_value(&e))
                                on:keydown=move |e| {
                                    if e.key() == "Escape" {
                                        e.prevent_default();
                                        e.stop_propagation();
                                        editing.set(false);
                                    }
                                    if e.key() == "Enter" && (e.meta_key() || e.ctrl_key()) {
                                        e.prevent_default();
                                        go();
                                    }
                                }
                            ></textarea>
                            <div class="row">
                                <span class="muted small">
                                    "Retrying restores files and removes this message and \
                                    everything after it from context."
                                </span>
                                <span class="grow"></span>
                                <button class="btn small" on:click=move |_| editing.set(false)>
                                    "Cancel"
                                </button>
                                <button class="btn small primary" on:click=move |_| go2()>
                                    "Retry"
                                </button>
                            </div>
                        </div>
                    }
                }
            >
                <div class="cc-uline">
                    <div class="cc-ut">{user_segments(&text)}</div>
                    {move || {
                        (latest.get() && !editing.get())
                            .then(|| {
                                view! {
                                    <span class="cc-acts">
                                        {first
                                            .then(|| {
                                                view! {
                                                    <button
                                                        class="cc-rw"
                                                        title="Edit and continue from here"
                                                        on:click=move |_| edit.run(())
                                                    >
                                                        "✎ Edit"
                                                    </button>
                                                    <button
                                                        class="cc-rw"
                                                        title="Retry with the same message"
                                                        on:click=move |_| retry.run(())
                                                    >
                                                        "⟳ Retry"
                                                    </button>
                                                }
                                            })}
                                        {move || {
                                            cp.get()
                                                .map(|_| {
                                                    view! {
                                                        <button
                                                            class="cc-rw"
                                                            title="Rewind code to here"
                                                            on:click=move |_| rewind.run(())
                                                        >
                                                            "↺ Rewind"
                                                        </button>
                                                    }
                                                })
                                        }}
                                    </span>
                                }
                            })
                    }}
                </div>
            </Show>
            {long
                .then(|| {
                    view! {
                        <button class="cc-xp" on:click=move |_| open.update(|o| *o = !*o)>
                            {move || if open.get() { "Show less" } else { "Show more" }}
                        </button>
                    }
                })}
            {move || {
                menu.get()
                    .map(|(x, y)| {
                        view! {
                            <MsgMenu
                                menu
                                x
                                y
                                first
                                edit
                                retry
                                rewind=cp.get().is_some().then_some(rewind)
                            />
                        }
                    })
            }}
        </div>
    }
}

#[component]
fn MsgMenu(
    menu: RwSignal<Option<(i32, i32)>>,
    x: i32,
    y: i32,
    first: bool,
    edit: Callback<()>,
    retry: Callback<()>,
    rewind: Option<Callback<()>>,
) -> impl IntoView {
    let close_down = window_event_listener(leptos::ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.closest(".ctx-menu").ok().flatten())
            .is_some();
        if !inside {
            menu.set(None);
        }
    });
    let close_key = window_event_listener(leptos::ev::keydown, move |e| {
        if e.key() == "Escape" {
            menu.set(None);
        }
    });
    let close_blur = window_event_listener(leptos::ev::blur, move |_| menu.set(None));
    on_cleanup(move || {
        close_down.remove();
        close_key.remove();
        close_blur.remove();
    });
    let size = |v: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
        v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as i32
    };
    let x = x.min(size(window().inner_width()) - 180).max(0);
    let y = y.min(size(window().inner_height()) - 104).max(0);
    let pick = move |f: Callback<()>| {
        move |_| {
            menu.set(None);
            f.run(());
        }
    };
    view! {
        <div class="ctx-menu" role="menu" style=format!("left:{x}px;top:{y}px")>
            {first
                .then(|| {
                    view! {
                        <button type="button" role="menuitem" class="ctx-item" on:click=pick(edit)>
                            "Edit"
                        </button>
                        <button type="button" role="menuitem" class="ctx-item" on:click=pick(retry)>
                            "Retry"
                        </button>
                    }
                })}
            {rewind
                .map(|r| {
                    view! {
                        <button type="button" role="menuitem" class="ctx-item" on:click=pick(r)>
                            "Rewind code to here"
                        </button>
                    }
                })}
        </div>
    }
}

fn warn_view(chat: Chat, denied: Vec<String>, calls: Vec<DeniedCall>, auto: bool) -> AnyView {
    let listed = move |denied: Vec<String>| {
        denied
            .into_iter()
            .map(|d| view! { <div class="muted">{format!("· {d}")}</div> })
            .collect_view()
    };
    if !auto {
        return view! {
            <div class="cc-row cc-warn">
                <span class="cc-dot"></span>
                <div class="cc-main">
                    {format!("{} actions were denied and not run:", denied.len())} {listed(denied)}
                    <div class="muted small">
                        "The current mode denies these actions. Switch to Edit automatically or Auto and retry."
                    </div>
                </div>
            </div>
        }
        .into_any();
    }
    let (cards, plain): (Vec<DeniedCall>, Vec<DeniedCall>) =
        calls.into_iter().partition(|c| c.reason.is_some());
    let rest: Vec<String> = if cards.is_empty() && plain.is_empty() {
        denied
    } else {
        plain
            .iter()
            .map(|c| format!("{}: {}", c.tool_name, c.what))
            .collect()
    };
    let place = place_of(chat);
    view! {
        <div class="cc-row cc-warn">
            <span class="cc-dot"></span>
            <div class="cc-main">
                {cards.into_iter().map(|c| denial_card(chat, c, place.clone())).collect_view()}
                {(!rest.is_empty())
                    .then(|| {
                        view! {
                            {format!("{} actions were denied and not run:", rest.len())}
                            {listed(rest)}
                        }
                    })}
            </div>
        </div>
    }
    .into_any()
}

fn place_of(chat: Chat) -> (String, String) {
    let app = crate::app_state::use_app();
    let id = chat.ws_id();
    let node = chat.node.get_value();
    let name = untrack(|| app.workspace(&id))
        .map(|w| crate::fmt::ws_title(&w.name, &w.node))
        .unwrap_or_else(|| "this workspace".to_owned());
    (
        format!(
            "Workspace {name} · machine {}",
            blazar_core_types::api::machine_label(&node)
        ),
        chat.root.get_value(),
    )
}

fn denial_card(chat: Chat, c: DeniedCall, place: (String, String)) -> AnyView {
    let key = c.key();
    let noun = if c.shell() {
        "This command"
    } else {
        "This action"
    };
    let state = if c.hard {
        "hard"
    } else {
        match c.decided {
            Some((true, _)) => "allowed",
            Some((false, _)) => "skipped",
            None => "open",
        }
    };
    let head = match (state, &c.decided) {
        ("hard", _) => format!("{noun} is outside the safety boundary and can't be approved"),
        ("allowed", Some((_, by))) => {
            format!(
                "{noun} was blocked by the safety check · allowed once ({by}), resent as your message"
            )
        }
        ("skipped", _) => format!("{noun} was blocked by the safety check · skipped"),
        _ => format!("{noun} was blocked by the safety check and not run"),
    };
    let k = key.clone();
    let busy = Memo::new(move |_| chat.denials.with(|d| d.busy(&k)));
    let done = RwSignal::new(false);
    let what = c.what.clone();
    let (sid_a, tid_a) = (c.sid.clone(), c.tool_use_id.clone());
    let (sid_n, tid_n) = (c.sid.clone(), c.tool_use_id.clone());
    let reason = c.reason.clone();
    let detail = c.detail.clone();
    let hard = c.hard;
    let (where_, root) = place;
    let target = c
        .shell()
        .then(|| blazar_core_types::denial::ssh_target(&c.what))
        .flatten();
    view! {
        <div class="cc-deny" data-state=state>
            <div class="cd-head">
                {hard
                    .then(|| {
                        view! { <span class="dot" data-tone="failed" aria-hidden="true"></span> }
                    })} {head}
            </div>
            <div class="cd-cmd">
                <pre class="acmd">{c.what.clone()}</pre>
                {hard
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="cc-copy"
                                aria-label="Copy command"
                                title="Copy"
                                data-done=move || done.get().to_string()
                                inner_html=COPY
                                on:click=move |_| copy(what.clone(), done)
                            ></button>
                        }
                    })}
            </div>
            <div class="cd-meta">
                <span title=root>{where_}</span>
                {target
                    .map(|h| {
                        view! {
                            <span title="The command actually runs on this machine">
                                {format!("Target {h}")}
                            </span>
                        }
                    })}
                {reason
                    .map(|r| {
                        view! {
                            <span title=format!(
                                "[{r}]",
                            )>
                                {format!(
                                    "Reason: {}",
                                    blazar_core_types::denial::reason_label_en(&r),
                                )}
                            </span>
                        }
                    })}
            </div>
            {(!detail.is_empty())
                .then(|| {
                    view! {
                        <details class="ui-detail">
                            <summary>"Why it was blocked"</summary>
                            <pre>{detail}</pre>
                        </details>
                    }
                })}
            {(state == "open")
                .then(|| {
                    view! {
                        <div class="cd-acts">
                            <button
                                type="button"
                                class="btn primary"
                                disabled=move || busy.get()
                                on:click=move |_| {
                                    chat.decide_denial(sid_a.clone(), tid_a.clone(), true)
                                }
                            >
                                "Allow once and retry"
                            </button>
                            <button
                                type="button"
                                class="btn ghost"
                                disabled=move || busy.get()
                                on:click=move |_| {
                                    chat.decide_denial(sid_n.clone(), tid_n.clone(), false)
                                }
                            >
                                "Skip"
                            </button>
                        </div>
                    }
                })}
            {hard
                .then(|| {
                    view! {
                        <div class="cd-note">
                            "To run it, copy the command into a terminal yourself."
                        </div>
                    }
                })}
        </div>
    }
    .into_any()
}

fn error_view(chat: Chat, text: String, by_account: bool) -> AnyView {
    let switch = move |_| {
        chat.spawn(async move {
            let rt = untrack(move || chat.runtime());
            let last = chat.view.get_untracked().and_then(|v| {
                chat.threads.with_untracked(|t| {
                    t.iter().find(|x| x.id == v).and_then(|x| x.account.clone())
                })
            });
            let list: Vec<crate::api::Account> = chat.accounts.with_untracked(|a| {
                a.as_ref()
                    .map(|a| {
                        a.accounts
                            .iter()
                            .filter(|x| {
                                x.provider == rt && Some(&x.id) != last.as_ref() && x.usable()
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            });
            if list.is_empty() {
                toast("No other account available. Add or log in to one on the Runtimes page.");
                return;
            }
            let mut choices = vec![Choice::plain("Cancel")];
            choices.extend(list.iter().map(|a| Choice::plain(a.label.clone())));
            if let Some(i) = dialog::ask(
                "Continue with another account",
                "Keep the conversation and continue on another account.",
                choices,
            )
            .await
                && i > 0
            {
                chat.continue_on_another(list[i - 1].id.clone());
            }
        });
    };
    let can = by_account
        && super::state::ACC_RUNTIMES.contains(&untrack(move || chat.runtime()).as_str());
    let (head, detail) = match text.split_once('\n') {
        Some((h, d)) => (
            h.to_owned(),
            Some(d.trim().to_owned()).filter(|d| !d.is_empty()),
        ),
        None => (text, None),
    };
    view! {
        <div class="cc-row cc-err">
            <span class="cc-dot"></span>
            <div class="cc-main">
                {head}
                {detail
                    .map(|d| {
                        view! {
                            <details class="ui-detail">
                                <summary>"Details"</summary>
                                <pre>{d}</pre>
                            </details>
                        }
                    })}
                {can
                    .then(|| {
                        view! {
                            <div>
                                <button class="linkbtn" on:click=switch>
                                    "Continue with another account"
                                </button>
                            </div>
                        }
                    })}
            </div>
        </div>
    }
    .into_any()
}

fn retry_view(chat: Chat, why: String, attempt: u32, max: u32, until: i64) -> AnyView {
    let now = RwSignal::new(js_sys::Date::now() as i64);
    let ticker = StoredValue::new_local(Some(gloo_timers::callback::Interval::new(
        1000,
        move || {
            let _ = now.try_set(js_sys::Date::now() as i64);
        },
    )));
    on_cleanup(move || ticker.set_value(None));
    let when = move || {
        let left = ((until - now.get()).max(0) + 999) / 1000;
        if left > 0 {
            format!("Retrying in {left:>2}s")
        } else {
            "Retrying".to_owned()
        }
    };
    let round = format!(
        "attempt {}",
        crate::vocab_en::retry_round(attempt.into(), max.into())
    );
    view! {
        <div class="cc-meta cc-retry" role="status">
            <span class="dot" aria-hidden="true"></span>
            <span>{why}</span>
            <span class="sep">"·"</span>
            <span class="cc-countdown">{when}</span>
            <span class="sep">"·"</span>
            <span>{round}</span>
            {move || {
                chat.running
                    .get()
                    .then(|| {
                        view! {
                            <span class="sep">"·"</span>
                            <button type="button" class="linkbtn" on:click=move |_| chat.stop()>
                                "Stop"
                            </button>
                        }
                    })
            }}
        </div>
    }
    .into_any()
}

fn item_view(it: Item, chat: Chat, opened: Opened, ws: String, copyable: bool) -> AnyView {
    let key = it.key.clone();
    match it.body {
        Body::User {
            sid,
            seq,
            text,
            first,
            long,
        } => view! { <UserMsg chat sid seq text first long /> }.into_any(),
        Body::Assistant { text } => {
            let done = RwSignal::new(false);
            let t = text.clone();
            view! {
                <div class="cc-row cc-msg">
                    <span class="cc-dot"></span>
                    <div class="cc-main">
                        <Md text ws />
                        {copyable
                            .then(|| {
                                view! {
                                    <div class="cc-acts2">
                                        <button
                                            class="cc-copy"
                                            aria-label="Copy response"
                                            title="Copy"
                                            data-done=move || done.get().to_string()
                                            inner_html=COPY
                                            on:click=move |_| copy(t.clone(), done)
                                        ></button>
                                    </div>
                                }
                            })}
                    </div>
                </div>
            }
            .into_any()
        }
        Body::Fold(f) => fold_view(f, chat, opened, ws),
        Body::Error {
            text, by_account, ..
        } => error_view(chat, text, by_account),
        Body::Warn {
            denied,
            calls,
            auto,
        } => warn_view(chat, denied, calls, auto),
        Body::Meta { text, bad } => view! {
            <div class="cc-meta" class:bad=bad>
                {text}
            </div>
        }
        .into_any(),
        Body::Retry {
            why,
            attempt,
            max,
            until,
        } => retry_view(chat, why, attempt, max, until),
        Body::Rewound { turns, items } => details(
            opened,
            format!("rw:{key}"),
            "cc-rewound",
            view! { {format!("Rewound · {turns} messages removed from context")} }.into_any(),
            move || {
                view! {
                    <div class="cc-rwl">
                        {items
                            .into_iter()
                            .map(|i| item_view(i, chat, opened, ws.clone(), false))
                            .collect_view()}
                    </div>
                }
                .into_any()
            },
        )
        .into_any(),
    }
}

fn fold_slot(key: String, chat: Chat, opened: Opened, ws: String) -> AnyView {
    let k = key.clone();
    let live = Memo::new(move |_| {
        chat.transcript.with(|t| match t.item(&k) {
            Some(Item {
                body: Body::Fold(f),
                ..
            }) => f.live,
            _ => false,
        })
    });
    let groups = move || {
        chat.transcript.with(|t| match t.item(&key) {
            Some(Item {
                body: Body::Fold(f),
                ..
            }) => group_steps(&f.steps)
                .into_iter()
                .map(|(i, g)| (i, group_sig(&g), g))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
    };
    view! {
        <div class="cc-fold" data-live=move || live.get().to_string()>
            <For each=groups key=|(i, sig, _)| (*i, *sig) let:group>
                {group_view(group.2, chat, opened, ws.clone())}
            </For>
        </div>
    }
    .into_any()
}

fn item_slot(
    key: String,
    chat: Chat,
    opened: Opened,
    ws: String,
    last_reply: Memo<Option<String>>,
) -> AnyView {
    let k = key.clone();
    let fold = Memo::new(move |_| {
        chat.transcript.with(|t| {
            matches!(
                t.item(&k),
                Some(Item {
                    body: Body::Fold(_),
                    ..
                })
            )
        })
    });
    let k = key.clone();
    let sig = Memo::new(move |_| chat.transcript.with(|t| t.item(&k).map(|it| it.sig)));
    let k = key.clone();
    let copyable = Memo::new(move |_| last_reply.with(|l| l.as_deref() == Some(k.as_str())));
    (move || {
        if fold.get() {
            return fold_slot(key.clone(), chat, opened, ws.clone());
        }
        sig.track();
        match chat.transcript.with_untracked(|t| t.item(&key).cloned()) {
            Some(it) => item_view(it, chat, opened, ws.clone(), copyable.get()),
            None => ().into_any(),
        }
    })
    .into_any()
}

const SPIN: [&str; 12] = ["·", "✢", "*", "✶", "✻", "✽", "✽", "✻", "✶", "*", "✢", "·"];
const WORDS: [&str; 12] = [
    "Thinking",
    "Pondering",
    "Musing",
    "Brewing",
    "Cogitating",
    "Percolating",
    "Mulling",
    "Crafting",
    "Tinkering",
    "Noodling",
    "Ruminating",
    "Synthesizing",
];

fn fmt_secs(sec: u64) -> String {
    if sec < 60 {
        format!("{sec}s")
    } else if sec < 3600 {
        format!("{}m {}s", sec / 60, sec % 60)
    } else {
        format!("{}h {}m", sec / 3600, sec % 3600 / 60)
    }
}

#[component]
fn Spinner(chat: Chat, tick: RwSignal<u32>) -> impl IntoView {
    let word = RwSignal::new(0usize);
    let changes = StoredValue::new((0u32, 0u32));
    let started = js_sys::Date::now();
    Effect::new(move |_| {
        let t = tick.get();
        let (at, n) = changes.get_value();
        let gap = [17u32, 25, 42].get(n as usize).copied().unwrap_or(42);
        if t.wrapping_sub(at) > gap {
            changes.set_value((t, n + 1));
            word.update(|w| *w = (*w + 1 + (js_sys::Math::random() * 10.0) as usize) % WORDS.len());
        }
    });
    let mode = move || chat.effective_mode().map(|m| m.0).unwrap_or("");
    let elapsed = move || {
        tick.track();
        fmt_secs(((js_sys::Date::now() - started) / 1000.0) as u64)
    };
    let tokens = move || {
        chat.transcript
            .with(|t| (t.turn_tokens > 0).then(|| fmt_tokens(t.turn_tokens)))
    };
    view! {
        <div class="cc-spin" data-mode=mode role="status">
            <span class="ic" aria-hidden="true">
                {move || SPIN[tick.get() as usize % SPIN.len()]}
            </span>
            <span class="tx">{move || format!("{}…", WORDS[word.get()])}</span>
            <span class="tk">{elapsed}</span>
            {move || {
                tokens().map(|n| view! { <span class="tk">{format!("· ↑{n} tokens")}</span> })
            }}
            {move || {
                chat.running.get().then(|| view! { <span class="tk">"· esc to interrupt"</span> })
            }}
        </div>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(module = "/js/scroll.js")]
extern "C" {
    #[wasm_bindgen(js_name = keepScroll)]
    fn keep_scroll(el: &web_sys::HtmlElement);
    #[wasm_bindgen(js_name = holdTail)]
    fn hold_tail(el: &web_sys::HtmlElement);
    #[wasm_bindgen(js_name = holdBottom)]
    fn hold_bottom(el: &web_sys::HtmlElement);
}

const WINDOW: usize = 120;

fn mark_stuck(log: &web_sys::HtmlElement) {
    let Ok(list) = log.query_selector_all(".cc-user") else {
        return;
    };
    let top = log.get_bounding_client_rect().top();
    for i in 0..list.length() {
        let Some(u) = list
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            continue;
        };
        let r = u.get_bounding_client_rect();
        let stuck = u.has_attribute("data-stuck");
        if r.top() <= top + 0.5 && r.bottom() > top {
            if !stuck {
                let _ = u.set_attribute("data-stuck", "true");
                let gap = r.height() - u.get_bounding_client_rect().height();
                let _ = u.style().set_property("--stuck-gap", &format!("{gap}px"));
            }
        } else if stuck {
            let _ = u.remove_attribute("data-stuck");
            let _ = u.style().remove_property("--stuck-gap");
        }
    }
}

#[component]
pub fn Log(chat: Chat, tick: RwSignal<u32>, log_h: RwSignal<f64>) -> impl IntoView {
    let opened = Opened(RwSignal::new(HashSet::new()));
    let el = NodeRef::<html::Div>::new();
    let at_bottom = StoredValue::new(true);
    let last_top = StoredValue::new(0i32);
    let frozen = RwSignal::new(None::<usize>);
    let ws = chat.ws_id();
    let sizer = StoredValue::new_local(None::<(web_sys::ResizeObserver, Closure<dyn FnMut()>)>);
    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        let Some(e) = el.get() else { return false };
        let target: web_sys::HtmlElement = e.clone().into();
        let measure = Closure::<dyn FnMut()>::new(move || {
            let target = target.clone();
            request_animation_frame(move || {
                let _ = log_h.try_set(f64::from(target.client_height()));
            });
        });
        if let Ok(o) = web_sys::ResizeObserver::new(measure.as_ref().unchecked_ref()) {
            o.observe(&e);
            sizer.set_value(Some((o, measure)));
        }
        true
    });
    on_cleanup(move || {
        sizer.try_update_value(|s| {
            if let Some((o, _)) = s.take() {
                o.disconnect();
            }
        });
    });
    Effect::new(move |_| {
        chat.view.track();
        opened.0.set(HashSet::new());
        at_bottom.set_value(true);
        frozen.set(None);
    });
    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        el.get().map(|e| keep_scroll(&e)).is_some()
    });
    let start = move |len: usize| {
        frozen
            .get()
            .unwrap_or_else(|| len.saturating_sub(WINDOW))
            .min(len)
    };
    let turns = Memo::new(move |_| {
        chat.transcript.with(|t| {
            group_turns(
                t.items[start(t.items.len())..]
                    .iter()
                    .map(|it| (it.key.clone(), matches!(it.body, Body::User { .. }))),
            )
        })
    });
    let turn_keys = move |head: String| {
        move || {
            turns.with(|ts| {
                ts.iter()
                    .find(|t| t.head == head)
                    .map(|t| t.keys.clone())
                    .unwrap_or_default()
            })
        }
    };
    let earlier = move || chat.transcript.with(|t| start(t.items.len()));
    let show_earlier = move |_| {
        if let Some(e) = el.get_untracked() {
            hold_tail(&e);
        }
        frozen.set(Some(earlier().saturating_sub(WINDOW)));
    };
    Effect::new(move |_| {
        chat.transcript.track();
        chat.local_errors.track();
        chat.pending.track();
        if at_bottom.get_value()
            && let Some(e) = el.get()
        {
            hold_bottom(&e);
        }
    });
    Effect::new(move |prev: Option<usize>| {
        let n = chat.pending.with(Vec::len);
        if prev.is_some_and(|p| n > p) {
            at_bottom.set_value(true);
        }
        n
    });
    let dim = move || {
        chat.transcript.with(|t| {
            t.pending
                .iter()
                .any(|p| !p.ask && !chat.decided.with(|d| d.contains(&p.id)))
        })
    };
    let busy = move || {
        (chat.running.get() || chat.pending.with(|p| !p.is_empty()))
            && chat.transcript.with(|t| {
                t.pending
                    .iter()
                    .all(|p| chat.decided.with(|d| d.contains(&p.id)))
            })
    };
    let app = crate::app_state::use_app();
    let node = chat.node.get_value();
    let unreachable = Memo::new(move |_| app.node_unreachable(&node));
    let empty = move || {
        !unreachable.get()
            && !chat.loading.get()
            && chat.rows.with(Vec::is_empty)
            && chat.local_errors.with(Vec::is_empty)
            && chat.pending.with(Vec::is_empty)
    };
    let click = move |e: leptos::ev::MouseEvent| {
        let Some(t) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let find = |sel: &str| t.closest(sel).ok().flatten();
        if let Some(row) = find(".cc-turn[data-current=\"false\"] .cc-tool[data-path]") {
            let path = row.get_attribute("data-path").unwrap_or_default();
            let line = row
                .get_attribute("data-line")
                .and_then(|l| l.parse().ok())
                .unwrap_or(0);
            chat.diff.locate(path, line);
            chat.show_diff.run(());
        } else if let Some(a) = find("a[href^=\"#open:\"]") {
            e.prevent_default();
            let raw = a.get_attribute("href").unwrap_or_default();
            let p = raw.trim_start_matches(md::OPEN_PREFIX);
            let p = js_sys::decode_uri_component(p).map_or_else(|_| p.to_owned(), String::from);
            chat.files.open(p, 0);
        } else if let Some(el) = find("[data-open]") {
            let p = el.get_attribute("data-open").unwrap_or_default();
            let (path, line) = match p.rsplit_once(':') {
                Some((f, l)) if l.parse::<u32>().is_ok() => (f.to_owned(), l.parse().unwrap_or(0)),
                _ => (p, 0),
            };
            chat.files.open(path, line);
        } else if find(".cc-tool[data-name] :is(.cc-diff, .cc-new)").is_some() {
            chat.show_diff.run(());
        }
    };
    view! {
        <div
            class="cc-log"
            node_ref=el
            on:click=click
            data-dim=move || dim().to_string()
            on:scroll=move |_| {
                if let Some(e) = el.try_get_untracked().flatten() {
                    mark_stuck(&e);
                    let top = e.scroll_top();
                    let near = e.scroll_height() - top - e.client_height() < 80;
                    if top + 1 < last_top.get_value() {
                        at_bottom.set_value(false);
                    } else if near {
                        at_bottom.set_value(true);
                    }
                    last_top.set_value(top);
                    let bottom = at_bottom.get_value();
                    if bottom {
                        if frozen.get_untracked().is_some() {
                            frozen.set(None);
                        }
                    } else if frozen.get_untracked().is_none() {
                        frozen.set(Some(earlier()));
                    }
                }
            }
        >
            <Show when=move || unreachable.get()>
                <super::offline::Offline chat />
            </Show>
            <Show when=move || chat.loading.get() && !unreachable.get()>
                <LoadingState text="Loading conversation…" class="cc-load" />
            </Show>
            <Show when=empty>
                <Empty chat />
            </Show>
            <div class="cc-flow" hidden=move || unreachable.get()>
                <Show when=move || { earlier() > 0 }>
                    <button type="button" class="cc-more cc-earlier" on:click=show_earlier>
                        {move || format!("Show {} earlier", earlier().min(WINDOW))}
                    </button>
                </Show>
                <For
                    each=move || turns.get()
                    key=|t| t.head.clone()
                    children=move |turn: Turn| {
                        let ws = ws.clone();
                        let head = turn.head.clone();
                        let current = Memo::new(move |_| {
                            turns.with(|ts| ts.last().is_some_and(|t| t.head == head))
                        });
                        let head = turn.head.clone();
                        let last_reply = Memo::new(move |_| {
                            turns
                                .with(|ts| {
                                    let t = ts.iter().find(|t| t.head == head)?;
                                    chat.transcript
                                        .with(|tr| {
                                            t.keys
                                                .iter()
                                                .rev()
                                                .find(|k| {
                                                    matches!(
                                                        tr.item(k),
                                                        Some(Item { body: Body::Assistant { .. }, .. })
                                                    )
                                                })
                                                .cloned()
                                        })
                                })
                        });
                        view! {
                            <div class="cc-turn" data-current=move || current.get().to_string()>
                                <For each=turn_keys(turn.head) key=|k| k.clone() let:k>
                                    {item_slot(k, chat, opened, ws.clone(), last_reply)}
                                </For>
                            </div>
                        }
                    }
                />
                <For each=move || chat.pending.get() key=|p| p.id let:p>
                    <div class="cc-turn">
                        <div class="cc-user pending">
                            <div class="cc-ut">
                                {p.text.clone()}
                                {(p.images > 0).then(|| format!("\n[{} attachments]", p.images))}
                            </div>
                        </div>
                    </div>
                </For>
                {move || {
                    chat.local_errors
                        .get()
                        .into_iter()
                        .map(|e| {
                            view! {
                                <div class="cc-row cc-err">
                                    <span class="cc-dot" aria-hidden="true"></span>
                                    <div class="cc-main">
                                        <InlineError message=e />
                                    </div>
                                </div>
                            }
                        })
                        .collect_view()
                }}
                <Show when=busy>
                    <Spinner chat tick />
                </Show>
            </div>
        </div>
    }
}

#[component]
fn Empty(chat: Chat) -> impl IntoView {
    move || {
        let p = chat.profile();
        let starters = p.as_ref().map(|p| p.starters.clone()).unwrap_or_default();
        view! {
            <div class="cc-empty">
                {match &p {
                    Some(p) => {
                        view! {
                            <div class="who">
                                <b>{format!("Chat with {}", p.name)}</b>
                                <div class="muted small">
                                    {p.description.clone().unwrap_or_default()}
                                </div>
                            </div>
                        }
                            .into_any()
                    }
                    None => {
                        let rt = chat.runtime();
                        let name = match rt.as_str() {
                            "claude" => "Claude Code",
                            "codex" => "Codex",
                            other => other,
                        }
                            .to_owned();
                        view! {
                            <div class="cc-hero">
                                <span
                                    class="cc-hero-mark"
                                    inner_html=crate::rt_logo::glyph(&rt)
                                ></span>
                                <span>{name}</span>
                            </div>
                            <div class="cc-tip">
                                "Press "<kbd>"Shift"</kbd>" "<kbd>"Tab"</kbd>
                                " to cycle permission modes, or type "<code>"/"</code>
                                " for commands."
                            </div>
                        }
                            .into_any()
                    }
                }}
                {(!starters.is_empty())
                    .then(|| {
                        view! {
                            <div class="starters">
                                {starters
                                    .into_iter()
                                    .map(|s| {
                                        let pr = s.prompt.clone();
                                        view! {
                                            <button
                                                class="starter"
                                                on:click=move |_| chat.prompt.set(pr.clone())
                                            >
                                                <b>{s.label}</b>
                                                <span>{s.prompt}</span>
                                            </button>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        }
                    })}
            </div>
        }
    }
}
