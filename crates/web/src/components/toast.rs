use leptos::prelude::*;

#[derive(Clone)]
struct Item {
    id: u32,
    text: String,
}

#[derive(Clone, Copy)]
struct Toasts {
    list: RwSignal<Vec<Item>>,
    next: StoredValue<u32>,
}

thread_local! {
    static TOASTS: std::cell::Cell<Option<Toasts>> = const { std::cell::Cell::new(None) };
}

pub fn provide() {
    TOASTS.set(Some(Toasts {
        list: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
    }));
}

pub fn toast(text: impl Into<String>) {
    let Some(t) = TOASTS.get() else { return };
    let id = t.next.get_value();
    t.next.set_value(id.wrapping_add(1));
    t.list.update(|l| {
        l.push(Item {
            id,
            text: text.into(),
        });
        if l.len() > 3 {
            l.remove(0);
        }
    });
    gloo_timers::callback::Timeout::new(3800, move || {
        t.list.try_update(|l| l.retain(|x| x.id != id));
    })
    .forget();
}

#[component]
pub fn ToastHost() -> impl IntoView {
    let t = TOASTS.get().expect("toast::provide 还没调用");
    view! {
        <div class="toasts" role="status" aria-live="polite" aria-relevant="additions">
            <For each=move || t.list.get() key=|x| x.id let:x>
                <div class="toast">{x.text}</div>
            </For>
        </div>
    }
}
