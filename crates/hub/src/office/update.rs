use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde_json::{Value, json};

use super::{kv_get, kv_put};
use crate::api::Shared;

const REPO: Option<&str> = option_env!("BLAZAR_RELEASE_REPO");
const KEY: &str = "update.latest";
const EVERY: chrono::Duration = chrono::Duration::hours(12);

fn version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split(['.', '-']);
    let mut next = || parts.next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

#[must_use]
pub fn newer(latest: &str, current: &str) -> bool {
    matches!((version(latest), version(current)), (Some(l), Some(c)) if l > c)
}

async fn fetch(repo: &str) -> Result<Value, String> {
    let release: Value = crate::agent::remote_cli::http()?
        .get(format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        .header("User-Agent", "blazar")
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))?;
    Ok(json!({
        "at": Utc::now().to_rfc3339(),
        "version": release["tag_name"].as_str().unwrap_or_default().trim_start_matches('v'),
        "url": release["html_url"],
    }))
}

pub async fn update(State(st): State<Shared>) -> Response {
    let current = env!("CARGO_PKG_VERSION");
    let Some(repo) = REPO.filter(|_| std::env::var_os("BLAZAR_NO_UPDATE_CHECK").is_none()) else {
        return Json(json!({ "current": current, "checks": false })).into_response();
    };
    let mut seen = kv_get(&st, KEY).await;
    let stale = seen["at"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_none_or(|t| Utc::now() - t.with_timezone(&Utc) > EVERY);
    if stale && let Ok(fresh) = fetch(repo).await {
        let _ = kv_put(&st, KEY, &fresh).await;
        seen = fresh;
    }
    let latest = seen["version"].as_str().unwrap_or_default();
    Json(json!({
        "current": current,
        "checks": true,
        "latest": (!latest.is_empty()).then_some(latest),
        "newer": newer(latest, current),
        "url": seen["url"],
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_release_tags_with_the_running_version() {
        assert!(newer("v0.1.2", "0.1.1"));
        assert!(newer("0.2.0", "0.1.9"));
        assert!(!newer("0.1.1", "0.1.1"));
        assert!(!newer("0.1.0", "0.1.1"));
        assert!(!newer("", "0.1.1"));
        assert!(!newer("nightly", "0.1.1"));
    }
}
