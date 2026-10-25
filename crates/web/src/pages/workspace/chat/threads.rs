use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::menu::menu_keydown;

use super::state::Chat;

pub(crate) const CHEVRON: &str = r#"<svg viewBox="0 0 16 16"><path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#;

pub fn cycle(chat: Chat, step: i32) {
    let tabs = chat.tabs.get_untracked();
    if tabs.len() < 2 {
        return;
    }
    let cur = chat.view.get_untracked();
    let at = tabs.iter().position(|t| *t == cur).unwrap_or(0) as i32;
    let n = tabs.len() as i32;
    chat.activate(tabs[(at + step).rem_euclid(n) as usize].clone());
}

fn runtime_name(rt: &str) -> &str {
    match rt {
        "claude" | "claude_cli" => "Claude Code",
        "codex" => "Codex",
        other => other,
    }
}

#[component]
pub fn ThreadPicker(chat: Chat, open: RwSignal<bool>) -> impl IntoView {
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
            .is_some_and(|t| t.closest(".ws-thread").ok().flatten().is_some());
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
    let hint = |id: &str| {
        Some(crate::shortcuts::label(&crate::shortcuts::current(id))).filter(|k| !k.is_empty())
    };
    view! {
        <span class="more ws-thread">
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
                    <For each=move || chat.tabs.get() key=|t| t.clone() let:t>
                        {
                            let on = {
                                let t = t.clone();
                                move || chat.view.get() == t
                            };
                            let live = {
                                let t = t.clone();
                                move || t.as_deref().is_some_and(|id| chat.thread_running(id))
                            };
                            let label = {
                                let t = t.clone();
                                move || chat.thread_title(t.as_deref())
                            };
                            view! {
                                <button
                                    type="button"
                                    role="menuitem"
                                    class="hist-row"
                                    data-sel=move || on().to_string()
                                    on:click=move |_| {
                                        open.set(false);
                                        if chat.view.get_untracked() != t {
                                            chat.activate(t.clone());
                                        }
                                    }
                                >
                                    <b>{label}</b>
                                    <span class="muted small">
                                        {move || if live() { "运行中" } else { "已打开" }}
                                    </span>
                                </button>
                            }
                        }
                    </For>
                    <div class="menu-sep"></div>
                    <button
                        type="button"
                        role="menuitem"
                        on:click=move |_| {
                            open.set(false);
                            chat.new_chat();
                        }
                    >
                        <span>"新对话"</span>
                        {hint("newchat").map(|k| view! { <kbd>{k}</kbd> })}
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
                        <button
                            type="button"
                            role="menuitem"
                            on:click=move |_| {
                                open.set(false);
                                chat.close_tab(chat.view.get_untracked());
                            }
                        >
                            <span>"关闭对话"</span>
                        </button>
                    </Show>
                    {move || {
                        let tabs = chat.tabs.get();
                        let rest: Vec<_> = chat
                            .threads
                            .get()
                            .into_iter()
                            .filter(|t| !tabs.iter().any(|x| x.as_deref() == Some(t.id.as_str())))
                            .collect();
                        (!rest.is_empty())
                            .then(|| {
                                view! {
                                    <div class="menu-sep"></div>
                                    {rest
                                        .into_iter()
                                        .map(|t| {
                                            let id = t.id.clone();
                                            let when = t
                                                .last_at
                                                .clone()
                                                .unwrap_or_else(|| t.created_at.clone());
                                            let sub = format!(
                                                "{} · {}",
                                                runtime_name(&t.runtime),
                                                crate::fmt::ago(&when),
                                            );
                                            view! {
                                                <button
                                                    type="button"
                                                    role="menuitem"
                                                    class="hist-row"
                                                    on:click=move |_| {
                                                        open.set(false);
                                                        chat.activate(Some(id.clone()));
                                                    }
                                                >
                                                    <b>
                                                        {t
                                                            .title
                                                            .clone()
                                                            .filter(|x| !x.is_empty())
                                                            .unwrap_or_else(|| "新对话".into())}
                                                    </b>
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
