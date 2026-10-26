use leptos::prelude::*;

use super::chat::{Chat, Dock};
use super::layout::LayoutState;
use crate::app_state::use_app;

#[component]
pub fn MiniBar(
    id: String,
    chat: RwSignal<Option<Chat>, LocalStorage>,
    state: LayoutState,
) -> impl IntoView {
    let app = use_app();
    let status = Signal::derive(move || {
        app.workspace(&id)
            .map(|w| crate::home_rows::status(&w.activity, w.step.as_ref()))
            .unwrap_or_else(|| crate::home_rows::status("", None))
    });
    let prompt = move || chat.get().map_or_else(String::new, |c| c.prompt.get());
    let busy = move || chat.get().is_some_and(|c| c.running.get());
    view! {
        <div class="ws-mini">
            {move || chat.get().map(|c| view! { <div class="ws-mini-dock"><Dock chat=c /></div> })}
            <div class="ws-mini-bar">
                <span class="status" data-tone=move || status.get().1.key()>
                    {move || status.get().0}
                </span>
                <input
                    class="ws-mini-in"
                    aria-label="给智能体发消息"
                    placeholder=move || if busy() { "追加一句，回车发送…" } else { "发消息，回车发送…" }
                    prop:value=prompt
                    on:input=move |e| {
                        if let Some(c) = chat.get_untracked() {
                            c.prompt.set(event_target_value(&e));
                        }
                    }
                    on:keydown=move |e| {
                        if e.key() == "Enter" && !e.is_composing() && !e.shift_key() {
                            e.prevent_default();
                            if let Some(c) = chat.get_untracked()
                                && !c.prompt.with_untracked(|p| p.trim().is_empty())
                            {
                                c.send();
                            }
                        }
                    }
                />
                <button
                    type="button"
                    class="btn small"
                    title=crate::shortcuts::titled("回到对话", "chat")
                    on:click=move |_| state.set_max(false)
                >
                    "回到对话"
                </button>
            </div>
        </div>
    }
}
