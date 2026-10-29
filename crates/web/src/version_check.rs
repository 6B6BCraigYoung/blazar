use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api;
use crate::components::toast;
use crate::storage;

const SEEN: &str = "blazar.update.seen";
const LATER_MS: u32 = 20_000;

pub fn install() {
    spawn_local(async {
        mismatch().await;
        newer().await;
    });
}

async fn mismatch() {
    if !crate::desktop::remote_hub().await {
        return;
    }
    let (Ok(mode), Ok(about)) = (
        crate::desktop::invoke("hub_mode", &json!({})).await,
        api::get::<Value>("/api/about").await,
    ) else {
        return;
    };
    let (app, hub) = (
        mode["app"].as_str().unwrap_or_default(),
        about["version"].as_str().unwrap_or_default(),
    );
    if app.is_empty() || hub.is_empty() || app == hub {
        return;
    }
    toast::offer(
        format!(
            "这台电脑上的 Blazar 是 {app}，连着的 hub 是 {hub}。版本不一样时有的功能用不了，两边升到同一个版本最稳。"
        ),
        "知道了",
        LATER_MS,
        || {},
    );
}

async fn newer() {
    let Ok(u) = api::get::<Value>("/api/update").await else {
        return;
    };
    if !u["newer"].as_bool().unwrap_or(false) {
        return;
    }
    let latest = u["latest"].as_str().unwrap_or_default().to_owned();
    if storage::load::<String>(SEEN).as_deref() == Some(latest.as_str()) {
        return;
    }
    storage::save(SEEN, &latest);
    let url = u["url"].as_str().unwrap_or_default().to_owned();
    toast::offer(
        format!("Blazar {latest} 可以更新了"),
        "去下载",
        LATER_MS,
        move || {
            let _ = window().open_with_url_and_target(&url, "_blank");
        },
    );
}
