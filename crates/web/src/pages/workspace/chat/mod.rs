mod composer;
mod dock;
mod log;
pub mod state;
mod threads;
mod turns;

use leptos::prelude::*;

use wasm_bindgen::JsCast;

use crate::realtime::use_bus;

use composer::Composer;
use log::Log;
pub use state::Chat;
pub use threads::ThreadPicker;

#[component]
pub fn ChatPane(
    chat: Chat,
    tree_files: Signal<Vec<String>>,
    draft: RwSignal<Option<String>>,
    mention: RwSignal<Option<String>>,
) -> impl IntoView {
    let bus = use_bus();
    chat.wire(bus);
    let query = leptos_router::hooks::use_query_map();
    let requested = Signal::derive(move || query.read().get("thread").filter(|s| !s.is_empty()));
    let ready = RwSignal::new(false);
    chat.init(requested, ready);
    Effect::new(move |_| {
        let target = requested.get();
        if ready.get()
            && let Some(id) = target
        {
            if chat
                .threads
                .with_untracked(|t| t.iter().any(|t| t.id == id))
                && chat.view.get_untracked().as_ref() != Some(&id)
            {
                chat.activate(Some(id));
            }
            chat.show_aux.run(());
        }
    });
    chat.load_catalogs();
    let keys = window_event_listener(leptos::ev::keydown, move |e| {
        match crate::shortcuts::action(&e) {
            Some("newchat") => {
                chat.new_chat();
                chat.show_aux.run(());
            }
            Some("prevchat") => threads::cycle(chat, -1),
            Some("nextchat") => threads::cycle(chat, 1),
            _ => return,
        }
        e.prevent_default();
    });
    let menu = crate::shortcuts::on_menu(move |id| {
        if id == "act:newchat" {
            chat.new_chat();
            chat.show_aux.run(());
        }
    });
    on_cleanup(move || {
        keys.remove();
        menu.remove();
    });
    Effect::new(move |previous: Option<Option<String>>| {
        let profile = query.read().get("agent").filter(|id| !id.is_empty());
        let fresh = query.read().get("new").as_deref() == Some("1");
        if !ready.get() {
            return None;
        }
        if fresh && profile.is_some() && previous.as_ref() != Some(&profile) {
            chat.new_chat();
            chat.set_agent(format!("p:{}", profile.as_deref().unwrap_or_default()));
            chat.show_aux.run(());
        }
        profile
    });
    Effect::new(move |_| {
        if let Some(text) = draft.get() {
            draft.set(None);
            chat.new_chat();
            chat.prompt.set(text);
            chat.show_aux.run(());
            crate::components::toast::toast("草稿已放进新对话，选好智能体后发送。");
        }
    });

    Effect::new(move |_| {
        if let Some(path) = mention.get() {
            mention.set(None);
            chat.prompt.update(|p| {
                if !p.is_empty() && !p.ends_with(char::is_whitespace) {
                    p.push(' ');
                }
                p.push('@');
                p.push_str(&path);
                p.push(' ');
            });
            request_animation_frame(|| {
                if let Ok(Some(el)) = document().query_selector(".cc-box .prompt")
                    && let Ok(el) = el.dyn_into::<web_sys::HtmlElement>()
                {
                    let _ = el.focus();
                }
            });
        }
    });

    let tick = RwSignal::new(0u32);
    let iv = StoredValue::new_local(None::<gloo_timers::callback::Interval>);
    Effect::new(move |_| {
        if chat.running.get() {
            if iv.with_value(Option::is_none) {
                iv.set_value(Some(gloo_timers::callback::Interval::new(120, move || {
                    tick.try_update(|t| *t = t.wrapping_add(1));
                })));
            }
        } else {
            iv.set_value(None);
        }
    });
    on_cleanup(move || iv.set_value(None));

    let log_h = RwSignal::new(0.0f64);
    view! {
        <div class="chat-body">
            <super::children::ChildrenPanel id=chat.ws_id() />
            <Log chat tick log_h />
            <Composer chat tree_files log_h />
        </div>
    }
}
