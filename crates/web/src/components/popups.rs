use leptos::prelude::*;

use crate::app_state::use_app;
use crate::components::icon;
use crate::fmt;
use crate::pages::inbox::{InboxCards, Item};

const SHOWN: usize = 3;
const CARD_MS: u32 = 8000;
const LINE_MS: u32 = 4000;

#[derive(Clone)]
struct Pop {
    id: u32,
    item: Item,
}

#[derive(Clone, Copy)]
struct Pops {
    list: RwSignal<Vec<Pop>>,
    next: StoredValue<u32>,
}

thread_local! {
    static POPS: std::cell::Cell<Option<Pops>> = const { std::cell::Cell::new(None) };
}

pub fn provide() {
    POPS.set(Some(Pops {
        list: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
    }));
}

pub fn arrived(items: Vec<Item>) {
    let Some(p) = POPS.get() else { return };
    for item in items {
        let id = p.next.get_value();
        p.next.set_value(id.wrapping_add(1));
        let ms = if item.needs_you() { CARD_MS } else { LINE_MS };
        p.list.update(|l| {
            l.retain(|x| x.item.id != item.id);
            l.push(Pop { id, item });
        });
        gloo_timers::callback::Timeout::new(ms, move || dismiss(id)).forget();
    }
}

pub fn dismiss(id: u32) {
    if let Some(p) = POPS.get() {
        p.list.try_update(|l| l.retain(|x| x.id != id));
    }
}

pub fn clear() {
    if let Some(p) = POPS.get() {
        p.list.try_update(Vec::clear);
    }
}

fn pop_view(x: Pop) -> impl IntoView {
    let id = x.id;
    if !x.item.needs_you() {
        let it = x.item;
        return view! {
            <article class="pop mini" data-kind=it.kind.clone()>
                <span class="ic" aria-hidden="true" inner_html=icon::svg(icon::TICK)></span>
                <a class="pop-line" href=it.href.clone() on:click=move |_| dismiss(id)>
                    {it.title.clone()}
                </a>
                <time>{fmt::ago(&it.created)}</time>
            </article>
        }
        .into_any();
    }
    let items = Signal::derive(move || vec![x.item.clone()]);
    let rev = RwSignal::new(0u32);
    view! {
        <article class="pop card" data-kind=items.with_untracked(|l| l[0].kind.clone())>
            <InboxCards items rev label="新消息" />
            <button type="button" class="pop-x" aria-label="收起" on:click=move |_| dismiss(id)>
                "×"
            </button>
        </article>
    }
    .into_any()
}

#[component]
pub fn PopHost() -> impl IntoView {
    let p = POPS.get().expect("popups::provide 还没调用");
    let app = use_app();
    Effect::new(move |_| {
        let open: Vec<Item> = app.inbox_items();
        p.list.update(|l| {
            l.retain(|x| {
                open.iter()
                    .any(|it| it.id == x.item.id && (!x.item.needs_you() || it.needs_you()))
            });
        });
    });
    Effect::new(move |_| {
        if app.rail_shown.get() {
            clear();
        }
    });
    let shown = move || {
        p.list
            .with(|l| l.iter().take(SHOWN).cloned().collect::<Vec<_>>())
    };
    let more = move || p.list.with(|l| l.len().saturating_sub(SHOWN));
    view! {
        <div class="pops" role="status" aria-live="polite" aria-relevant="additions">
            <For each=shown key=|x| x.id children=pop_view />
            <Show when=move || { more() > 0 }>
                <div class="pops-more">
                    {move || format!("还有 {} 条消息", more())}
                    <a href="/" on:click=move |_| clear()>
                        "打开中控台"
                    </a>
                </div>
            </Show>
        </div>
    }
}
