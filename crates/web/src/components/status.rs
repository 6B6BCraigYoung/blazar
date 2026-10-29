use leptos::prelude::*;

use crate::api::ApiError;

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

/// 一条行内错误。默认说人话：`error`（一个 [`ApiError`]）或 `message`（原文）先过
/// [`crate::vocab::explain`]，原文折进「详情」；`lead` 是“在做什么”的前缀（“无法加载设置”）。
/// 已经是人话的文字用 `plain`（或者自己给了 `detail`）原样显示。对话里用 `english`。
#[component]
pub fn InlineError(
    #[prop(optional, into)] message: String,
    #[prop(optional, into)] error: Option<ApiError>,
    #[prop(optional, into)] lead: String,
    #[prop(optional)] plain: bool,
    #[prop(optional)] english: bool,
    #[prop(optional)] retry: Option<Callback<()>>,
    #[prop(default = "")] class: &'static str,
    #[prop(into, default = None)] detail: Option<String>,
) -> impl IntoView {
    let (message, detail) = if plain || detail.is_some() {
        (message, detail)
    } else {
        match (&error, english) {
            (Some(e), false) => e.explained(),
            (Some(e), true) => e.explained_en(),
            (None, false) => crate::vocab::explain(&message, None),
            (None, true) => crate::vocab_en::explain(&message, None),
        }
    };
    let (message, detail) = if english {
        crate::vocab_en::lead(&lead, (message, detail))
    } else {
        crate::vocab::lead(&lead, (message, detail))
    };
    let (more, again) = if english {
        ("Details", "Retry")
    } else {
        ("详情", "重试")
    };
    view! {
        <div class=format!("ui-state ui-error {class}") role="alert">
            <span>{message}</span>
            {detail
                .map(|d| {
                    view! {
                        <details class="ui-detail">
                            <summary>{more}</summary>
                            <pre>{d}</pre>
                        </details>
                    }
                })}
            {retry
                .map(|retry| {
                    view! {
                        <button type="button" class="btn small" on:click=move |_| retry.run(())>
                            {again}
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
pub fn Checking(#[prop(into)] busy: Signal<bool>) -> impl IntoView {
    view! {
        <span class="page-checking" data-on=move || busy.get().to_string() aria-live="polite">
            "检查中…"
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
