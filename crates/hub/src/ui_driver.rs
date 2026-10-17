use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_core_types::api::ServerEvent;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::api::Shared;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct UiDriver {
    enabled: bool,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
}

impl UiDriver {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("BLAZAR_UI_DRIVER").is_ok_and(|v| v == "1"),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn register(&self) -> (u64, oneshot::Receiver<Value>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id, tx);
        }
        (id, rx)
    }

    fn forget(&self, id: u64) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
    }

    fn resolve(&self, id: u64, value: Value) -> bool {
        let tx = self.pending.lock().ok().and_then(|mut p| p.remove(&id));
        tx.is_some_and(|tx| tx.send(value).is_ok())
    }
}

fn timeout_of(command: &Value) -> Duration {
    command
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_TIMEOUT, |ms| {
            Duration::from_millis(ms).min(MAX_TIMEOUT)
        })
}

pub async fn command(State(st): State<Shared>, Json(command): Json<Value>) -> Response {
    let driver = &st.ui_driver;
    if !driver.enabled() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "界面驱动没有开启" })),
        )
            .into_response();
    }
    if !command.get("action").is_some_and(Value::is_string) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少 action" })),
        )
            .into_response();
    }
    if st.bus.receiver_count() == 0 {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "没有打开的页面" })),
        )
            .into_response();
    }
    let wait = timeout_of(&command);
    let (id, rx) = driver.register();
    st.emit(ServerEvent::UiCommand { id, command });
    match tokio::time::timeout(wait, rx).await {
        Ok(Ok(value)) => Json(value).into_response(),
        _ => {
            driver.forget(id);
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({ "error": "页面没有在限定时间内回应" })),
            )
                .into_response()
        }
    }
}

pub async fn result(
    State(st): State<Shared>,
    Path(id): Path<u64>,
    Json(value): Json<Value>,
) -> StatusCode {
    if st.ui_driver.enabled() && st.ui_driver.resolve(id, value) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_reach_only_the_waiting_command() {
        let driver = UiDriver {
            enabled: true,
            ..UiDriver::default()
        };
        let (id, mut rx) = driver.register();
        assert!(!driver.resolve(id + 1, json!(1)));
        assert!(driver.resolve(id, json!({ "ok": true })));
        assert_eq!(rx.try_recv().unwrap(), json!({ "ok": true }));
        assert!(!driver.resolve(id, json!(2)));
    }

    #[test]
    fn timeouts_are_capped() {
        assert_eq!(timeout_of(&json!({})), DEFAULT_TIMEOUT);
        assert_eq!(
            timeout_of(&json!({ "timeout_ms": 500 })),
            Duration::from_millis(500)
        );
        assert_eq!(timeout_of(&json!({ "timeout_ms": 9_999_999 })), MAX_TIMEOUT);
    }
}
