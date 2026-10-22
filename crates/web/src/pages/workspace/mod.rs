pub mod chat;
mod children;
mod diff_panel;
mod editor;
mod files;
mod git_panel;
mod layout;
mod preview_panel;
mod term_panel;
mod tree;

use std::collections::HashSet;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;

use crate::components::status::{EmptyState, InlineError};
use leptos_router::hooks::use_params_map;
use wasm_bindgen::JsCast;

use crate::api::{self, Tree, WorkspaceDetail};
use crate::realtime::use_bus;
use crate::storage;

use children::{Children, ChildrenOpen, children_open};
use diff_panel::{DiffBar, DiffState, DiffView};
pub(crate) use editor::md_lang;
use editor::{EdBar, MdView, Tabs};
use files::Files;
use git_panel::{Git, GitView};
use layout::{Edge, LayoutState, Region, Splitter};
use preview_panel::PreviewBar;
use term_panel::{TermPane, TermTabs, Terms};
use tree::FileTree;

pub(crate) const ICON_LEFT: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.1"><rect class="fill" x="2" y="3" width="4" height="10" fill="currentColor" stroke="none"/><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M6 2.5v11"/></svg>"#;
pub(crate) const ICON_BOTTOM: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.1"><rect class="fill" x="2" y="10" width="12" height="3.5" fill="currentColor" stroke="none"/><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M1.5 10h13"/></svg>"#;
pub(crate) const ICON_RIGHT: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.1"><rect class="fill" x="10" y="3" width="4" height="10" fill="currentColor" stroke="none"/><rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M10 2.5v11"/></svg>"#;
pub(crate) const ICON_REFRESH: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 3v6h-6"/></svg>"#;

#[component]
pub fn WorkspacePage() -> impl IntoView {
    let app = crate::app_state::use_app();
    app.side_rail.set(true);
    on_cleanup(move || app.side_rail.set(false));
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();
    move || {
        let id = id();
        view! { <Workspace id /> }
    }
}

#[component]
fn Workspace(id: String) -> impl IntoView {
    let bus = use_bus();
    let ws_id = id.clone();
    let detail = LocalResource::new(move || {
        bus.workspaces.track();
        let id = ws_id.clone();
        async move { api::get::<WorkspaceDetail>(&format!("/api/workspaces/{id}/detail")).await }
    });

    let app = crate::app_state::use_app();
    app.current_ws.set(Some(id.clone()));
    app.mark_seen(&id);
    {
        let id = id.clone();
        on_cleanup(move || {
            app.current_ws.set(None);
            app.mark_seen(&id);
        });
    }
    let insp = RwSignal::new(false);
    let kids_open = children_open(&id);
    provide_context(ChildrenOpen(kids_open));

    let files = Files::new(id.clone());
    let dirty = wasm_bindgen::closure::Closure::<dyn Fn() -> u32>::new(move || {
        files
            .dirty
            .try_with_untracked(|d| d.len() as u32)
            .unwrap_or(0)
    });
    crate::files_js::set_navigation_guard(&id, dirty.as_ref().unchecked_ref());
    let guard = StoredValue::new_local(dirty);
    let guard_id = id.clone();
    on_cleanup(move || {
        crate::files_js::clear_navigation_guard(&guard_id);
        guard.dispose();
    });
    let state = LayoutState::new();
    let grid = NodeRef::<html::Div>::new();
    let host = NodeRef::<html::Div>::new();

    let tree_rev = RwSignal::new(0u32);
    let tid = id.clone();
    let tree = LocalResource::new(move || {
        bus.workspaces.track();
        tree_rev.track();
        let id = tid.clone();
        async move { api::get::<Tree>(&format!("/api/workspaces/{id}/tree")).await }
    });
    let root = Memo::new(move |_| {
        tree.get()
            .and_then(Result::ok)
            .map(|t| tree::build(&t.entries))
    });
    let opened = RwSignal::new(HashSet::<String>::new());
    let root_name = Signal::derive(move || {
        detail
            .get()
            .and_then(Result::ok)
            .map(|d| {
                d.path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "工作区".to_owned())
    });
    let tree_reload = Signal::derive(move || {
        bus.workspaces.track();
        tree_rev.get()
    });
    let expanded_once = StoredValue::new(false);
    Effect::new(move |_| {
        let changes = tree.with(|t| {
            t.as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|t| t.entries.iter().filter(|e| e.change.is_some()).count())
        });
        if let (Some(n), false) = (changes, expanded_once.get_value()) {
            expanded_once.set_value(true);
            if n > 0 && n <= 40 {
                root.with_untracked(|r| {
                    if let Some(r) = r {
                        opened.update(|o| tree::expand_changed(r, o));
                    }
                });
            }
        }
    });
    Effect::new(move |prev: Option<()>| {
        bus.workspaces.track();
        if prev.is_some()
            && let Some(cur) = files.current.get_untracked()
        {
            files.reload(cur, false);
        }
    });
    let changes = Memo::new(move |_| {
        tree.with(|t| {
            t.as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|t| {
                    t.entries
                        .iter()
                        .filter_map(|e| e.change.map(|c| (e.path.clone(), c)))
                        .collect::<std::collections::HashMap<_, _>>()
                })
                .unwrap_or_default()
        })
    });
    let tree_count = move || {
        tree.with(|t| {
            t.as_ref().and_then(|r| r.as_ref().ok()).map(|t| {
                let files_n = t.entries.iter().filter(|e| !e.is_dir).count();
                match t.stat {
                    Some(s) if s.files > 0 => format!("+{} −{}", s.added, s.removed),
                    _ if t.truncated => format!("{files_n}+ 个文件"),
                    _ => format!("{files_n} 个文件"),
                }
            })
        })
    };

    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        match host.get() {
            Some(el) => {
                files.mount(el.unchecked_into());
                true
            }
            None => false,
        }
    });
    on_cleanup(move || files.dispose());

    let cid = id.clone();
    leptos::task::spawn_local(async move {
        if let Ok(ctx) =
            api::get::<serde_json::Value>(&format!("/api/workspaces/{cid}/context")).await
            && let Some(f) = ctx["editor"]["file"].as_str()
            && files.current.try_get_untracked() == Some(None)
        {
            let line = ctx["editor"]["line"]
                .as_u64()
                .and_then(|l| u32::try_from(l).ok())
                .unwrap_or(0);
            files.open(f.to_owned(), line);
        }
    });

    let git = Git::new(&id);
    git.keep_loaded();
    let diff = DiffState::new(&id);
    diff_panel::keep_loaded(id.clone(), diff, git);
    Effect::new(move |prev: Option<()>| {
        git.changed.track();
        if prev.is_some() {
            tree_rev.update(|n| *n += 1);
            diff.reload.update(|n| *n += 1);
            if let Some(cur) = files.current.get_untracked() {
                files.reload(cur, false);
            }
        }
    });

    let panel_tab = RwSignal::new(
        storage::load::<String>("blazar.v2.ws.panel").unwrap_or_else(|| "term".into()),
    );
    let running = Signal::derive(move || {
        detail.with(|d| {
            d.as_ref()
                .and_then(|d| d.as_ref().ok())
                .is_some_and(|d| matches!(d.activity.as_str(), "running" | "awaiting_approval"))
        })
    });
    let tree_files = Signal::derive(move || {
        tree.with(|t| {
            t.as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|t| {
                    t.entries
                        .iter()
                        .filter(|e| !e.is_dir)
                        .map(|e| e.path.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    });
    let show_aux = Callback::new(move |()| {
        if state.lay.with_untracked(|l| l.hide_aux) {
            state.toggle(Region::Aux)
        }
    });
    let show_diff = Callback::new(move |()| {
        if state.lay.with_untracked(|l| l.hide_panel) {
            state.toggle(Region::Panel)
        }
        panel_tab.set("diff".into());
        storage::save("blazar.v2.ws.panel", &"diff");
    });
    let hide_aux = Callback::new(move |()| state.toggle(Region::Aux));
    let draft = RwSignal::new(None::<String>);
    let isolated = Signal::derive(move || {
        detail.with(|d| {
            d.as_ref()
                .and_then(|d| d.as_ref().ok())
                .is_some_and(|d| d.isolated)
        })
    });
    let chat_id = id.clone();
    let chat_view = move || {
        detail
            .with(|d| {
                d.as_ref()
                    .and_then(|d| d.as_ref().ok())
                    .map(|d| (d.path.clone(), d.node.clone()))
            })
            .map(|(path, node)| {
                let c = chat::Chat::new(
                    &chat_id, &path, &node, files, diff, git, running, show_aux, show_diff,
                );
                view! { <chat::ChatPane chat=c tree_files on_hide=hide_aux draft /> }
            })
    };
    let chat_once = Memo::new(move |was: Option<&bool>| {
        was.copied().unwrap_or(false) || detail.with(|d| d.as_ref().is_some_and(Result::is_ok))
    });

    let measure = move || {
        if let Some(g) = grid.try_get_untracked().flatten() {
            let r = g.get_bounding_client_rect();
            state.area.set((r.width(), r.height()));
        }
    };
    Effect::new(move |_| {
        if grid.get().is_some() {
            measure();
        }
    });
    let resize = window_event_listener(ev::resize, move |_| measure());
    Effect::new(move |_| {
        app.side_collapsed.track();
        request_animation_frame(measure);
    });
    let run = move |action: &str| match action {
        "explorer" | "chat" | "panel" => {
            state.toggle(match action {
                "explorer" => Region::Explorer,
                "chat" => Region::Aux,
                _ => Region::Panel,
            });
            true
        }
        tab @ ("diff" | "git" | "preview") => {
            if state.lay.with_untracked(|l| l.hide_panel) {
                state.toggle(Region::Panel);
            }
            panel_tab.set(tab.into());
            storage::save("blazar.v2.ws.panel", &tab);
            true
        }
        _ => false,
    };
    let keys = window_event_listener(ev::keydown, move |e| {
        if let Some(action) = crate::shortcuts::action(&e)
            && run(action)
        {
            e.prevent_default();
        }
    });
    let menu = crate::shortcuts::on_menu(move |id| {
        if let Some(action) = id.strip_prefix("act:") {
            run(action);
        }
    });
    on_cleanup(move || {
        resize.remove();
        keys.remove();
        menu.remove();
    });

    let unload = window_event_listener(ev::beforeunload, move |e| {
        if files.dirty.with_untracked(|d| !d.is_empty()) {
            e.prevent_default();
        }
    });
    on_cleanup(move || unload.remove());

    let md_preview =
        RwSignal::new(crate::markdown_mode::load() == crate::markdown_mode::MarkdownMode::Preview);

    let sizes = move || state.sizes();
    let lay = move || state.lay.get();
    let grid_style = move || {
        let s = sizes();
        format!(
            "--ex-w:{}px;--aux-w:{}px;--panel-h:{}px",
            s.ex, s.aux, s.panel
        )
    };

    let gone = Memo::new(move |_| {
        detail
            .get()
            .is_some_and(|d| d.as_ref().is_err_and(api::ApiError::not_found))
    });
    view! {
        <div class="ws">
            <Show when=move || gone.get()>
                <div class="ws-gone" role="alert">
                    <h2>"这个工作区已被删除"</h2>
                    <p class="muted">
                        "它在别处被删除了，这里的文件、终端和对话都不再可用。"
                    </p>
                    <a class="btn primary" href="/">
                        "回到全部工作区"
                    </a>
                </div>
            </Show>
            <div class="ws-head" inert=move || gone.get()>
                {
                    let id = id.clone();
                    move || {
                        app.workspace(&id)
                            .map(|w| {
                                let up = w.parent.as_deref().and_then(|p| app.workspace(p));
                                view! {
                                    <span class="ws-where">
                                        <span class="ws-machine" title=w.path.clone()>
                                            {crate::fmt::machine(&w.node)}
                                        </span>
                                        {up
                                            .map(|p| {
                                                view! {
                                                    <a class="ws-up" href=format!("/w/{}", p.id)>
                                                        {format!(
                                                            "属于「{}」",
                                                            crate::fmt::ws_title(&p.name, &p.node),
                                                        )}
                                                    </a>
                                                }
                                            })}
                                    </span>
                                }
                            })
                    }
                }
                {move || {
                    detail
                        .get()
                        .and_then(|d| d.err())
                        .filter(|e| !e.not_found())
                        .map(|e| view! { <InlineError message=e.to_string() /> })
                }}
                <span class="grow"></span>
                <Children id=id.clone() open=kids_open />
                <button class="btn small" on:click=move |_| insp.set(true)>
                    "属性"
                </button>
                <button
                    class="laybtn"
                    aria-label="显示或隐藏文件"
                    title=crate::shortcuts::titled("文件", "explorer")
                    aria-pressed=move || (!lay().hide_ex).to_string()
                    on:click=move |_| state.toggle(Region::Explorer)
                    inner_html=ICON_LEFT
                ></button>
                <button
                    class="laybtn"
                    aria-label="切换底部面板"
                    title=crate::shortcuts::titled("面板", "panel")
                    aria-pressed=move || (!lay().hide_panel).to_string()
                    on:click=move |_| state.toggle(Region::Panel)
                    inner_html=ICON_BOTTOM
                ></button>
                <button
                    class="laybtn"
                    aria-label="切换对话面板"
                    title=crate::shortcuts::titled("对话", "chat")
                    aria-pressed=move || (!lay().hide_aux).to_string()
                    on:click=move |_| state.toggle(Region::Aux)
                    inner_html=ICON_RIGHT
                ></button>
            </div>
            {
                let id = id.clone();
                move || {
                    insp.get()
                        .then(|| {
                            view! {
                                <crate::pages::workspaces::Inspector
                                    id=id.clone()
                                    on_close=move || insp.set(false)
                                />
                            }
                        })
                }
            }
            {move || {
                files
                    .error
                    .get()
                    .map(|e| {
                        view! {
                            <div class="ws-error">
                                <InlineError message=e />
                                <button
                                    type="button"
                                    class="btn ghost"
                                    aria-label="关闭文件错误提示"
                                    on:click=move |_| files.error.set(None)
                                >
                                    "×"
                                </button>
                            </div>
                        }
                    })
            }}
            <div class="ws-grid" node_ref=grid style=grid_style inert=move || gone.get()>
                <section
                    class="region explorer"
                    aria-label="文件"
                    data-collapsed=move || lay().hide_ex.to_string()
                >
                    <div class="rhead">
                        <span class="grow"></span>
                        <span class="count">{tree_count}</span>
                        <button
                            class="laybtn"
                            aria-label="刷新文件树"
                            title="重新扫描"
                            inner_html=ICON_REFRESH
                            on:click=move |_| tree_rev.update(|n| *n += 1)
                        ></button>
                    </div>
                    <FileTree root opened files root_name=root_name reload=tree_reload />
                </section>
                <Splitter edge=Edge::Explorer state />
                <section class="region center" data-max=move || lay().panel_max.to_string()>
                    <div class="editor-area">
                        <Tabs files changes />
                        <EdBar files preview=md_preview />
                        <div class="editor-stack">
                            <div
                                class="editor-host"
                                node_ref=host
                                aria-label="代码编辑器"
                            ></div>
                            <MdView files preview=md_preview />
                            <Show when=move || files.open_tabs.with(Vec::is_empty)>
                                <div class="welcome">
                                    <EmptyState
                                        title="选择文件开始编辑"
                                        detail="⌘B 文件 · ⌘J 面板 · ⌘⌥B 对话"
                                        class="editor-empty"
                                    />
                                </div>
                            </Show>
                        </div>
                    </div>
                    <Splitter edge=Edge::Panel state />
                    <section
                        class="region panel"
                        data-collapsed=move || lay().hide_panel.to_string()
                    >
                        <Panel ws=id.clone() files git diff state tab=panel_tab draft isolated />
                    </section>
                </section>
                <Splitter edge=Edge::Aux state />
                <aside class="region aux" data-collapsed=move || lay().hide_aux.to_string()>
                    {move || chat_once.get().then(|| untrack(chat_view.clone()))}
                </aside>
            </div>
        </div>
    }
}

#[component]
fn Panel(
    ws: String,
    files: Files,
    git: Git,
    diff: DiffState,
    state: LayoutState,
    tab: RwSignal<String>,
    draft: RwSignal<Option<String>>,
    isolated: Signal<bool>,
) -> impl IntoView {
    let pick = move |t: &'static str| {
        tab.set(t.to_owned());
        storage::save("blazar.v2.ws.panel", &t);
    };
    let shown = move |t: &'static str| {
        Signal::derive(move || tab.get() == t && !state.lay.get().hide_panel)
    };
    let terms = Terms::new(&ws);
    let collapsed = RwSignal::new(HashSet::<String>::new());
    let panel_max = RwSignal::new(state.lay.get_untracked().panel_max);
    Effect::new(move |_| {
        let m = panel_max.get();
        if state.lay.with_untracked(|l| l.panel_max != m) {
            state.lay.update(|l| l.panel_max = m);
            state.save();
        }
    });
    let diff_n = move || diff.files.with(Vec::len);
    let git_mark = move || {
        git.status.with(|g| match g {
            Some(g) if g.op.is_some() => Some(("bad", "!".to_owned())),
            Some(g) if g.uncommitted > 0 => Some(("", g.uncommitted.to_string())),
            _ => None,
        })
    };
    let tws = ws.clone();
    view! {
        <div class="rhead">
            <div class="rtabs">
                <button
                    class="rtab"
                    data-active=move || (tab.get() == "term").to_string()
                    on:click=move |_| pick("term")
                    title="会话常驻（断线重连回到原处）"
                >
                    "终端"
                </button>
                <button
                    class="rtab"
                    data-active=move || (tab.get() == "diff").to_string()
                    on:click=move |_| pick("diff")
                >
                    "差异"
                    {move || (diff_n() > 0).then(|| view! { <span class="tabn">{diff_n()}</span> })}
                </button>
                <button
                    class="rtab"
                    data-active=move || (tab.get() == "git").to_string()
                    on:click=move |_| pick("git")
                >
                    "Git"
                    {move || {
                        git_mark()
                            .map(|(c, t)| view! { <span class=format!("tabn {c}")>{t}</span> })
                    }}
                </button>
                <button
                    class="rtab"
                    data-active=move || (tab.get() == "preview").to_string()
                    on:click=move |_| pick("preview")
                >
                    "预览"
                </button>
            </div>
            <Show when=move || tab.get() == "term">
                <TermTabs ws=tws.clone() terms />
            </Show>
            <span class="grow"></span>
            <Show when=move || tab.get() == "term">
                <span
                    class="term-state"
                    role="status"
                    aria-live="polite"
                    data-s=move || terms.state.get()
                >
                    {move || match terms.state.get() {
                        "open" => "",
                        "closed" => "已断开",
                        _ => "连接中…",
                    }}
                </span>
                <button
                    class="laybtn"
                    aria-label="重新连接终端"
                    title="重新连接；远端会话仍在时会回到原处"
                    inner_html=ICON_REFRESH
                    on:click=move |_| terms.reconnect.update(|n| *n += 1)
                ></button>
            </Show>
            <button
                class="laybtn"
                aria-label="收起底部面板"
                title=crate::shortcuts::titled("收起面板", "panel")
                on:click=move |_| state.toggle(Region::Panel)
            >
                "▾"
            </button>
        </div>
        <div class="panelbody">
            <div class="ppane" data-active=move || (tab.get() == "term").to_string()>
                <TermPane ws=ws.clone() terms active=shown("term") />
            </div>
            <div class="ppane col" data-active=move || (tab.get() == "diff").to_string()>
                <DiffBar d=diff git collapsed panel_max draft />
                <DiffView ws=ws.clone() d=diff files collapsed />
            </div>
            <div class="ppane" data-active=move || (tab.get() == "git").to_string()>
                <GitView git files draft />
            </div>
            <div class="ppane col" data-active=move || (tab.get() == "preview").to_string()>
                <PreviewBar ws=ws.clone() active=shown("preview") panel_max isolated />
            </div>
        </div>
    }
}
