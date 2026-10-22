use leptos::prelude::*;

#[derive(Clone)]
struct Item {
    id: u32,
    text: String,
    quiet: bool,
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
    show(text.into(), false, 3800);
}

pub fn flash(text: impl Into<String>) {
    show(text.into(), true, 1400);
}

fn show(text: String, quiet: bool, ms: u32) {
    let Some(t) = TOASTS.get() else { return };
    let id = t.next.get_value();
    t.next.set_value(id.wrapping_add(1));
    t.list.update(|l| {
        l.retain(|x| x.text != text);
        l.push(Item { id, text, quiet });
        if l.len() > 3 {
            l.remove(0);
        }
    });
    gloo_timers::callback::Timeout::new(ms, move || {
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
                <div class="toast" class:quiet=x.quiet>
                    {x.text}
                </div>
            </For>
        </div>
    }
}
