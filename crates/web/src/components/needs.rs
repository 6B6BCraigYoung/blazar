use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::app_state::use_app;
use crate::pages::inbox::{InboxCards, Item};
use crate::pages::work_shared::work_revision;

/// Everything waiting on you, from every workspace and the console, one click from any page.
/// The console keeps its own rail, so the sheet only opens where that rail isn't shown.
#[component]
pub fn NeedsSheet() -> impl IntoView {
    let app = use_app();
    let location = use_location();
    let rev = work_revision("inbox");
    let close = move || app.needs_open.set(false);
    Effect::new(move |prev: Option<String>| {
        let here = format!("{}{}", location.pathname.get(), location.search.get());
        if prev.is_some_and(|p| p != here) {
            app.needs_open.set(false);
        }
        here
    });
    Effect::new(move |_| {
        if app.rail_shown.get() {
            app.needs_open.set(false);
        }
    });
    let keys = window_event_listener(leptos::ev::keydown, move |e| {
        if e.key() == "Escape" && app.needs_open.get_untracked() {
            e.prevent_default();
            close();
        }
    });
    on_cleanup(move || keys.remove());
    let needs = Signal::derive(move || {
        app.inbox_items()
            .into_iter()
            .filter(Item::needs_you)
            .collect::<Vec<Item>>()
    });
    view! {
        <Show when=move || app.needs_open.get()>
            <div class="needs-scrim" on:click=move |_| close()></div>
            <section class="needs-sheet" role="dialog" aria-label="需要你处理">
                <header class="needs-head">
                    <h2>
                        "需要你处理"
                        {move || {
                            let n = needs.with(Vec::len);
                            (n > 0).then(|| view! { <em>{n}</em> })
                        }}
                    </h2>
                    <span class="grow"></span>
                    <a class="linkbtn" href="/">
                        "打开中控台"
                    </a>
                    <button
                        type="button"
                        class="needs-x"
                        aria-label="关闭"
                        title="关闭（Esc）"
                        on:click=move |_| close()
                    >
                        "×"
                    </button>
                </header>
                <div class="needs-body">
                    <Show
                        when=move || needs.with(|l| !l.is_empty())
                        fallback=|| {
                            view! {
                                <p class="rail-empty">
                                    "都处理完了，新的请求会出现在这里。"
                                </p>
                            }
                        }
                    >
                        <InboxCards
                            items=needs
                            rev
                            label="需要你处理"
                            compact=true
                            open_first=true
                        />
                    </Show>
                </div>
            </section>
        </Show>
    }
}
