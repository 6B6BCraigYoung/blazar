use leptos::prelude::*;

use super::icon;

pub const DONE: &str = "都处理完了";

fn spaced(prefix: &str, noun: &str) -> String {
    if noun.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        format!("{prefix} {noun}")
    } else {
        format!("{prefix}{noun}")
    }
}

#[must_use]
pub fn never(noun: &str) -> String {
    spaced("还没有", noun)
}

#[must_use]
pub fn unmatched(noun: &str) -> String {
    spaced("没有符合条件的", noun)
}

#[derive(Clone)]
pub struct Action {
    label: String,
    keys: Option<String>,
    primary: bool,
    run: Callback<()>,
}

impl Action {
    #[must_use]
    pub fn new(label: impl Into<String>, run: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            label: label.into(),
            keys: None,
            primary: false,
            run: Callback::new(move |()| run()),
        }
    }

    #[must_use]
    pub fn clear(run: impl Fn() + Send + Sync + 'static) -> Self {
        Self::new("清除筛选", run)
    }

    #[must_use]
    pub fn keys(mut self, combo: &str) -> Self {
        let label = crate::shortcuts::label(combo);
        self.keys = (!label.is_empty()).then_some(label);
        self
    }

    #[must_use]
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
}

#[component]
pub fn Blank(
    icon: &'static str,
    #[prop(into)] title: String,
    #[prop(optional, into)] detail: String,
    #[prop(optional)] action: Option<Action>,
    /// A second, plain button beside `action`.
    #[prop(optional)]
    also: Option<Action>,
    #[prop(optional)] inset: bool,
) -> impl IntoView {
    let button = |a: Action| {
        let run = a.run;
        view! {
            <button type="button" class="btn" class:primary=a.primary on:click=move |_| run.run(())>
                {a.label}
                {a.keys.map(|k| view! { <kbd>{k}</kbd> })}
            </button>
        }
    };
    view! {
        <div class="blank" class:inset=inset class:fill=!inset>
            <span class="blank-mark" aria-hidden="true">
                <span class="blank-horizon" inner_html=super::brand::HORIZON></span>
                <span class="blank-icon" inner_html=icon::svg(icon)></span>
            </span>
            <h2 class="blank-title">{title}</h2>
            {(!detail.is_empty()).then(|| view! { <p class="blank-detail">{detail}</p> })}
            {(action.is_some() || also.is_some())
                .then(|| {
                    view! {
                        <div class="blank-actions">{action.map(button)} {also.map(button)}</div>
                    }
                })}
        </div>
    }
}
