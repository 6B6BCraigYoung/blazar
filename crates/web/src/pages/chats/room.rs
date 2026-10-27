use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_location, use_navigate, use_params_map, use_query_map};
use serde_json::json;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app_state::use_app;
use crate::components::menu::menu_keydown;
use crate::components::status::{InlineError, LoadingState};
use crate::pages::workspace::chat::{CHEVRON, Chat, ChatPane};
use crate::vocab;

use super::pulse::Pulse;
use super::rail::Rail;
use super::{NEW, title_of};

#[component]
pub fn ConsolePage() -> impl IntoView {
    let params = use_params_map();
    let location = use_location();
    let assistant = LocalResource::new(|| async move {
        api::send::<api::Assistant>("POST", "/api/assistant", &json!({})).await
    });
    view! {
        {move || match assistant.get() {
            None => view! { <LoadingState text="Loading conversation…" /> }.into_any(),
            Some(Err(e)) => view! { <InlineError message=e.to_string() /> }.into_any(),
            Some(Ok(a)) => {
                match a.id {
                    Some(id) => {
                        let latest = a
                            .threads
                            .iter()
                            .max_by(|x, y| x.when().cmp(y.when()))
                            .map(|t| t.id.clone());
                        let thread = Signal::derive(move || {
                            params
                                .read()
                                .get("thread")
                                .filter(|t| !t.is_empty() && t != "new")
                                .or_else(|| {
                                    location.pathname.with(|p| p == "/").then(|| latest.clone()).flatten()
                                })
                        });
                        view! { <Room id path=a.path thread /> }.into_any()
                    }
                    None => {
                        view! { <InlineError message="Couldn't open the conversation. Try again shortly" /> }
                            .into_any()
                    }
                }
            }
        }}
    }
}

#[component]
fn Room(id: String, path: String, thread: Signal<Option<String>>) -> impl IntoView {
    let app = use_app();
    let navigate = StoredValue::new_local(use_navigate());
    let query = use_query_map();
    let location = use_location();
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
    let files = crate::pages::workspace::files::Files::new(id.clone());
    let diff = crate::pages::workspace::diff_panel::DiffState::new(&id);
    let git = crate::pages::workspace::git_panel::Git::new(&id);
    let noop = Callback::new(|()| {});
    let chat = Chat::new(&id, &path, "local", files, diff, git, noop, noop);
    let draft = RwSignal::new(None::<String>);
    let mention = RwSignal::new(None::<String>);
    let tree_files = Signal::derive(Vec::new);
    let ready = RwSignal::new(false);
    let last = StoredValue::new(None::<Option<String>>);
    Effect::new(move |_| {
        if !ready.get() {
            return;
        }
        let want = thread.get();
        let view = chat.view.get();
        let moved = last.get_value().as_ref() != Some(&want);
        last.set_value(Some(want.clone()));
        if view == want {
            return;
        }
        if moved {
            chat.activate(want);
        } else {
            let href = view.map_or_else(|| NEW.to_owned(), |t| format!("/chats/{t}"));
            navigate.with_value(|n| {
                n(
                    &href,
                    NavigateOptions {
                        replace: true,
                        ..Default::default()
                    },
                )
            });
        }
    });
    let status = Memo::new(move |_| {
        let id = chat.view.get()?;
        let live = chat
            .threads
            .with(|t| t.iter().find(|x| x.id == id).and_then(|x| x.status.clone()));
        let (state, latest) = app.chats.with(|a| {
            a.threads
                .iter()
                .find(|x| x.id == id)
                .map(|t| (t.state().to_owned(), t.latest.clone()))
        })?;
        let state = live.unwrap_or(state);
        if state == "running"
            && let Some(w) = app.assistant()
            && w.session_id.is_some()
            && w.session_id == latest
            && let Some((n, max)) = crate::home_rows::retrying(w.step.as_ref())
        {
            return Some((
                format!("Retrying {}", crate::vocab_en::retry_round(n, max)),
                crate::vocab::Tone::Run,
            ));
        }
        let (_, tone) = vocab::state(&state);
        Some((crate::vocab_en::state(&state).to_owned(), tone))
    });
    let go = Callback::new(move |href: String| {
        navigate.with_value(|n| n(&href, Default::default()));
    });
    let all = Signal::derive(move || query.with(|q| q.get("inbox").as_deref() == Some("all")));
    let rail_mode = Callback::new(move |all: bool| {
        let path = location.pathname.get_untracked();
        let href = if all {
            format!("{path}?inbox=all")
        } else {
            path
        };
        navigate.with_value(|n| n(&href, Default::default()));
    });
    let hint = crate::shortcuts::label(&crate::shortcuts::current("newchat"));
    view! {
        <div class="ws chat-room console" data-rail=move || if all.get() { "all" } else { "summary" }>
            <header class="ws-head chat-room-head console-head">
                <h1 class="console-title">"中控台"</h1>
                <ChatPicker chat go />
                {move || {
                    status
                        .get()
                        .map(|(label, tone)| {
                            view! {
                                <span class="chat-sep" aria-hidden="true">
                                    "·"
                                </span>
                                <span class="status" data-tone=tone.key()>
                                    {label}
                                </span>
                            }
                        })
                }}
                <span class="grow"></span>
                <a class="btn console-all" href="/chats">
                    "全部对话"
                </a>
                <button
                    type="button"
                    class="btn primary console-new"
                    on:click=move |_| {
                        chat.new_chat();
                        chat.show_aux.run(());
                    }
                >
                    "New conversation"
                    {(!hint.is_empty()).then(|| view! { <kbd>{hint.clone()}</kbd> })}
                </button>
            </header>
            <div class="console-body">
                <div class="console-main">
                    <Pulse rail_mode />
                    <div class="ws-body">
                        <div class="ws-stage">
                            <div class="ws-row">
                                <section class="ws-chat" aria-label="Conversation">
                                    <ChatPane chat tree_files draft mention thread ready />
                                </section>
                            </div>
                        </div>
                    </div>
                </div>
                <Rail all rail_mode />
            </div>
        </div>
    }
}

#[component]
fn ChatPicker(chat: Chat, go: Callback<String>) -> impl IntoView {
    let app = use_app();
    let open = RwSignal::new(false);
    let root = NodeRef::<html::Div>::new();
    let button = NodeRef::<html::Button>::new();
    let close = Callback::new(move |()| {
        open.set(false);
        if let Some(b) = button.get_untracked() {
            let _ = b.focus();
        }
    });
    Effect::new(move |_| {
        if open.get() {
            chat.spawn(async move { chat.load_threads().await });
            request_animation_frame(move || {
                if let Some(r) = root.try_get_untracked().flatten()
                    && let Ok(Some(first)) = r.query_selector("[role=\"menuitem\"]")
                    && let Ok(first) = first.dyn_into::<web_sys::HtmlElement>()
                {
                    let _ = first.focus();
                }
            });
        }
    });
    let outside = window_event_listener(ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| t.closest(".chat-pick").ok().flatten().is_some());
        if !inside {
            let _ = open.try_set(false);
        }
    });
    on_cleanup(move || outside.remove());
    let title = move || chat.thread_title(chat.view.get().as_deref());
    let rename = move || {
        let Some(id) = chat.view.get_untracked() else {
            return;
        };
        let cur = chat.thread_title(Some(&id));
        chat.spawn(async move {
            if let Some(name) = crate::components::dialog::prompt_en(
                "Rename conversation",
                "Conversation name",
                &cur,
                "Rename",
            )
            .await
            {
                chat.rename(id, name);
            }
        });
    };
    let hint = crate::shortcuts::label(&crate::shortcuts::current("newchat"));
    view! {
        <span class="more ws-thread chat-pick">
            <button
                type="button"
                class="ws-thread-btn"
                node_ref=button
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                title="Switch conversation"
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <span class="t">{title}</span>
                <span class="chev" aria-hidden="true" inner_html=CHEVRON></span>
            </button>
            <Show when=move || open.get()>
                <div
                    class="menu ws-threads"
                    node_ref=root
                    role="menu"
                    aria-label="Conversations"
                    on:keydown=move |event| {
                        if let Some(r) = root.get_untracked() {
                            menu_keydown(&event, r.unchecked_ref(), close);
                        }
                    }
                >
                    <button
                        type="button"
                        role="menuitem"
                        on:click=move |_| {
                            open.set(false);
                            chat.new_chat();
                        }
                    >
                        <span>"New conversation"</span>
                        {(!hint.is_empty()).then(|| view! { <kbd>{hint.clone()}</kbd> })}
                    </button>
                    <Show when=move || chat.view.get().is_some()>
                        <button
                            type="button"
                            role="menuitem"
                            on:click=move |_| {
                                open.set(false);
                                rename();
                            }
                        >
                            <span>"Rename conversation…"</span>
                        </button>
                    </Show>
                    {move || {
                        let current = chat.view.get();
                        let list = app.chats.with(|a| a.threads.clone());
                        (!list.is_empty())
                            .then(|| {
                                view! {
                                    <div class="menu-sep"></div>
                                    {list
                                        .into_iter()
                                        .map(|t| {
                                            let on = current.as_deref() == Some(t.id.as_str());
                                            let href = format!("/chats/{}", t.id);
                                            let label = crate::vocab_en::state(t.state());
                                            let sub = format!("{label} · {}", crate::fmt::ago_en(t.when()));
                                            view! {
                                                <button
                                                    type="button"
                                                    role="menuitem"
                                                    class="hist-row"
                                                    data-sel=on.to_string()
                                                    on:click=move |_| {
                                                        open.set(false);
                                                        go.run(href.clone());
                                                    }
                                                >
                                                    <b>{title_of(&t)}</b>
                                                    <span class="muted small">{sub}</span>
                                                </button>
                                            }
                                        })
                                        .collect_view()}
                                }
                            })
                    }}
                </div>
            </Show>
        </span>
    }
}
