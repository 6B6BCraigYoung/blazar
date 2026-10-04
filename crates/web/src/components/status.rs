use leptos::prelude::*;

#[component]
pub fn LoadingState(
    #[prop(into)] text: String,
    #[prop(default = "empty")] class: &'static str,
) -> impl IntoView {
    view! { <div class=format!("ui-state {class}") role="status" aria-live="polite" aria-busy="true">{text}</div> }
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
) -> impl IntoView {
    view! {
        <div class=format!("ui-state ui-error {class}") role="alert">
            <span>{message}</span>
            {retry.map(|retry| view! { <button type="button" class="btn small" on:click=move |_| retry.run(())>"重试"</button> })}
        </div>
    }
}
