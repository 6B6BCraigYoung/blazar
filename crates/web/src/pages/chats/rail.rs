use leptos::prelude::*;

use crate::app_state::use_app;
use crate::pages::inbox::{ALL, InboxAll, InboxCards, Item, mark_inbox};
use crate::pages::work_shared::{refresh, work_revision};

const FEED_MAX: usize = 12;

#[component]
pub fn Rail(all: Signal<bool>, rail_mode: Callback<bool>) -> impl IntoView {
    let app = use_app();
    app.rail_shown.set(true);
    on_cleanup(move || app.rail_shown.set(false));
    let rev = work_revision("inbox");
    let items = Memo::new(move |_| app.inbox_items());
    let needs = Signal::derive(move || {
        items.with(|l| {
            l.iter()
                .filter(|it| it.needs_you())
                .cloned()
                .collect::<Vec<Item>>()
        })
    });
    let feed = Signal::derive(move || {
        items.with(|l| {
            l.iter()
                .filter(|it| !it.needs_you() && !it.archived)
                .take(FEED_MAX)
                .cloned()
                .collect::<Vec<Item>>()
        })
    });
    let busy = RwSignal::new(false);
    let read_all = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        mark_inbox("read", vec![], move || {
            refresh(rev);
            let _ = busy.try_set(false);
        });
    };
    view! {
        <aside class="console-rail" aria-label="消息" data-mode=move || if all.get() { "all" } else { "summary" }>
            <Show
                when=move || all.get()
                fallback=move || {
                    view! {
                        <div class="rail-head">
                            <h2>"消息"</h2>
                            <span class="grow"></span>
                            <Show when=move || { app.inbox_unread.get() > 0 }>
                                <button
                                    type="button"
                                    class="linkbtn"
                                    disabled=move || busy.get()
                                    on:click=read_all
                                >
                                    "全部已读"
                                </button>
                            </Show>
                        </div>
                        <section class="rail-needs" aria-label="需要你处理">
                            <h3 class="rail-title">
                                "需要你处理"
                                {move || {
                                    let n = needs.with(Vec::len);
                                    (n > 0).then(|| view! { <em>{n}</em> })
                                }}
                            </h3>
                            <Show
                                when=move || needs.with(|l| !l.is_empty())
                                fallback=|| view! { <p class="rail-empty">"都处理完了，新的请求会出现在这里。"</p> }
                            >
                                <InboxCards items=needs rev label="需要你处理" />
                            </Show>
                        </section>
                        <section class="rail-feed" aria-label="动态">
                            <h3 class="rail-title">
                                "动态"
                                <span class="grow"></span>
                                <a class="rail-link" href=ALL on:click=move |e| {
                                    e.prevent_default();
                                    rail_mode.run(true);
                                }>
                                    "查看全部"
                                </a>
                            </h3>
                            <Show
                                when=move || feed.with(|l| !l.is_empty())
                                fallback=|| view! { <p class="rail-empty">"还没有动态。"</p> }
                            >
                                <InboxCards items=feed rev label="动态" />
                            </Show>
                        </section>
                    }
                }
            >
                <div class="rail-head">
                    <button type="button" class="rail-back" on:click=move |_| rail_mode.run(false)>
                        <span class="ic" aria-hidden="true">
                            "‹"
                        </span>
                        "消息"
                    </button>
                    <span class="grow"></span>
                    <span class="rail-count">
                        {move || {
                            let n = app.inbox_unread.get();
                            if n > 0 { format!("{n} 条未读") } else { String::new() }
                        }}
                    </span>
                </div>
                <InboxAll />
            </Show>
        </aside>
    }
}
