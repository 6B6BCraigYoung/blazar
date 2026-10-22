use blazar_core_types::api::WorkspaceView;
use leptos::prelude::*;
use serde_json::Value;

use crate::app_state::{AppData, use_app};
use crate::pages::workspaces::diff_badge;
use crate::{api, fmt, storage};

use crate::components::status::StateOf;

const LOOK_KEY: &str = "blazar.children.look";
const NODE_H: usize = 64;
const NODE_GAP: usize = 12;
const EDGE_W: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Cards,
    Graph,
}

#[derive(Clone, Copy)]
pub struct ChildrenOpen(pub RwSignal<bool>);

fn open_key(id: &str) -> String {
    format!("blazar.children.open.{id}")
}

pub fn children_open(id: &str) -> RwSignal<bool> {
    let open = RwSignal::new(storage::load_raw(&open_key(id)).as_deref() != Some("0"));
    let key = open_key(id);
    Effect::new(move |_| storage::save_raw(&key, if open.get() { "1" } else { "0" }));
    open
}

pub(super) fn add_child(app: AppData, id: String) {
    let name = app.workspace(&id).map(|w| w.name).unwrap_or_default();
    app.new_ws_parent.set(Some((id, name)));
    app.new_ws.set(true);
}

fn say(w: &WorkspaceView, g: &Value) -> (String, &'static str) {
    let prompt = g["prompt"].as_str();
    let reply = g["reply"].as_str();
    let newer_prompt = match (g["prompt_at"].as_str(), g["reply_at"].as_str()) {
        (Some(p), Some(r)) => p > r,
        (Some(_), None) => true,
        _ => false,
    };
    if w.activity == "running"
        && newer_prompt
        && let Some(p) = prompt
    {
        return (format!("正在做：{p}"), "");
    }
    if let Some(r) = reply {
        return if g["failed"].as_bool() == Some(true) {
            (format!("失败：{r}"), "bad")
        } else {
            (r.to_owned(), "")
        };
    }
    match prompt {
        Some(p) => (format!("派了：{p}"), ""),
        None => ("还没有对话".into(), "muted"),
    }
}

struct Kid {
    ws: Memo<Option<WorkspaceView>>,
    shown: Memo<String>,
    line: Signal<(String, &'static str)>,
}

fn kid(app: AppData, id: String) -> Kid {
    let wid = StoredValue::new(id);
    let ws = Memo::new(move |_| app.workspace(&wid.get_value()));
    let shown = Memo::new(move |_| {
        ws.with(|w| {
            w.as_ref()
                .map(|w| app.shown_activity(w))
                .unwrap_or_default()
        })
    });
    let stamp = Memo::new(move |_| ws.with(|w| w.as_ref().map(|w| w.activity.clone())));
    let glance = LocalResource::new(move || {
        stamp.track();
        let id = wid.get_value();
        async move {
            api::get::<Value>(&format!("/api/workspaces/{}/glance", api::enc(&id)))
                .await
                .ok()
        }
    });
    let line = Signal::derive(move || {
        let g = glance.get().flatten();
        ws.with(|w| match (w, g) {
            (Some(w), Some(g)) => say(w, &g),
            _ => (String::new(), "muted"),
        })
    });
    Kid { ws, shown, line }
}

fn text_of(
    ws: Memo<Option<WorkspaceView>>,
    f: fn(&WorkspaceView) -> String,
) -> impl Fn() -> String + Copy {
    move || ws.with(|w| w.as_ref().map(f).unwrap_or_default())
}

fn card(app: AppData, id: String) -> impl IntoView {
    let href = format!("/w/{id}");
    let Kid { ws, shown, line } = kid(app, id);
    let name = text_of(ws, |w| fmt::ws_title(&w.name, &w.node));
    view! {
        <a class="fl-card" href=href data-act=move || shown.get()>
            <div class="top">
                <b class="nm" title=name>
                    {name}
                </b>
                <StateOf state=shown />
            </div>
            <div class="meta">
                <span class="ws-machine">{text_of(ws, |w| fmt::machine(&w.node))}</span>
                <span>
                    {text_of(ws, |w| w.last_active_at.as_deref().map(fmt::ago).unwrap_or_default())}
                </span>
                {move || ws.with(|w| w.as_ref().map(diff_badge))}
            </div>
            <p class=move || format!("fl-say {}", line.get().1)>{move || line.get().0}</p>
        </a>
    }
}

fn node(app: AppData, id: String) -> impl IntoView {
    let href = format!("/w/{id}");
    let Kid { ws, shown, line } = kid(app, id);
    let name = text_of(ws, |w| fmt::ws_title(&w.name, &w.node));
    view! {
        <a class="fl-node" href=href data-act=move || shown.get()>
            <div class="top">
                <b class="nm" title=name>
                    {name}
                </b>
                <StateOf state=shown />
            </div>
            <div class="meta">
                <span class="ws-machine">{text_of(ws, |w| fmt::machine(&w.node))}</span>
                <span class=move || {
                    format!("fl-say1 {}", line.get().1)
                }>{move || line.get().0}</span>
            </div>
        </a>
    }
}

fn edge(app: AppData, id: String, at: usize, height: Memo<usize>) -> impl IntoView {
    let shown = Memo::new(move |_| {
        app.workspace(&id)
            .map(|w| app.shown_activity(&w))
            .unwrap_or_default()
    });
    let d = move || {
        let from = height.get() as f64 / 2.0;
        let to = (at * (NODE_H + NODE_GAP) + NODE_H / 2) as f64;
        let mid = EDGE_W as f64 / 2.0;
        format!("M0 {from} C{mid} {from} {mid} {to} {EDGE_W} {to}")
    };
    view! { <path class="fl-edge" data-act=move || shown.get() d=d></path> }
}

fn graph(app: AppData, id: String, kids: Memo<Vec<WorkspaceView>>) -> impl IntoView {
    let me = Memo::new(move |_| app.workspace(&id));
    let mine = Memo::new(move |_| {
        me.with(|w| {
            w.as_ref()
                .map(|w| app.shown_activity(w))
                .unwrap_or_default()
        })
    });
    let height = Memo::new(move |_| {
        let n = kids.with(Vec::len).max(1);
        n * NODE_H + (n - 1) * NODE_GAP
    });
    let name = text_of(me, |w| fmt::ws_title(&w.name, &w.node));
    let edges = move || {
        kids.get()
            .into_iter()
            .enumerate()
            .map(|(at, w)| (at, w.id))
            .collect::<Vec<_>>()
    };
    view! {
        <div
            class="fl-graph"
            style=move || {
                format!(
                    "--fl-h: {}px; --fl-node: {NODE_H}px; --fl-gap: {NODE_GAP}px; --fl-edge: {EDGE_W}px",
                    height.get(),
                )
            }
        >
            <div class="fl-node fl-root" data-act=move || mine.get()>
                <div class="top">
                    <span class="fl-tag">"本工作区"</span>
                    <b class="nm" title=name>
                        {name}
                    </b>
                </div>
                <div class="meta">
                    <span class="ws-machine">{text_of(me, |w| fmt::machine(&w.node))}</span>
                    <StateOf state=mine />
                </div>
            </div>
            <svg
                class="fl-edges"
                aria-hidden="true"
                width=EDGE_W
                height=move || height.get()
                viewBox=move || format!("0 0 {EDGE_W} {}", height.get())
            >
                <For each=edges key=|(at, id)| (*at, id.clone()) let:item>
                    {edge(app, item.1, item.0, height)}
                </For>
            </svg>
            <div class="fl-kids">
                <For each=move || kids.get() key=|w| w.id.clone() let:w>
                    {node(app, w.id)}
                </For>
            </div>
        </div>
    }
}

#[component]
pub fn ChildrenPanel(id: String) -> impl IntoView {
    let app = use_app();
    let open = use_context::<ChildrenOpen>().map_or_else(|| children_open(&id), |c| c.0);
    let wid = StoredValue::new(id);
    let kids = Memo::new(move |_| app.children_of(&wid.get_value()));
    let look = RwSignal::new(if storage::load_raw(LOOK_KEY).as_deref() == Some("graph") {
        Look::Graph
    } else {
        Look::Cards
    });
    let pick = move |l: Look| {
        look.set(l);
        storage::save_raw(LOOK_KEY, if l == Look::Graph { "graph" } else { "cards" });
    };
    let shown = move || open.get() && kids.with(|k| !k.is_empty());
    view! {
        <Show when=shown>
            <section class="fl" aria-label="子工作区">
                <div class="fl-head">
                    <b>"子工作区"</b>
                    <span class="muted small">{move || kids.with(Vec::len)}</span>
                    <span class="grow"></span>
                    <div class="seg" role="group" aria-label="子工作区的显示方式">
                        <button
                            type="button"
                            data-on=move || (look.get() == Look::Cards).to_string()
                            aria-pressed=move || (look.get() == Look::Cards).to_string()
                            on:click=move |_| pick(Look::Cards)
                        >
                            "卡片"
                        </button>
                        <button
                            type="button"
                            data-on=move || (look.get() == Look::Graph).to_string()
                            aria-pressed=move || (look.get() == Look::Graph).to_string()
                            on:click=move |_| pick(Look::Graph)
                        >
                            "连线图"
                        </button>
                    </div>
                    <button
                        type="button"
                        class="btn small ghost"
                        on:click=move |_| add_child(app, wid.get_value())
                    >
                        "添加"
                    </button>
                    <button type="button" class="btn small ghost" on:click=move |_| open.set(false)>
                        "收起"
                    </button>
                </div>
                {move || match look.get() {
                    Look::Cards => {
                        view! {
                            <div class="fl-cards">
                                <div class="fl-track">
                                    <For each=move || kids.get() key=|w| w.id.clone() let:w>
                                        {card(app, w.id)}
                                    </For>
                                </div>
                            </div>
                        }
                            .into_any()
                    }
                    Look::Graph => graph(app, wid.get_value(), kids).into_any(),
                }}
            </section>
        </Show>
    }
}
