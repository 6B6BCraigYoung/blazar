use leptos::prelude::*;

pub use crate::vocab::Tone;

#[component]
pub fn LoadingState(
    #[prop(into)] text: String,
    #[prop(default = "empty")] class: &'static str,
) -> impl IntoView {
    view! {
        <div class=format!("ui-state {class}") role="status" aria-live="polite" aria-busy="true">
            {text}
        </div>
    }
}

#[component]
pub fn EmptyState(
    #[prop(into)] title: String,
    #[prop(optional, into)] detail: String,
    #[prop(default = "empty")] class: &'static str,
) -> impl IntoView {
    view! {
        <div class=format!("ui-state {class}")>
            <div>{title}</div>
            {(!detail.is_empty()).then(|| view! { <div class="ui-state-detail">{detail}</div> })}
        </div>
    }
}

#[component]
pub fn InlineError(
    #[prop(into)] message: String,
    #[prop(optional)] retry: Option<Callback<()>>,
    #[prop(default = "")] class: &'static str,
    #[prop(into, default = None)] detail: Option<String>,
) -> impl IntoView {
    view! {
        <div class=format!("ui-state ui-error {class}") role="alert">
            <span>{message}</span>
            {detail
                .map(|d| {
                    view! {
                        <details class="ui-detail">
                            <summary>"详情"</summary>
                            <pre>{d}</pre>
                        </details>
                    }
                })}
            {retry
                .map(|retry| {
                    view! {
                        <button type="button" class="btn small" on:click=move |_| retry.run(())>
                            "重试"
                        </button>
                    }
                })}
        </div>
    }
}

#[component]
pub fn Status(
    #[prop(into)] tone: Signal<Tone>,
    #[prop(into)] label: Signal<String>,
    #[prop(into, default = None)] title: Option<String>,
) -> impl IntoView {
    view! {
        <span class="status" data-tone=move || tone.get().key() title=title>
            {move || label.get()}
        </span>
    }
}

#[component]
pub fn StateOf(
    #[prop(into)] state: Signal<String>,
    #[prop(into, default = None)] title: Option<String>,
) -> impl IntoView {
    let shown = Memo::new(move |_| crate::vocab::state(&state.get()));
    view! {
        <Status
            tone=Signal::derive(move || shown.get().1)
            label=Signal::derive(move || shown.get().0.to_owned())
            title
        />
    }
}
