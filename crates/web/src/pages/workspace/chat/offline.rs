use leptos::prelude::*;
use serde_json::{Value, json};

use crate::app_state::use_app;
use crate::components::toast::toast_error_en;
use crate::link_state::NodeLink;

use super::state::Chat;

/// 「Last seen 3m ago」；不知道上次什么时候连上过时，退回机器状态（不说重复的 offline）。
fn seen_line(chat: Chat) -> Memo<String> {
    let app = use_app();
    let node = chat.node.get_value();
    Memo::new(move |_| {
        let (status, seen) = app
            .nodes()
            .into_iter()
            .find(|n| n.name == node)
            .map(|n| (n.status, n.last_seen_at))
            .unwrap_or_default();
        seen.filter(|s| !s.is_empty())
            .map(|s| crate::fmt::ago_en(&s))
            .filter(|s| !s.is_empty())
            .map(|s| format!("Last seen {s}"))
            .or_else(|| {
                (!status.is_empty() && status != "offline")
                    .then(|| crate::vocab_en::state(&status).to_owned())
            })
            .unwrap_or_default()
    })
}

fn retry(app: crate::app_state::AppData, chat: Chat, retrying: RwSignal<bool>) {
    if retrying.get_untracked() {
        return;
    }
    retrying.set(true);
    let node = chat.node.get_value();
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
            toast_error_en("Still can't connect", &e);
        }
    });
}

/// 对话是空的、机器又连不上：整块空状态。
#[component]
pub fn Offline(chat: Chat) -> impl IntoView {
    let machine = chat.machine();
    let href = format!("/nodes/{}", crate::api::enc(&chat.node.get_value()));
    let app = use_app();
    let retrying = RwSignal::new(false);
    let seen = seen_line(chat);
    view! {
        <div class="blank cc-offline" role="status">
            <h2 class="blank-title">{format!("Can't reach {machine}")}</h2>
            <Show when=move || seen.with(|s| !s.is_empty())>
                <p class="blank-detail">{move || seen.get()}</p>
            </Show>
            <div class="cc-offline-acts">
                <button
                    type="button"
                    class="btn primary"
                    disabled=move || retrying.get()
                    on:click=move |_| retry(app, chat, retrying)
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

/// 有历史时：对话照常显示，顶部一行安静的提示。
#[component]
pub fn OfflineLine(chat: Chat, link: Memo<NodeLink>) -> impl IntoView {
    let machine = chat.machine();
    let app = use_app();
    let retrying = RwSignal::new(false);
    let seen = seen_line(chat);
    view! {
        <div class="cc-link" role="status" data-link=move || format!("{:?}", link.get())>
            {move || {
                if link.get() == NodeLink::Reconnecting {
                    view! {
                        <span>
                            {format!("Reconnecting to {machine}… the agent keeps running there")}
                        </span>
                    }
                        .into_any()
                } else {
                    let line = seen.get();
                    let text = if line.is_empty() {
                        format!("Can't reach {machine}")
                    } else {
                        format!("Can't reach {machine} · {line}")
                    };
                    view! {
                        <span>{text}</span>
                        <button
                            type="button"
                            class="linkbtn"
                            disabled=move || retrying.get()
                            on:click=move |_| retry(app, chat, retrying)
                        >
                            {move || if retrying.get() { "Retrying…" } else { "Retry" }}
                        </button>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}
