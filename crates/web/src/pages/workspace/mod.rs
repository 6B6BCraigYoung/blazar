pub mod chat;
mod children;
pub(crate) mod diff_panel;
mod editor;
pub(crate) mod files;
pub(crate) mod git_panel;
mod head;
mod inspector;
mod layout;
mod preview_panel;
mod term_panel;
mod tree;

use std::collections::HashSet;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;

use crate::components::status::InlineError;
use leptos_router::hooks::use_params_map;
use wasm_bindgen::JsCast;

use crate::api::{self, Tree, WorkspaceDetail};
use crate::realtime::use_bus;

use children::{ChildrenOpen, children_open};
use diff_panel::DiffState;
pub(crate) use editor::md_lang;
use files::Files;
use git_panel::Git;
use head::WsHead;
use inspector::Inspector;
use layout::{Changes, Edge, LayoutState, Splitter, Tab};
use term_panel::{TermPane, TermTabs, Terms};
use tree::{TreeActs, TreePane};

pub(crate) const ICON_FILES: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.1"><path class="fill" d="M2 6.5h12V13a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" fill="currentColor" stroke="none"/><path d="M1.5 4A1.5 1.5 0 0 1 3 2.5h3.2l1.5 1.5H13A1.5 1.5 0 0 1 14.5 5.5V13a1.5 1.5 0 0 1-1.5 1.5H3A1.5 1.5 0 0 1 1.5 13z"/><path d="M1.5 6.5h13"/></svg>"#;
pub(crate) const ICON_REFRESH: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 3v6h-6"/></svg>"#;

#[component]
pub fn WorkspacePage() -> impl IntoView {
    let app = crate::app_state::use_app();
    app.side_rail.set(true);
    on_cleanup(move || app.side_rail.set(false));
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();
    let navigate = StoredValue::new_local(leptos_router::hooks::use_navigate());
    Effect::new(move |_| {
        if app.is_assistant(&id()) {
            navigate.with_value(|n| n("/chats", Default::default()));
        }
    });
    move || {
        let id = id();
        view! { <Workspace id /> }
    }
}

fn escapable(e: &web_sys::KeyboardEvent) -> bool {
    e.key() == "Escape"
        && !e.default_prevented()
        && !e.is_composing()
        && document()
            .query_selector("dialog[open], [role=\"dialog\"], .menu")
            .ok()
            .flatten()
            .is_none()
        && !e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| {
                t.closest("input, textarea, select, [contenteditable], .monaco-editor, .xterm")
                    .ok()
                    .flatten()
                    .is_some()
            })
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
            if app.current_ws.get_untracked().as_deref() == Some(id.as_str()) {
                app.current_ws.set(None);
            }
            app.mark_seen(&id);
        });
    }
    let props = RwSignal::new(false);
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
    let state = LayoutState::new(&id);
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
    let tree_count = Signal::derive(move || {
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
    });

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
            files.restore(f.to_owned(), line);
        }
    });

    let git = Git::new(&id);
    git.keep_loaded();
    let diff = DiffState::new(&id);
    diff_panel::keep_loaded(id.clone(), diff, git, state);
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
    let show_aux = Callback::new(move |()| state.reveal_chat());
    let show_diff = Callback::new(move |()| state.open_changes(Changes::Diff));
    let draft = RwSignal::new(None::<String>);
    let mention = RwSignal::new(None::<String>);
    let terms = Terms::new(&id);
    let local_files = RwSignal::new(false);
    let acts = {
        let node_local = Signal::derive(move || {
            detail.with(|d| {
                d.as_ref()
                    .and_then(|d| d.as_ref().ok())
                    .is_some_and(|d| d.node == "local")
            })
        });
        leptos::task::spawn_local(async move {
            let desktop_here = crate::desktop::available() && !crate::desktop::remote_hub().await;
            let _ = local_files.try_set(desktop_here);
        });
        let local = Signal::derive(move || local_files.get() && node_local.get());
        let term_ws = id.clone();
        TreeActs {
            root: Signal::derive(move || {
                detail.with(|d| {
                    d.as_ref()
                        .and_then(|d| d.as_ref().ok())
                        .map(|d| d.path.clone())
                        .unwrap_or_default()
                })
            }),
            local,
            mention: Callback::new(move |p: String| {
                mention.set(Some(p));
                state.reveal_chat();
            }),
            changes: Callback::new(move |p: String| {
                state.open_changes(Changes::Diff);
                let inside = format!("{p}/");
                let at = diff.files.with_untracked(|f| {
                    f.iter()
                        .position(|x| p.is_empty() || x.path == p || x.path.starts_with(&inside))
                });
                if let Some(i) = at {
                    gloo_timers::callback::Timeout::new(120, move || {
                        if let Some(el) = document().get_element_by_id(&format!("df-{i}")) {
                            el.scroll_into_view();
                        }
                    })
                    .forget();
                }
            }),
            terminal: Callback::new(move |dir: String| {
                terms.open_at(&term_ws, dir);
                state.open_term();
            }),
        }
    };
    let tree_pane = TreePane {
        root,
        opened,
        root_name,
        reload: tree_reload,
        rev: tree_rev,
        count: tree_count,
        acts,
    };
    let isolated = Signal::derive(move || {
        detail.with(|d| {
            d.as_ref()
                .and_then(|d| d.as_ref().ok())
                .is_some_and(|d| d.isolated)
        })
    });
    let chat_id = id.clone();
    let chat_slot = RwSignal::new_local(None::<chat::Chat>);
    let thread_menu = RwSignal::new(false);
    let chat_view = move || {
        detail
            .with(|d| {
                d.as_ref()
                    .and_then(|d| d.as_ref().ok())
                    .map(|d| (d.path.clone(), d.node.clone()))
            })
            .map(|(path, node)| {
                let c = chat::Chat::new(
                    &chat_id, &path, &node, files, diff, git, show_aux, show_diff,
                );
                chat_slot.set(Some(c));
                view! { <chat::ChatPane chat=c tree_files draft mention /> }
            })
    };
    let chat_once = Memo::new(move |was: Option<&bool>| {
        was.copied().unwrap_or(false) || detail.with(|d| d.as_ref().is_some_and(Result::is_ok))
    });

    Effect::new(move |prev: Option<u32>| {
        let n = files.revealed.get();
        if prev.is_some_and(|p| p != n) {
            state.open(Tab::Editor);
        }
        n
    });
    let run = move |action: &str| {
        match action {
            "explorer" => state.toggle_files(),
            "chat" => state.toggle_inspect(),
            "panel" => state.toggle_term(),
            "diff" => state.open_changes(Changes::Diff),
            "git" => state.open_changes(Changes::Git),
            "preview" => state.open(Tab::Preview),
            "inspect" => props.set(true),
            _ => return false,
        }
        true
    };
    let keys = window_event_listener(ev::keydown, move |e| {
        if let Some(action) = crate::shortcuts::action(&e)
            && run(action)
        {
            e.prevent_default();
        } else if state.lay.try_with_untracked(|l| l.inspect) == Some(true) && escapable(&e) {
            e.prevent_default();
            state.close();
        }
    });
    let menu = crate::shortcuts::on_menu(move |id| {
        if let Some(action) = id.strip_prefix("act:") {
            run(action);
        }
    });
    on_cleanup(move || {
        keys.remove();
        menu.remove();
    });

    let unload = window_event_listener(ev::beforeunload, move |e| {
        if files.dirty.try_with_untracked(|d| !d.is_empty()) == Some(true) {
            e.prevent_default();
        }
    });
    on_cleanup(move || unload.remove());

    let md_preview =
        RwSignal::new(crate::markdown_mode::load() == crate::markdown_mode::MarkdownMode::Preview);

    let lay = move || state.lay.get();
    let gone = Memo::new(move |_| {
        detail
            .get()
            .is_some_and(|d| d.as_ref().is_err_and(api::ApiError::not_found))
    });
    view! {
        <div
            class="ws"
            data-insp=move || lay().inspect.to_string()
            data-max=move || lay().max.to_string()
            data-term=move || lay().term.to_string()
        >
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
            <WsHead
                id=id.clone()
                detail
                state
                git
                props
                kids_open
                gone
                chat=chat_slot
                thread_menu
            />
            {
                let id = id.clone();
                move || {
                    props
                        .get()
                        .then(|| {
                            view! {
                                <crate::pages::workspaces::Inspector
                                    id=id.clone()
                                    on_close=move || props.set(false)
                                />
                            }
                        })
                }
            }
            {move || {
                files
                    .error
                    .get()
                    .map(|(message, detail)| {
                        view! {
                            <div class="ws-error">
                                <InlineError message detail />
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
            <div class="ws-body" style=move || state.style() inert=move || gone.get()>
                <div class="ws-stage">
                    <div class="ws-row">
                        <section class="ws-chat" aria-label="对话">
                            {move || chat_once.get().then(|| untrack(chat_view.clone()))}
                        </section>
                        <div
                            class="ws-scrim"
                            aria-hidden="true"
                            on:click=move |_| state.close()
                        ></div>
                        <Splitter edge=Edge::Inspector state />
                        <section class="ws-insp" aria-label="检查器">
                            <Inspector
                                ws=id.clone()
                                files
                                tree=tree_pane
                                git
                                diff
                                state
                                host
                                md_preview
                                draft
                                isolated
                            />
                        </section>
                    </div>
                    <Splitter edge=Edge::Terminal state />
                    <section class="ws-term" aria-label="终端">
                        <Terminal ws=id.clone() state terms />
                    </section>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Terminal(ws: String, state: LayoutState, terms: Terms) -> impl IntoView {
    let shown = Signal::derive(move || state.lay.with(|l| l.term));
    view! {
        <div class="rhead">
            <span class="rtitle" title="会话常驻（断线重连回到原处）">
                "终端"
            </span>
            <TermTabs ws=ws.clone() terms />
            <span class="grow"></span>
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
            <button
                class="laybtn"
                aria-label="收起终端"
                title=crate::shortcuts::titled("收起终端", "panel")
                on:click=move |_| state.close_term()
            >
                "▾"
            </button>
        </div>
        <div class="term-body">
            <TermPane ws terms active=shown />
        </div>
    }
}
