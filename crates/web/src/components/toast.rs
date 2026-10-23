use leptos::prelude::*;

#[derive(Clone)]
struct Item {
    id: u32,
    text: String,
    quiet: bool,
    action: Option<(String, Callback<()>)>,
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
    show(text.into(), false, None, 3800);
}

pub fn flash(text: impl Into<String>) {
    show(text.into(), true, None, 1400);
}

pub fn offer(
    text: impl Into<String>,
    label: &str,
    ms: u32,
    run: impl Fn() + Send + Sync + 'static,
) -> u32 {
    show(
        text.into(),
        false,
        Some((label.to_owned(), Callback::new(move |()| run()))),
        ms,
    )
}

pub fn dismiss(id: u32) {
    if let Some(t) = TOASTS.get() {
        t.list.try_update(|l| l.retain(|x| x.id != id));
    }
}

fn show(text: String, quiet: bool, action: Option<(String, Callback<()>)>, ms: u32) -> u32 {
    let Some(t) = TOASTS.get() else { return 0 };
    let id = t.next.get_value();
    t.next.set_value(id.wrapping_add(1));
    t.list.update(|l| {
        l.retain(|x| x.text != text || x.action.is_some());
        l.push(Item {
            id,
            text,
            quiet,
            action,
        });
        if l.len() > 3 {
            l.remove(0);
        }
    });
    gloo_timers::callback::Timeout::new(ms, move || dismiss(id)).forget();
    id
}

fn toast_view(x: Item) -> impl IntoView {
    let with_action = x.action.is_some();
    view! {
        <div class="toast" class:quiet=x.quiet class:act=with_action>
            <span class="toast-text">{x.text}</span>
            {x
                .action
                .map(|(label, run)| {
                    view! {
                        <span class="toast-sep" aria-hidden="true">
                            "·"
                        </span>
                        <button type="button" class="toast-btn" on:click=move |_| run.run(())>
                            {label}
                        </button>
                    }
                })}
        </div>
    }
}

#[component]
pub fn ToastHost() -> impl IntoView {
    let t = TOASTS.get().expect("toast::provide 还没调用");
    view! {
        <div class="toasts" role="status" aria-live="polite" aria-relevant="additions">
            <For each=move || t.list.get() key=|x| x.id children=toast_view />
        </div>
    }
}
