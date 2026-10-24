mod inspector;
mod start;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use serde_json::Value;
use wasm_bindgen::JsCast;

use crate::action_gate::ActionGate;
use crate::api;
use crate::app_state::use_app;
use crate::components::empty::{self, Blank};
use crate::components::group::ListGroup;
use crate::components::icon;
use crate::components::page_head::PageHead;
use crate::components::prefs::{PrefGroup, PrefRow};
use crate::components::rows::{MenuAt, RowAction, RowMenu};
use crate::components::status::{InlineError, LoadingState, StateOf, Status, Tone};
use crate::components::undo;
use crate::fmt;
use crate::home_rows::{self, Group, Row, StepText};

use super::work_shared::{
    allow_always, approval_key_hint, approval_summary, decide_approval, list_keys_apply,
};
pub use inspector::{
    Inspector, InspectorHost, remove_workspace, rename_workspace, retry_workspace,
};
use start::Start;

pub fn diff_badge(w: &blazar_core_types::api::WorkspaceView) -> AnyView {
    match w.diff {
        None => view! { <span class="muted">"—"</span> }.into_any(),
        Some(d) if d.files == 0 => view! { <span class="muted">"无改动"</span> }.into_any(),
        Some(d) => view! {
            <span class="diffst" title=format!("{} 个文件", d.files)>
                <i class="ok">{format!("+{}", d.added)}</i>
                " "
                <i class="bad">{format!("−{}", d.removed)}</i>
            </span>
        }
        .into_any(),
    }
}

#[component]
pub fn WorkspacesPage() -> impl IntoView {
    view! { <Home /> }
}

#[component]
pub fn ProjectPage() -> impl IntoView {
    let params = use_params_map();
    let project = Signal::derive(move || {
        params
            .read()
            .get("name")
            .map(|n| {
                js_sys::decode_uri_component(&n)
                    .map(String::from)
                    .unwrap_or(n)
            })
            .filter(|n| !n.is_empty())
    });
    view! { <ProjectDetail project /> }
}

#[component]
fn ProjectDetail(project: Signal<Option<String>>) -> impl IntoView {
    let app = use_app();
    let title = Signal::derive(move || project.get().unwrap_or_else(|| "项目".to_owned()));
    let members = Memo::new(move |_| {
        let p = project.get();
        let mut list = app
            .top_workspaces()
            .into_iter()
            .filter(|w| {
                p.as_deref()
                    .is_some_and(|p| w.project.as_deref() == Some(p))
            })
            .collect::<Vec<_>>();
        list.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
        list
    });
    let machines = Memo::new(move |_| {
        let mut m = members
            .get()
            .into_iter()
            .map(|w| fmt::machine(&w.node))
            .collect::<Vec<_>>();
        m.sort();
        m.dedup();
        m
    });
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
                                    label=format!("{} 个工作区", members.get().len())
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
            {move || {
                app.state_error
                    .get()
                    .map(|message| {
                        view! {
                            <InlineError message retry=Callback::new(move |_| app.load_state()) />
                        }
                    })
            }}
            <div class="detail-body">
                <PrefGroup title="工作区">
                    {move || {
                        if app.state.with(Option::is_none) {
                            return view! { <LoadingState text="加载工作区…" /> }.into_any();
                        }
                        let list = members.get();
                        if list.is_empty() {
                            return view! {
                                <PrefRow
                                    label="还没有工作区"
                                    detail="新建工作区时填上这个项目名，就会归到这里。"
                                />
                            }
                                .into_any();
                        }
                        list.into_iter()
                            .map(|w| {
                                let activity = app.shown_activity(&w);
                                view! {
                                    <PrefRow
                                        label=fmt::ws_title(&w.name, &w.node)
                                        detail=format!("{}:{}", fmt::machine(&w.node), w.path)
                                    >
                                        <StateOf state=activity />
                                        <a class="btn" href=format!("/w/{}", w.id)>
                                            "打开"
                                        </a>
                                    </PrefRow>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </PrefGroup>
                <PrefGroup title="项目" note="同名项目的工作区归为一组，在侧栏里可以直接跳到。">
                    <PrefRow label="名称">
                        <span class="pref-text">{move || title.get()}</span>
                    </PrefRow>
                    <PrefRow label="机器">
                        <span class="pref-text">
                            {move || {
                                let m = machines.get();
                                if m.is_empty() { "—".to_owned() } else { m.join("、") }
                            }}
                        </span>
                    </PrefRow>
                </PrefGroup>
            </div>
        </div>
    }
}

#[derive(Clone, Copy)]
struct Ctx {
    sel: RwSignal<Option<usize>>,
    menu: RwSignal<Option<MenuAt>>,
    gate: RwSignal<ActionGate>,
    appr_rev: RwSignal<u32>,
}

fn decide(ctx: Ctx, id: String, allow: bool) {
    decide_approval(ctx.gate, id, allow, None, move || {
        let _ = ctx.appr_rev.try_update(|n| *n += 1);
    });
}

fn always(ctx: Ctx, a: &Value, workspace: String) {
    allow_always(
        ctx.gate,
        a["id"].as_str().unwrap_or("").to_owned(),
        a["request"].clone(),
        workspace,
        move || {
            let _ = ctx.appr_rev.try_update(|n| *n += 1);
        },
    );
}

fn pick_approval(ctx: Ctx, key: &str, a: &Value, workspace: String) {
    let id = a["id"].as_str().unwrap_or("").to_owned();
    match key {
        "1" => decide(ctx, id, true),
        "2" => always(ctx, a, workspace),
        _ => decide(ctx, id, false),
    }
}

fn is_question(a: &Value) -> bool {
    crate::chat_model::tool_name(a["request"]["tool_name"].as_str().unwrap_or(""))
        == "AskUserQuestion"
}

fn new_keys() -> Option<AnyView> {
    crate::desktop::available().then(|| {
        view! { <kbd>{crate::shortcuts::label(&crate::shortcuts::current("newws"))}</kbd> }
            .into_any()
    })
}

#[component]
fn Home() -> impl IntoView {
    let app = use_app();
    let ctx = Ctx {
        sel: RwSignal::new(None),
        menu: RwSignal::new(None),
        gate: RwSignal::new(ActionGate::default()),
        appr_rev: RwSignal::new(0),
    };
    let list_el = NodeRef::<html::Div>::new();
    let rows = Memo::new(move |_| {
        let all = app.workspaces();
        let top = app
            .top_workspaces()
            .into_iter()
            .filter(|w| undo::shows(&w.id))
            .collect();
        home_rows::group_rows(top, &all, |w| app.shown_activity(w))
    });
    let approvals = LocalResource::new(move || {
        ctx.appr_rev.track();
        let need = rows.with(|r| r.iter().any(|row| row.state == "awaiting_approval"));
        async move {
            if !need {
                return Vec::<Value>::new();
            }
            api::get::<Vec<Value>>("/api/approvals")
                .await
                .unwrap_or_default()
        }
    });
    let pending_of = move |row: &Row| -> Option<Value> {
        let id = row.subject().id.clone();
        approvals.get().and_then(|list| {
            list.iter()
                .find(|a| a["workspace_id"].as_str() == Some(id.as_str()))
                .cloned()
        })
    };
    Effect::new(move |_| {
        let id = ctx
            .sel
            .get()
            .and_then(|i| rows.with(|r| r.get(i).map(|row| row.ws.id.clone())));
        app.selected_ws.set(id);
    });
    on_cleanup(move || app.selected_ws.set(None));
    let mode = Memo::new(move |_| {
        if app.state.with(Option::is_none) {
            "loading"
        } else if app.top_workspaces().is_empty() {
            "start"
        } else {
            "list"
        }
    });
    let focus_row = move |i: usize| {
        if let Some(list) = list_el.try_get_untracked().flatten()
            && let Ok(Some(a)) =
                list.query_selector(&format!(".home-row[data-index=\"{i}\"] > .home-main"))
            && let Ok(a) = a.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = a.focus();
        }
    };
    let keys = window_event_listener(ev::keydown, move |e| {
        if !list_keys_apply(&e) {
            return;
        }
        let n = rows.try_with_untracked(|r| r.len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        let meta = e.meta_key() || e.ctrl_key();
        let key = e.key();
        let current = ctx.sel.try_get_untracked().flatten();
        let selected =
            move || current.and_then(|i| rows.try_with_untracked(|r| r.get(i).cloned()).flatten());
        match key.as_str() {
            "ArrowDown" | "ArrowUp" if !meta => {
                e.prevent_default();
                let down = key == "ArrowDown";
                let next = match current {
                    None if down => 0,
                    None => n - 1,
                    Some(i) if down => (i + 1).min(n - 1),
                    Some(i) => i.saturating_sub(1),
                };
                ctx.sel.set(Some(next));
                focus_row(next);
            }
            "1" | "2" | "3" if !meta && !e.alt_key() && !e.shift_key() => {
                let Some(row) = selected() else {
                    return;
                };
                let Some(a) = pending_of(&row).filter(|a| !is_question(a)) else {
                    return;
                };
                e.prevent_default();
                pick_approval(ctx, &key, &a, row.subject().id.clone());
            }
            _ => match crate::shortcuts::action(&e) {
                Some("inspect") => {
                    if let Some(row) = selected() {
                        e.prevent_default();
                        app.inspect_ws.set(Some(row.ws.id.clone()));
                    }
                }
                Some("delete") => {
                    if let Some(row) = selected() {
                        e.prevent_default();
                        remove_workspace(app, row.ws.id.clone());
                    }
                }
                _ => {}
            },
        }
    });
    on_cleanup(move || keys.remove());
    let groups = move || {
        let r = rows.get();
        if r.is_empty() {
            return view! {
                <Blank
                    icon=icon::FOLDER
                    title=empty::never("工作区")
                    detail="新建一个工作区，智能体就能开始干活。"
                />
            }
            .into_any();
        }
        Group::ALL
            .iter()
            .filter_map(|g| {
                let members: Vec<(usize, Row)> = r
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.group == *g)
                    .map(|(i, row)| (i, row.clone()))
                    .collect();
                (!members.is_empty()).then(|| {
                    let count = members.len();
                    let rows = members
                        .into_iter()
                        .map(|(i, row)| {
                            let pending = pending_of(&row);
                            row_view(ctx, i, row, pending)
                        })
                        .collect_view();
                    view! {
                        <ListGroup title=g.title() count key=g.key() class="home-group">
                            {rows}
                        </ListGroup>
                    }
                })
            })
            .collect_view()
            .into_any()
    };
    view! {
        <div class="page home-page" class:home-start=move || mode.get() == "start">
            <Show when=move || mode.get() != "start">
                <PageHead title="工作区">
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
            </Show>
            {move || {
                app.state_error
                    .get()
                    .map(|message| {
                        view! {
                            <InlineError message retry=Callback::new(move |_| app.load_state()) />
                        }
                    })
            }}
            <div class="home-list" node_ref=list_el>
                {move || match mode.get() {
                    "loading" => {
                        if app.state_error.with(Option::is_some) {
                            ().into_any()
                        } else {
                            view! { <LoadingState text="加载工作区…" /> }.into_any()
                        }
                    }
                    "start" => view! { <Start /> }.into_any(),
                    _ => groups(),
                }}
            </div>
            {move || {
                ctx.menu
                    .get()
                    .and_then(|at| {
                        rows.with(|r| r.get(at.index).cloned())
                            .map(|row| {
                                let pending = pending_of(&row);
                                view! {
                                    <RowMenu
                                        at
                                        label=format!("{} 的操作", fmt::ws_title(&row.ws.name, &row.ws.node))
                                        groups=home_menu(ctx, &row, pending)
                                        on_close=Callback::new(move |()| ctx.menu.set(None))
                                    />
                                }
                            })
                    })
            }}
        </div>
    }
}

fn row_view(ctx: Ctx, i: usize, row: Row, pending: Option<Value>) -> impl IntoView {
    let app = use_app();
    let subject = row.subject().clone();
    let unseen = app.unseen(&row.ws) || row.child.as_ref().is_some_and(|c| app.unseen(c));
    let href = format!("/w/{}", subject.id);
    let title = fmt::ws_title(&row.ws.name, &row.ws.node);
    let child_title = row.child.as_ref().map(|c| fmt::ws_title(&c.name, &c.node));
    let tone = crate::vocab::state(&row.state).1.key();
    let (step, pending_id, ask) = match &pending {
        Some(a) => {
            let (label, code) = approval_summary(&a["request"], &subject.path);
            (
                StepText { label, code },
                Some(a["id"].as_str().unwrap_or("").to_owned()),
                is_question(a),
            )
        }
        None => (
            home_rows::step_text(subject.step.as_ref(), &row.state, &subject.path),
            None,
            false,
        ),
    };
    let diff = subject.diff.filter(|d| d.files > 0);
    let when = subject
        .last_active_at
        .as_deref()
        .map(fmt::ago)
        .unwrap_or_default();
    let machine = (subject.node != "local").then(|| subject.node.clone());
    let full = if step.code.is_empty() {
        step.label.clone()
    } else {
        format!("{} {}", step.label, step.code)
    };
    let location = format!("{}:{}", fmt::machine(&subject.node), subject.path);
    let open_href = href.clone();
    let actions = pending_id.map(|id| {
        if ask {
            view! {
                <div class="home-act">
                    <a class="btn small" href=open_href>
                        "回答"
                    </a>
                </div>
            }
            .into_any()
        } else {
            let (i1, i2, i3, i4) = (id.clone(), id.clone(), id.clone(), id);
            view! {
                <div class="home-act" data-ui-guard="">
                    <button
                        type="button"
                        class="btn small"
                        disabled=move || ctx.gate.with(|g| g.busy(&i1))
                        on:click=move |_| decide(ctx, i2.clone(), true)
                    >
                        "允许"
                        <kbd>{approval_key_hint(0)}</kbd>
                    </button>
                    <button
                        type="button"
                        class="btn small"
                        disabled=move || ctx.gate.with(|g| g.busy(&i3))
                        on:click=move |_| decide(ctx, i4.clone(), false)
                    >
                        "拒绝"
                        <kbd>{approval_key_hint(2)}</kbd>
                    </button>
                </div>
            }
            .into_any()
        }
    });
    view! {
        <div
            class="home-row lrow"
            data-index=i.to_string()
            data-state=row.state.clone()
            data-sel=move || (ctx.sel.get() == Some(i)).to_string()
            data-menu=move || {
                ctx.menu.with(|m| m.as_ref().is_some_and(|m| m.index == i)).to_string()
            }
            data-unseen=unseen.to_string()
            on:focusin=move |_| ctx.sel.set(Some(i))
            on:contextmenu=move |e| {
                e.prevent_default();
                ctx.sel.set(Some(i));
                ctx.menu.set(Some(MenuAt::from(&e, i)));
            }
        >
            <a class="home-main" href=href title=location>
                <span class="dot" data-tone=tone aria-hidden="true"></span>
                <span class="home-name">
                    {title}
                    {child_title
                        .map(|c| view! { <span class="home-child">{format!(" ↳ {c}")}</span> })}
                </span>
                {row.ws.project.clone().map(|p| view! { <span class="home-project">{p}</span> })}
                {machine.map(|m| view! { <span class="home-machine">{m}</span> })}
                <span class="home-step" title=full>
                    {(!step.label.is_empty())
                        .then(|| view! { <span class="home-step-label">{step.label.clone()}</span> })}
                    {(!step.code.is_empty()).then(|| view! { <code>{step.code.clone()}</code> })}
                </span>
                {diff
                    .map(|d| {
                        view! {
                            <span class="home-diff" title=format!("{} 个文件", d.files)>
                                <i class="ok">{format!("+{}", d.added)}</i>
                                <i class="bad">{format!("−{}", d.removed)}</i>
                            </span>
                        }
                    })}
                <span class="home-time">{when}</span>
            </a>
            {actions}
        </div>
    }
}

fn home_menu(ctx: Ctx, row: &Row, pending: Option<Value>) -> Vec<Vec<RowAction>> {
    let app = use_app();
    let id = row.ws.id.clone();
    let href = format!("/w/{}", row.subject().id);
    let approval = pending.filter(|a| !is_question(a));
    let subject = row.subject().id.clone();
    let failed = (row.state == "errored")
        .then(|| row.subject().session_id.clone())
        .flatten()
        .map(|session| (row.subject().id.clone(), session));
    let (id_detail, id_rename, id_remove) = (id.clone(), id.clone(), id);
    vec![
        failed
            .map(|(ws, session)| {
                vec![RowAction::new("重试", move || {
                    retry_workspace(app, ws.clone(), session.clone())
                })]
            })
            .unwrap_or_default(),
        approval
            .map(|a| {
                let (a1, a2, a3) = (a.clone(), a.clone(), a);
                let (s1, s2, s3) = (subject.clone(), subject.clone(), subject);
                vec![
                    RowAction::new("允许", move || pick_approval(ctx, "1", &a1, s1.clone()))
                        .hint(approval_key_hint(0)),
                    RowAction::new("总是允许", move || {
                        pick_approval(ctx, "2", &a2, s2.clone())
                    })
                    .hint(approval_key_hint(1)),
                    RowAction::new("拒绝", move || pick_approval(ctx, "3", &a3, s3.clone()))
                        .hint(approval_key_hint(2)),
                ]
            })
            .unwrap_or_default(),
        vec![
            RowAction::open(href),
            RowAction::inspect(move || app.inspect_ws.set(Some(id_detail.clone()))),
            RowAction::new("改名…", move || {
                rename_workspace(app, id_rename.clone())
            }),
        ],
        vec![RowAction::delete("移除…", move || {
            remove_workspace(app, id_remove.clone())
        })],
    ]
}
