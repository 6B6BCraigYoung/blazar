use leptos::prelude::*;
use serde_json::{Value, json};

use crate::app_state::use_app;
use crate::components::toast::toast;

use super::state::Chat;

fn reason(status: &str) -> &'static str {
    match status {
        "offline" => "机器离线",
        other => crate::vocab::state(other).0,
    }
}

#[component]
pub fn Offline(chat: Chat) -> impl IntoView {
    let app = use_app();
    let node = chat.node.get_value();
    let machine = crate::fmt::machine(&node);
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
            .map(|s| crate::fmt::ago(&s))
            .filter(|s| !s.is_empty())
            .map(|s| format!("上次在线 {s}"));
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
                    "还是连不上：{}",
                    crate::vocab::error(&e.to_string(), None)
                ));
            }
        });
    };
    view! {
        <div class="blank cc-offline" role="status">
            <h2 class="blank-title">{format!("{machine} 连不上")}</h2>
            <p class="blank-detail">{sub}</p>
            <div class="cc-offline-acts">
                <button
                    type="button"
                    class="btn primary"
                    disabled=move || retrying.get()
                    on:click=move |_| retry()
                >
                    {move || if retrying.get() { "正在重试…" } else { "重试" }}
                </button>
                <a class="btn" href=href>
                    "打开机器页"
                </a>
            </div>
        </div>
    }
}
