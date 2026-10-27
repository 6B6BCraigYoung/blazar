use leptos::prelude::*;
use serde_json::{Value, json};

use crate::app_state::use_app;
use crate::components::toast::toast;

use super::state::Chat;

fn reason(status: &str) -> &'static str {
    match status {
        "offline" => "Machine offline",
        other => crate::vocab_en::state(other),
    }
}

#[component]
pub fn Offline(chat: Chat) -> impl IntoView {
    let app = use_app();
    let node = chat.node.get_value();
    let machine = crate::vocab_en::machine(&node);
    let href = format!("/nodes/{}", crate::api::enc(&node));
    let retrying = RwSignal::new(false);
    let info = {
        let node = node.clone();
        Memo::new(move |_| {
            app.nodes()
                .into_iter()
                .find(|n| n.name == node)
                .map(|n| (n.status, n.last_seen_at))
        })
    };
    let sub = move || {
        let (status, seen) = info.get().unwrap_or_default();
        let seen = seen
            .filter(|s| !s.is_empty())
            .map(|s| crate::fmt::ago_en(&s))
            .filter(|s| !s.is_empty())
            .map(|s| format!("Last seen {s}"));
        [seen, Some(reason(&status).to_owned())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let retry = move || {
        if retrying.get_untracked() {
            return;
        }
        retrying.set(true);
        let node = node.clone();
        chat.spawn(async move {
            let probed = crate::api::send::<Value>(
                "POST",
                &format!("/api/nodes/{}/probe", crate::api::enc(&node)),
                &json!({}),
            )
            .await;
            app.load_state();
            let _ = retrying.try_set(false);
            if let Err(e) = probed {
                toast(format!(
                    "Still can't connect: {}",
                    crate::vocab_en::error(&e.to_string(), None)
                ));
            }
        });
    };
    view! {
        <div class="blank cc-offline" role="status">
            <h2 class="blank-title">{format!("Can't reach {machine}")}</h2>
            <p class="blank-detail">{sub}</p>
            <div class="cc-offline-acts">
                <button
                    type="button"
                    class="btn primary"
                    disabled=move || retrying.get()
                    on:click=move |_| retry()
                >
                    {move || if retrying.get() { "Retrying…" } else { "Retry" }}
                </button>
                <a class="btn" href=href>
                    "Open machine"
                </a>
            </div>
        </div>
    }
}
