use blazar_core_types::api::{Board, BoardNote, BoardRow};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app_state::{AppData, use_app};
use crate::board_rows::{self, Placed, Tally};
use crate::components::icon;
use crate::components::page_head::PageHead;
use crate::components::prefs::{PrefGroup, PrefRow};
use crate::components::status::{InlineError, LoadingState, StateOf, Status, Tone};
use crate::fmt;

use super::new_keys;

fn live(app: AppData, row: &BoardRow) -> String {
    app.workspace(&row.id)
        .map_or_else(|| row.activity.clone(), |w| app.shown_activity(&w))
}

fn row_view(app: AppData, row: BoardRow, depth: usize, main: &str) -> impl IntoView + use<> {
    let elsewhere = (row.node != main).then(|| fmt::machine(&row.node));
    let href = format!("/w/{}", row.id);
    let state = {
        let row = row.clone();
        Signal::derive(move || live(app, &row))
    };
    let running = move || matches!(state.get().as_str(), "running" | "starting");
    let waiting = row.waiting;
    let wait_link = (waiting > 0).then(|| {
        view! {
            <a class="board-wait" href=href.clone()>
                {format!("{waiting} 张卡片等你")}
            </a>
        }
    });
    let progress = row.progress.clone().map(|p| {
        let pct = board_rows::percent(&p);
        view! {
            <span class="board-bar" title=p.title.clone()>
                <i style=format!("width:{pct}%")></i>
            </span>
            <span class="board-frac">{board_rows::fraction(&p)}</span>
        }
    });
    view! {
        <div class="board-row" style=format!("--depth:{depth}")>
            <div class="board-main">
                <div class="board-line">
                    <StateOf state />
                    <a class="board-name" href=href.clone()>
                        {fmt::ws_title(&row.name, &row.node)}
                    </a>
                    {elsewhere.map(|m| view! { <span class="board-node">{m}</span> })}
                </div>
                {row
                    .doing
                    .clone()
                    .map(|d| {
                        view! {
                            <div class="board-doing">
                                <span class="board-tag">
                                    {move || if running() { "在做" } else { "上次" }}
                                </span>
                                {d}
                            </div>
                        }
                    })}
                {row.latest.clone().map(|l| view! { <div class="board-latest">{l}</div> })}
            </div>
            <div class="board-progress">{progress}</div>
            <div class="board-meta">
                {wait_link}
                <span class="board-when">
                    {row.updated_at.as_deref().map(fmt::ago).unwrap_or_default()}
                </span>
            </div>
            <a class="btn" href=href>
                "打开"
            </a>
        </div>
    }
}

fn note_view(n: BoardNote) -> impl IntoView {
    let meta = format!(
        "{} · {}",
        fmt::ws_title(&n.workspace, &n.node),
        fmt::ago(&n.at)
    );
    view! {
        <a class="board-note" href=format!("/w/{}", n.workspace_id)>
            <div class="board-note-head">
                <span class="board-note-title">{n.title.clone()}</span>
                {n
                    .progress
                    .as_ref()
                    .map(|p| view! { <span class="board-frac">{board_rows::fraction(p)}</span> })}
            </div>
            <div class="board-note-meta">{meta}</div>
            {(!n.body.trim().is_empty())
                .then(|| view! { <div class="board-note-body">{n.body.clone()}</div> })}
        </a>
    }
}

fn rows_view(app: AppData, rows: Placed, main: &str) -> impl IntoView + use<> {
    rows.into_iter()
        .map(|(r, d)| row_view(app, r, d, main))
        .collect_view()
}

#[component]
pub fn ProjectDetail(project: Signal<Option<String>>) -> impl IntoView {
    let app = use_app();
    let title = Signal::derive(move || project.get().unwrap_or_else(|| "项目".to_owned()));
    let board = RwSignal::new(None::<Result<Board, String>>);
    let seq = StoredValue::new(0u32);
    let fold_open = RwSignal::new(false);
    let load = move || {
        let Some(name) = project.get_untracked() else {
            return;
        };
        seq.update_value(|s| *s += 1);
        let mine = seq.get_value();
        spawn_local(async move {
            let r = api::get::<Board>(&format!("/api/projects/board?name={}", api::enc(&name)))
                .await
                .map_err(|e| e.to_string());
            if seq.try_get_value() == Some(mine) {
                let _ = board.try_set(Some(r));
            }
        });
    };
    Effect::new(move |_| {
        project.track();
        app.state.track();
        load();
    });
    let arranged = Memo::new(move |_| {
        board.with(|b| match b {
            Some(Ok(b)) => board_rows::arrange(&b.rows),
            _ => (Vec::new(), Vec::new()),
        })
    });
    let tally = Memo::new(move |_| {
        let mut t = Tally::default();
        arranged.with(|(shown, _)| {
            for (r, _) in shown {
                t.add(&live(app, r), r.waiting);
            }
        });
        t
    });
    let machines = Memo::new(move |_| {
        let mut m: Vec<String> =
            arranged.with(|(shown, _)| shown.iter().map(|(r, _)| fmt::machine(&r.node)).collect());
        m.sort();
        m.dedup();
        m
    });
    let summary = move || {
        let t = tally.get();
        let names = machines.get();
        let tiles = [
            ("run", t.running, "运行中"),
            ("attention", t.waiting, "等你处理"),
            ("failed", t.failed, "失败"),
            ("other", t.idle, "空闲"),
        ]
        .into_iter()
        .map(|(tone, n, label)| {
            view! {
                <div class="board-stat" data-tone=tone>
                    <b>{n}</b>
                    <span>{label}</span>
                </div>
            }
        })
        .collect_view();
        view! {
            <div class="board-sum">
                {tiles} <div class="board-stat board-machines" title=names.join("、")>
                    <b>{names.len()}</b>
                    <span>"台机器"</span>
                    <small>{names.join("、")}</small>
                </div>
            </div>
        }
    };
    let workspaces = move || {
        let (shown, folded) = arranged.get();
        let main = board_rows::main_node(&shown);
        let list = if shown.is_empty() {
            view! {
                <PrefRow
                    label="还没有工作区"
                    detail="新建工作区时填上这个项目名，就会归到这里。"
                />
            }
            .into_any()
        } else {
            rows_view(app, shown, &main).into_any()
        };
        let stash = (!folded.is_empty()).then(|| {
            let n = folded.len();
            view! {
                <details
                    class="board-fold"
                    prop:open=move || fold_open.get_untracked()
                    on:toggle=move |e| {
                        let open = e
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlDetailsElement>().ok())
                            .is_some_and(|d| d.open());
                        fold_open.set(open);
                    }
                >
                    <summary>{format!("已归档和看图暂存（{n}）")}</summary>
                    {rows_view(app, folded, &main)}
                </details>
            }
        });
        view! {
            {list}
            {stash}
        }
    };
    let notes = move || {
        let recent = board.with(|b| match b {
            Some(Ok(b)) => b.recent.clone(),
            _ => Vec::new(),
        });
        if recent.is_empty() {
            return view! {
                <PrefRow
                    label="还没有汇报"
                    detail="智能体用 report 汇报之后，会按时间出现在这里。"
                />
            }
            .into_any();
        }
        recent.into_iter().map(note_view).collect_view().into_any()
    };
    view! {
        <div class="page project-page detail-page">
            <PageHead
                parent=("工作区", "/")
                icon=|| view! { <span class="ic" inner_html=icon::svg(icon::FOLDER)></span> }
                title
                status=move || {
                    view! {
                        {move || {
                            view! {
                                <Status
                                    tone=Tone::Other
                                    label=format!("{} 个工作区", arranged.with(|a| a.0.len()))
                                />
                            }
                        }}
                    }
                }
            >
                <button
                    type="button"
                    class="btn primary"
                    aria-haspopup="dialog"
                    on:click=move |_| app.new_ws.set(true)
                >
                    "新建"
                    {new_keys()}
                </button>
            </PageHead>
            <div class="detail-body">
                {move || match board.with(|b| b.as_ref().map(|r| r.as_ref().err().cloned())) {
                    None => view! { <LoadingState text="加载项目…" /> }.into_any(),
                    Some(Some(message)) => {
                        view! { <InlineError message retry=Callback::new(move |_| load()) /> }
                            .into_any()
                    }
                    Some(None) => {
                        view! {
                            {summary}
                            <PrefGroup title="工作区">{workspaces}</PrefGroup>
                            <PrefGroup title="最近进展">{notes}</PrefGroup>
                        }
                            .into_any()
                    }
                }}
            </div>
        </div>
    }
}
