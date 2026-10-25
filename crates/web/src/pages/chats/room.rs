use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::json;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app_state::use_app;
use crate::components::menu::menu_keydown;
use crate::components::status::{InlineError, LoadingState};
use crate::pages::workspace::chat::{CHEVRON, Chat, ChatPane};
use crate::vocab;

use super::{NEW, title_of};

#[component]
pub fn ChatPage() -> impl IntoView {
    let params = use_params_map();
    let thread = Signal::derive(move || {
        params
            .read()
            .get("thread")
            .filter(|t| !t.is_empty() && t != "new")
    });
    let assistant = LocalResource::new(|| async move {
        api::send::<api::Assistant>("POST", "/api/assistant", &json!({})).await
    });
    view! {
        {move || match assistant.get() {
            None => view! { <LoadingState text="正在打开对话…" /> }.into_any(),
            Some(Err(e)) => view! { <InlineError message=e.to_string() /> }.into_any(),
            Some(Ok(a)) => {
                match a.id {
                    Some(id) => view! { <Room id path=a.path thread /> }.into_any(),
                    None => view! { <InlineError message="对话没能打开，稍后再试" /> }.into_any(),
                }
            }
        }}
    }
}

#[component]
fn Room(id: String, path: String, thread: Signal<Option<String>>) -> impl IntoView {
    let app = use_app();
    let navigate = StoredValue::new_local(use_navigate());
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
    let running = Signal::derive(move || {
        app.assistant()
            .is_some_and(|w| matches!(w.activity.as_str(), "running" | "awaiting_approval"))
    });
    let files = crate::pages::workspace::files::Files::new(id.clone());
    let diff = crate::pages::workspace::diff_panel::DiffState::new(&id);
    let git = crate::pages::workspace::git_panel::Git::new(&id);
    let noop = Callback::new(|()| {});
    let chat = Chat::new(&id, &path, "local", files, diff, git, running, noop, noop);
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
        let state = app.chats.with(|a| {
            a.threads
                .iter()
                .find(|x| x.id == id)
                .map(|t| t.state().to_owned())
        })?;
        if state == "running"
            && let Some(w) = app.assistant()
        {
            return Some(crate::home_rows::status(&w.activity, w.step.as_ref()));
        }
        let (label, tone) = vocab::state(&state);
        Some((label.to_owned(), tone))
    });
    let go = Callback::new(move |href: String| {
        navigate.with_value(|n| n(&href, Default::default()));
    });
    view! {
        <div class="ws chat-room">
            <header class="ws-head chat-room-head">
                <a class="chat-up" href="/chats">
                    "对话"
                </a>
                <span class="chat-sep" aria-hidden="true">
                    "·"
                </span>
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
            </header>
            <div class="ws-body">
                <div class="ws-stage">
                    <div class="ws-row">
                        <section class="ws-chat" aria-label="对话">
                            <ChatPane chat tree_files draft mention thread ready />
                        </section>
                    </div>
                </div>
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
            if let Some(name) =
                crate::components::dialog::prompt_with("重命名对话", "对话名称", &cur, "重命名")
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
                title="切换对话"
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
                    aria-label="对话"
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
                        <span>"新对话"</span>
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
                            <span>"重命名对话…"</span>
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
                                            let (label, _) = vocab::state(t.state());
                                            let sub = format!("{label} · {}", crate::fmt::ago(t.when()));
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
