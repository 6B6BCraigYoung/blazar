mod inspector;

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
use crate::components::icon;
use crate::components::menu::menu_keydown;
use crate::components::page_head::PageHead;
use crate::components::status::{InlineError, LoadingState, Status, Tone};
use crate::fmt;
use crate::home_rows::{self, Group, Row, StepText};

use super::work_shared::{approval_summary, decide_approval, list_keys_apply};
pub use inspector::{Inspector, InspectorHost, remove_workspace, rename_workspace};

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
    view! { <Home project=Signal::derive(|| None) /> }
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
    view! { <Home project /> }
}

#[derive(Clone, PartialEq)]
struct MenuAt {
    x: i32,
    y: i32,
    index: usize,
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
fn Home(project: Signal<Option<String>>) -> impl IntoView {
    let app = use_app();
    let ctx = Ctx {
        sel: RwSignal::new(None),
        menu: RwSignal::new(None),
        gate: RwSignal::new(ActionGate::default()),
        appr_rev: RwSignal::new(0),
    };
    let list_el = NodeRef::<html::Div>::new();
    let rows = Memo::new(move |_| {
        let p = project.get();
        let all = app.workspaces();
        let top = app
            .top_workspaces()
            .into_iter()
            .filter(|w| p.as_deref().is_none_or(|p| w.project.as_deref() == Some(p)))
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
        } else if project.get().is_none() && app.top_workspaces().is_empty() {
            "start"
        } else {
            "list"
        }
    });
    let focus_row = move |i: usize| {
        if let Some(list) = list_el.get_untracked()
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
        let n = rows.with_untracked(|r| r.len());
        if n == 0 {
            return;
        }
        let meta = e.meta_key() || e.ctrl_key();
        let key = e.key();
        match key.as_str() {
            "ArrowDown" | "ArrowUp" if !meta => {
                e.prevent_default();
                let down = key == "ArrowDown";
                let next = match ctx.sel.get_untracked() {
                    None if down => 0,
                    None => n - 1,
                    Some(i) if down => (i + 1).min(n - 1),
                    Some(i) => i.saturating_sub(1),
                };
                ctx.sel.set(Some(next));
                focus_row(next);
            }
            "Enter" | "Backspace" if meta => {
                let Some(i) = ctx.sel.get_untracked() else {
                    return;
                };
                let Some(a) = rows
                    .with_untracked(|r| r.get(i).cloned())
                    .and_then(|row| pending_of(&row))
                else {
                    return;
                };
                if is_question(&a) {
                    return;
                }
                e.prevent_default();
                decide(
                    ctx,
                    a["id"].as_str().unwrap_or("").to_owned(),
                    key == "Enter",
                );
            }
            _ => {}
        }
    });
    on_cleanup(move || keys.remove());
    let title = Signal::derive(move || project.get().unwrap_or_else(|| "工作区".to_owned()));
    let groups = move || {
        let r = rows.get();
        if r.is_empty() {
            return view! {
                <Blank
                    icon=icon::FOLDER
                    title=empty::never("工作区")
                    detail="这个项目里还没有工作区。"
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
                    view! {
                        <section class="home-group" data-group=g.key() aria-label=g.title()>
                            <h2 class="home-group-title">
                                {g.title()}
                                <span class="n">{members.len()}</span>
                            </h2>
                            <div class="home-rows">
                                {members
                                    .into_iter()
                                    .map(|(i, row)| {
                                        let pending = pending_of(&row);
                                        row_view(ctx, i, row, pending)
                                    })
                                    .collect_view()}
                            </div>
                        </section>
                    }
                })
            })
            .collect_view()
            .into_any()
    };
    view! {
        <div class="page home-page" class:home-start=move || mode.get() == "start">
            <Show when=move || mode.get() != "start">
                <PageHead
                    title
                    status=move || {
                        project
                            .get()
                            .map(|_| {
                                let n = rows.with(Vec::len);
                                view! { <Status tone=Tone::Other label=format!("{n} 个工作区") /> }
                            })
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
                    "start" => {
                        view! {
                            <Blank
                                icon=icon::FOLDER
                                title=empty::never("工作区")
                                detail="工作区是一个代码目录，智能体在里面读代码、改代码、跑命令。"
                            />
                        }
                            .into_any()
                    }
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
                                view! { <RowMenu ctx at row pending /> }
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
                        class="btn small primary"
                        disabled=move || ctx.gate.with(|g| g.busy(&i1))
                        on:click=move |_| decide(ctx, i2.clone(), true)
                    >
                        "允许"
                        <kbd>{crate::shortcuts::label("Mod+Enter")}</kbd>
                    </button>
                    <button
                        type="button"
                        class="btn small"
                        disabled=move || ctx.gate.with(|g| g.busy(&i3))
                        on:click=move |_| decide(ctx, i4.clone(), false)
                    >
                        "拒绝"
                    </button>
                </div>
            }
            .into_any()
        }
    });
    view! {
        <div
            class="home-row"
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
                ctx.menu
                    .set(
                        Some(MenuAt {
                            x: e.client_x(),
                            y: e.client_y(),
                            index: i,
                        }),
                    );
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

#[component]
fn RowMenu(ctx: Ctx, at: MenuAt, row: Row, pending: Option<Value>) -> impl IntoView {
    let app = use_app();
    let root = NodeRef::<html::Div>::new();
    let first = NodeRef::<html::A>::new();
    let close = move || ctx.menu.set(None);
    let close_down = window_event_listener(ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.closest(".ctx-menu").ok().flatten())
            .is_some();
        if !inside {
            close();
        }
    });
    let close_blur = window_event_listener(ev::blur, move |_| close());
    on_cleanup(move || {
        close_down.remove();
        close_blur.remove();
    });
    request_animation_frame(move || {
        if let Some(b) = first.try_get_untracked().flatten() {
            let _ = b.focus();
        }
    });
    let w = window()
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let h = window()
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let (x, y) = (at.x.min(w - 200).max(0), at.y.min(h - 190).max(0));
    let id = row.ws.id.clone();
    let href = format!("/w/{}", row.subject().id);
    let approval = pending
        .filter(|a| !is_question(a))
        .map(|a| a["id"].as_str().unwrap_or("").to_owned());
    let (id_detail, id_rename, id_remove) = (id.clone(), id.clone(), id);
    view! {
        <div
            class="ctx-menu home-menu"
            role="menu"
            aria-label=format!("{} 的操作", fmt::ws_title(&row.ws.name, &row.ws.node))
            node_ref=root
            style=format!("left:{x}px;top:{y}px")
            on:keydown=move |e| {
                if let Some(r) = root.get_untracked() {
                    menu_keydown(&e, r.unchecked_ref(), Callback::new(move |_| close()));
                }
            }
        >
            {approval
                .map(|aid| {
                    let (a1, a2) = (aid.clone(), aid);
                    view! {
                        <button
                            type="button"
                            role="menuitem"
                            class="ctx-item"
                            on:click=move |_| {
                                close();
                                decide(ctx, a1.clone(), true);
                            }
                        >
                            "允许"
                        </button>
                        <button
                            type="button"
                            role="menuitem"
                            class="ctx-item"
                            on:click=move |_| {
                                close();
                                decide(ctx, a2.clone(), false);
                            }
                        >
                            "拒绝"
                        </button>
                        <div class="menu-sep"></div>
                    }
                })}
            <a role="menuitem" class="ctx-item" href=href node_ref=first on:click=move |_| close()>
                "打开"
            </a>
            <button
                type="button"
                role="menuitem"
                class="ctx-item"
                on:click=move |_| {
                    close();
                    app.inspect_ws.set(Some(id_detail.clone()));
                }
            >
                "详情"
            </button>
            <button
                type="button"
                role="menuitem"
                class="ctx-item"
                on:click=move |_| {
                    close();
                    rename_workspace(app, id_rename.clone());
                }
            >
                "改名…"
            </button>
            <div class="menu-sep"></div>
            <button
                type="button"
                role="menuitem"
                class="ctx-item danger"
                on:click=move |_| {
                    close();
                    remove_workspace(app, id_remove.clone());
                }
            >
                "移除…"
            </button>
        </div>
    }
}
