use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::diff_panel::DiffState;
use super::git_panel::Git;
use super::layout::{Changes, LayoutState, Tab};
use crate::api::{self, WorkspaceDetail};
use crate::app_state::use_app;
use crate::components::menu::menu_keydown;
use crate::components::status::{InlineError, Tone};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Jump {
    Bottom,
    Approval,
    Error,
}

fn query(sel: &str) -> Option<web_sys::Element> {
    document().query_selector(sel).ok().flatten()
}

fn last(sel: &str) -> Option<web_sys::Element> {
    let list = document().query_selector_all(sel).ok()?;
    list.item(list.length().checked_sub(1)?)?
        .dyn_into::<web_sys::Element>()
        .ok()
}

fn reveal(el: &web_sys::Element) {
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_behavior(web_sys::ScrollBehavior::Smooth);
    opts.set_block(web_sys::ScrollLogicalPosition::Center);
    el.scroll_into_view_with_scroll_into_view_options(&opts);
}

fn jump(to: Jump) {
    match to {
        Jump::Bottom => {
            if let Some(log) = query(".ws .cc-log") {
                let opts = web_sys::ScrollToOptions::new();
                opts.set_top(f64::from(log.scroll_height()));
                opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                log.scroll_to_with_scroll_to_options(&opts);
            }
        }
        Jump::Approval => {
            if let Some(card) = query(".ws .cc-appr") {
                reveal(&card);
                if let Some(b) = card
                    .query_selector("button:not([disabled])")
                    .ok()
                    .flatten()
                    .and_then(|b| b.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = b.focus();
                }
            }
        }
        Jump::Error => {
            if let Some(e) = query(".ws-head .ui-error")
                .or_else(|| query(".ws-error"))
                .or_else(|| last(".ws .cc-log .cc-err"))
            {
                reveal(&e);
            }
        }
    }
}

#[component]
fn WsStatus(id: String, failed: Signal<bool>) -> impl IntoView {
    let app = use_app();
    let shown = Memo::new(move |_| {
        let w = app.workspace(&id);
        let offline = w.as_ref().is_some_and(|w| app.node_unreachable(&w.node));
        if failed.get() || offline {
            return ("连不上".to_owned(), Tone::Failed, Some(Jump::Error));
        }
        let (label, tone) = w
            .as_ref()
            .map(|w| crate::home_rows::status(&w.activity, w.step.as_ref()))
            .unwrap_or_else(|| crate::home_rows::status("", None));
        let jump = match tone {
            Tone::Run => Some(Jump::Bottom),
            Tone::Attention => Some(Jump::Approval),
            Tone::Failed => Some(Jump::Error),
            Tone::Other => None,
        };
        (label, tone, jump)
    });
    view! {
        {move || {
            let (label, tone, to) = shown.get();
            match to {
                Some(to) => {
                    view! {
                        <button
                            type="button"
                            class="status ws-status"
                            data-tone=tone.key()
                            title=match to {
                                Jump::Bottom => "跳到对话底部",
                                Jump::Approval => "跳到审批",
                                Jump::Error => "跳到报错",
                            }
                            on:click=move |_| jump(to)
                        >
                            {label}
                        </button>
                    }
                        .into_any()
                }
                None => {
                    view! { <span class="status" data-tone=tone.key()>{label}</span> }.into_any()
                }
            }
        }}
    }
}

#[derive(Clone)]
struct Item {
    label: Signal<String>,
    keys: Option<&'static str>,
    run: Callback<()>,
}

fn item(
    label: &'static str,
    keys: Option<&'static str>,
    run: impl Fn() + Send + Sync + 'static,
) -> Item {
    Item {
        label: Signal::derive(move || label.to_owned()),
        keys,
        run: Callback::new(move |()| run()),
    }
}

#[component]
fn WsMenu(groups: Vec<Vec<Item>>) -> impl IntoView {
    let open = RwSignal::new(false);
    let root = NodeRef::<html::Div>::new();
    let button = NodeRef::<html::Button>::new();
    let close = Callback::new(move |()| {
        open.set(false);
        if let Some(b) = button.get_untracked() {
            let _ = b.focus();
        }
    });
    let groups = StoredValue::new(groups);
    view! {
        <span
            class="more"
            on:focusout=move |e| {
                let inside = e
                    .related_target()
                    .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                    .zip(e.current_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()))
                    .is_some_and(|(t, me)| me.contains(Some(&t)));
                if !inside {
                    open.set(false);
                }
            }
        >
            <button
                type="button"
                class="btn ghost more-btn"
                node_ref=button
                aria-label="更多"
                title="更多"
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)
            >
                "⋯"
            </button>
            <Show when=move || open.get()>
                <div
                    class="menu ws-menu"
                    node_ref=root
                    role="menu"
                    aria-label="工作区"
                    on:keydown=move |event| {
                        if let Some(r) = root.get_untracked() {
                            menu_keydown(&event, r.unchecked_ref(), close);
                        }
                    }
                >
                    {groups
                        .get_value()
                        .into_iter()
                        .enumerate()
                        .map(|(i, g)| {
                            view! {
                                {(i > 0).then(|| view! { <div class="menu-sep"></div> })}
                                {g
                                    .into_iter()
                                    .map(|it| {
                                        let hint = it
                                            .keys
                                            .map(|k| {
                                                crate::shortcuts::label(
                                                    &crate::shortcuts::current(k),
                                                )
                                            })
                                            .filter(|k| !k.is_empty());
                                        view! {
                                            <button
                                                type="button"
                                                role="menuitem"
                                                on:click=move |_| {
                                                    open.set(false);
                                                    it.run.run(());
                                                }
                                            >
                                                <span>{move || it.label.get()}</span>
                                                {hint.map(|k| view! { <kbd>{k}</kbd> })}
                                            </button>
                                        }
                                    })
                                    .collect_view()}
                            }
                        })
                        .collect_view()}
                </div>
            </Show>
        </span>
    }
}

#[component]
pub fn WsHead(
    id: String,
    detail: LocalResource<Result<WorkspaceDetail, api::ApiError>>,
    state: LayoutState,
    diff: DiffState,
    git: Git,
    props: RwSignal<bool>,
    kids_open: RwSignal<bool>,
    gone: Memo<bool>,
) -> impl IntoView {
    let app = use_app();
    let wid = StoredValue::new(id.clone());
    let lay = move || state.lay.get();
    let error = Memo::new(move |_| {
        detail
            .get()
            .and_then(|d| d.err())
            .filter(|e| !e.not_found())
            .map(|e| e.to_string())
    });
    let failed = Signal::derive(move || error.with(Option::is_some));
    let branch = move || {
        git.status
            .with(|g| {
                g.as_ref()
                    .filter(|g| g.repo && !g.detached)
                    .map(|g| g.branch.clone())
            })
            .or_else(|| detail.get().and_then(Result::ok).and_then(|d| d.branch))
            .filter(|b| !b.is_empty())
    };
    let title = move || {
        app.workspace(&wid.get_value())
            .map(|w| crate::fmt::ws_title(&w.name, &w.node))
            .unwrap_or_default()
    };
    let sub = move || {
        let w = app.workspace(&wid.get_value());
        let machine = w
            .as_ref()
            .filter(|w| w.node != "local")
            .map(|w| crate::fmt::machine(&w.node));
        [branch(), machine]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let path = move || {
        app.workspace(&wid.get_value())
            .map(|w| w.path)
            .unwrap_or_default()
    };
    let parent = move || {
        app.workspace(&wid.get_value())
            .and_then(|w| w.parent)
            .and_then(|p| app.workspace(&p))
    };
    let kids = Memo::new(move |_| app.children_of(&wid.get_value()).len());
    let changes_on = move || {
        let l = lay();
        l.inspect && l.tab == Tab::Changes
    };
    let sums = move || {
        diff.files.with(|f| {
            (
                f.len(),
                f.iter().map(|x| x.added).sum::<u32>(),
                f.iter().map(|x| x.removed).sum::<u32>(),
            )
        })
    };
    let kids_label = Signal::derive(move || match kids.get() {
        0 => "添加子工作区…".to_owned(),
        n if kids_open.get() => format!("收起子工作区（{n}）"),
        n => format!("显示子工作区（{n}）"),
    });
    let groups = vec![
        vec![
            item("文件", Some("explorer"), move || state.toggle_files()),
            item("终端", Some("panel"), move || state.toggle_term()),
            item("检查器", Some("chat"), move || state.toggle_inspect()),
        ],
        vec![
            item("编辑器", None, move || state.open(Tab::Editor)),
            item("Git", Some("git"), move || state.open_changes(Changes::Git)),
            item("预览", Some("preview"), move || state.open(Tab::Preview)),
        ],
        vec![
            Item {
                label: kids_label,
                keys: None,
                run: Callback::new(move |()| {
                    if kids.get_untracked() == 0 {
                        super::children::add_child(app, wid.get_value());
                    } else {
                        kids_open.update(|o| *o = !*o);
                    }
                }),
            },
            item("属性…", None, move || props.set(true)),
        ],
    ];
    view! {
        <header class="ws-head" inert=move || gone.get()>
            <button
                type="button"
                class="laybtn"
                aria-label="文件"
                title=crate::shortcuts::titled("文件", "explorer")
                aria-pressed=move || lay().files_shown().to_string()
                on:click=move |_| state.toggle_files()
                inner_html=super::ICON_FILES
            ></button>
            <h1 class="ws-name" title=path>
                {title}
            </h1>
            {move || {
                let s = sub();
                (!s.is_empty()).then(|| view! { <span class="ws-sub">{s}</span> })
            }}
            {move || {
                parent()
                    .map(|p| {
                        view! {
                            <a class="ws-up" href=format!("/w/{}", p.id)>
                                {format!("属于「{}」", crate::fmt::ws_title(&p.name, &p.node))}
                            </a>
                        }
                    })
            }}
            <WsStatus id=id.clone() failed />
            {move || error.get().map(|e| view! { <InlineError message=e /> })}
            <span class="grow"></span>
            <button
                type="button"
                class="btn small ws-changes"
                aria-pressed=move || changes_on().to_string()
                title=crate::shortcuts::titled("更改", "diff")
                on:click=move |_| {
                    if changes_on() {
                        state.close();
                    } else {
                        state.open_changes(Changes::Diff);
                    }
                }
            >
                "更改"
                {move || {
                    let (n, a, r) = sums();
                    (n > 0)
                        .then(|| {
                            view! {
                                <span class="ws-delta">
                                    <b class="ok">{format!("+{a}")}</b>
                                    <b class="bad">{format!("−{r}")}</b>
                                </span>
                            }
                        })
                }}
            </button>
            <WsMenu groups />
        </header>
    }
}
