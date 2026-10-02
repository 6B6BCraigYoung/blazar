//! 右侧对话栏：标签页 + 对话记录 + 输入框。交互和样子对齐 Claude Code 插件。

mod composer;
mod dock;
mod log;
pub mod state;

use leptos::prelude::*;

use crate::realtime::use_bus;

use composer::Composer;
use log::Log;
pub use state::Chat;

#[component]
pub fn ChatPane(
    chat: Chat,
    tree_files: Signal<Vec<String>>,
    on_hide: Callback<()>,
    draft: RwSignal<Option<String>>,
) -> impl IntoView {
    let bus = use_bus();
    chat.wire(bus);
    chat.init();
    chat.load_catalogs();
    // 别处（差异的「让 agent 审阅」、Git 的「让 agent 解决」）写好的请求：开新对话、填进输入框，不直接发。
    Effect::new(move |_| {
        if let Some(text) = draft.get() {
            draft.set(None);
            chat.new_chat();
            chat.prompt.set(text);
            chat.show_aux.run(());
            crate::components::toast::toast("已在新对话里写好请求：选好 agent 再发送");
        }
    });

    // 转圈和计时：在跑的时候每 120ms 走一格。
    let tick = RwSignal::new(0u32);
    let iv = StoredValue::new_local(None::<gloo_timers::callback::Interval>);
    Effect::new(move |_| {
        if chat.running.get() {
            if iv.with_value(Option::is_none) {
                iv.set_value(Some(gloo_timers::callback::Interval::new(120, move || {
                    tick.update(|t| *t = t.wrapping_add(1))
                })));
            }
        } else {
            iv.set_value(None);
        }
    });
    on_cleanup(move || iv.set_value(None));

    let hist = RwSignal::new(false);
    view! {
        <div class="rhead chat-head">
            <div class="chat-tabs">
                <For each=move || chat.tabs.get() key=|t| t.clone() let:t>
                    {
                        let t1 = t.clone();
                        let t2 = t.clone();
                        let t3 = t.clone();
                        let t4 = t.clone();
                        let title = Signal::derive(move || chat.thread_title(t1.as_deref()));
                        let active = move || chat.view.get() == t2;
                        let live = move || t3.as_deref().is_some_and(|id| chat.thread_running(id));
                        view! {
                            <button class="ctab" data-active=move || active().to_string() title=move || format!("{}（双击改名）", title.get())
                                on:click={ let t = t.clone(); move |_| if chat.view.get_untracked() != t { chat.activate(t.clone()) } }
                                on:dblclick={ let t = t4.clone(); move |_| {
                                    let Some(id) = t.clone() else { return };
                                    let cur = chat.thread_title(Some(&id));
                                    chat.spawn(async move {
                                        let name = window().prompt_with_message_and_default("对话标题", &cur).ok().flatten();
                                        if let Some(n) = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()) {
                                            chat.rename(id, n);
                                        }
                                    });
                                }}>
                                <span class="t">{move || title.get()}</span>
                                {move || live().then(|| view! { <span class="live"></span> })}
                                <span class="x" title="关闭标签" on:click={ let t = t.clone(); move |e| { e.stop_propagation(); chat.close_tab(t.clone()); } }>"×"</span>
                            </button>
                        }
                    }
                </For>
                <button class="ctab add" title="新对话" on:click=move |_| chat.new_chat()>"＋"</button>
            </div>
            <span class="more">
                <button class="laybtn" title="对话历史" on:click=move |_| {
                    hist.update(|h| *h = !*h);
                    if hist.get_untracked() { chat.spawn(async move { chat.load_threads().await }); }
                } inner_html=r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7M3 4v4h4"/><path d="M12 7v5l3 2"/></svg>"#></button>
                <Show when=move || hist.get()>
                    <div class="menu hist" on:mouseleave=move |_| hist.set(false)>
                        <button on:click=move |_| { hist.set(false); chat.new_chat(); }><b>"＋ New conversation"</b></button>
                        <div class="menu-sep"></div>
                        {move || chat.threads.get().into_iter().map(|t| {
                            let id = t.id.clone();
                            let on = chat.view.get().as_deref() == Some(id.as_str());
                            let when = t.last_at.clone().unwrap_or_else(|| t.created_at.clone());
                            let sub = format!("{} · {}{}", t.runtime, crate::fmt::ago(&when), if t.status.as_deref() == Some("running") { " · running" } else { "" });
                            view! {
                                <button class="hist-row" data-sel=on.to_string() on:click=move |_| { hist.set(false); chat.activate(Some(id.clone())); }>
                                    <b>{t.title.clone().filter(|x| !x.is_empty()).unwrap_or_else(|| "(empty)".into())}</b>
                                    <span class="muted small">{sub}</span>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </Show>
            </span>
            <button class="laybtn" title="新对话" on:click=move |_| chat.new_chat() inner_html=r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>"#></button>
            <button class="laybtn" title="收起 ⌘⌥B" on:click=move |_| on_hide.run(())>"×"</button>
        </div>
        <div class="chat-body">
            <Log chat tick/>
            <Composer chat tick tree_files/>
        </div>
    }
}
