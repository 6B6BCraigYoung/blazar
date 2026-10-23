use leptos::prelude::*;

#[component]
pub fn ListGroup(
    #[prop(optional, into)] title: Option<String>,
    #[prop(optional, into)] count: Option<Signal<usize>>,
    #[prop(optional, into)] label: Option<String>,
    #[prop(optional, into)] key: Option<String>,
    #[prop(default = "")] class: &'static str,
    #[prop(optional, into)] actions: Option<ViewFn>,
    children: Children,
) -> impl IntoView {
    let label = label.or_else(|| title.clone());
    view! {
        <section class=format!("lgroup {class}") data-group=key aria-label=label>
            {title
                .map(|t| {
                    view! {
                        <h2 class="lgroup-title">
                            <span>{t}</span>
                            {count.map(|n| view! { <span class="n">{move || n.get()}</span> })}
                            {actions
                                .map(|a| {
                                    view! {
                                        <span class="grow"></span>
                                        <span class="lgroup-actions">{a.run()}</span>
                                    }
                                })}
                        </h2>
                    }
                })}
            <div class="lgroup-body">{children()}</div>
        </section>
    }
}
