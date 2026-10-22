use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::menu::menu_keydown;

#[component]
pub fn PageHead(
    #[prop(into)] title: Signal<String>,
    #[prop(optional)] parent: Option<(&'static str, &'static str)>,
    #[prop(optional, into)] icon: Option<ViewFn>,
    #[prop(optional, into)] status: Option<ViewFn>,
    #[prop(optional, into)] nav: Option<ViewFn>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <header class="page-head">
            {parent
                .map(|(label, href)| {
                    view! {
                        <a class="page-parent" href=href>
                            {label}
                        </a>
                        <span class="page-sep" aria-hidden="true">
                            "/"
                        </span>
                    }
                })}
            {icon.map(|icon| view! { <span class="page-icon">{icon.run()}</span> })}
            <h1>{move || title.get()}</h1>
            {status.map(|status| view! { <span class="page-status">{status.run()}</span> })}
            {nav.map(|nav| nav.run())}
            <span class="grow"></span>
            {children.map(|children| view! { <div class="page-actions">{children()}</div> })}
        </header>
    }
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: &'static str,
    pub danger: bool,
    pub run: Callback<()>,
}

impl MenuItem {
    #[must_use]
    pub fn new(label: &'static str, run: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            label,
            danger: false,
            run: Callback::new(move |()| run()),
        }
    }

    #[must_use]
    pub fn danger(label: &'static str, run: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            danger: true,
            ..Self::new(label, run)
        }
    }
}

#[component]
pub fn MoreMenu(
    label: String,
    items: Vec<MenuItem>,
    #[prop(optional, into)] disabled: Signal<bool>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let root = NodeRef::<leptos::html::Div>::new();
    let button = NodeRef::<leptos::html::Button>::new();
    let close = Callback::new(move |()| {
        open.set(false);
        if let Some(b) = button.get_untracked() {
            let _ = b.focus();
        }
    });
    let items = StoredValue::new(items);
    view! {
        <span
            class="more"
            on:focusout=move |e| {
                let inside = e
                    .related_target()
                    .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                    .zip(e.current_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()))
                    .is_some_and(|(t, me)| me.contains(Some(&t)));
                if !inside {
                    open.set(false);
                }
            }
        >
            <button
                type="button"
                class="btn ghost more-btn"
                node_ref=button
                aria-label=label.clone()
                title=label.clone()
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                disabled=move || disabled.get()
                on:click=move |_| open.update(|o| *o = !*o)
            >
                "⋯"
            </button>
            <Show when=move || open.get()>
                <div
                    class="menu"
                    node_ref=root
                    role="menu"
                    aria-label=label.clone()
                    on:keydown=move |event| {
                        if let Some(r) = root.get_untracked() {
                            menu_keydown(&event, r.unchecked_ref(), close);
                        }
                    }
                >
                    {items
                        .get_value()
                        .into_iter()
                        .map(|item| {
                            view! {
                                {item.danger.then(|| view! { <div class="menu-sep"></div> })}
                                <button
                                    type="button"
                                    role="menuitem"
                                    class:danger=item.danger
                                    on:click=move |_| {
                                        open.set(false);
                                        item.run.run(());
                                    }
                                >
                                    {item.label}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
            </Show>
        </span>
    }
}
