use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State, WebSocketUpgrade, ws};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_core_types::{NormalizedEntry, ProviderSessionId, SessionId, WorkspaceId};
use blazar_runtime::{CliRuntime, SessionSpec};
use blazar_vfs::Vfs;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::state::{AppState, RunningSession, ServerEvent};

pub type Shared = Arc<AppState>;

pub struct ApiError(anyhow::Error);

impl ApiError {
    #[must_use]
    pub fn message(&self) -> String {
        format!("{:#}", self.0)
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if matches!(
            self.0.downcast_ref::<blazar_vfs::VfsError>(),
            Some(blazar_vfs::VfsError::PathEscape(_))
        ) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": self.0.to_string() })),
            )
                .into_response();
        }
        tracing::error!("API 错误: {:#}", self.0);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

pub(crate) type ApiResult<T> = std::result::Result<T, ApiError>;

mod agent_config;
mod browse;
mod context;
mod hooks;
mod lifecycle;
mod nodes;
mod profiles;
mod search;
mod sessions;
mod terminal;
mod usage;
mod view;
mod websocket;
mod workspaces;
pub use agent_config::*;
pub use browse::*;
pub use context::*;
pub use hooks::*;
pub use lifecycle::*;
pub use nodes::*;
pub use profiles::*;
pub use search::*;
pub use sessions::*;
pub use terminal::*;
pub use usage::*;
pub use view::*;
pub use websocket::*;
pub use workspaces::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(node: Option<&str>) -> AgentConfigView {
        AgentConfigView {
            id: "x".into(),
            node: node.map(str::to_owned),
            agent_id: "claude".into(),
            display_name: None,
            program_path: None,
            model: None,
            permission_mode: None,
            custom_args: Vec::new(),
            custom_env: Default::default(),
            max_concurrent: 2,
            enabled: true,
        }
    }

    #[test]
    fn node_config_overrides_by_key_not_wholesale() {
        let mut global = cfg(None);
        global.model = Some("sonnet".into());
        global
            .custom_env
            .insert("HTTPS_PROXY".into(), "http://proxy.internal:7890".into());
        global.custom_args = vec!["--fallback-model".into(), "haiku".into()];

        let mut node = cfg(Some("gpu-1"));
        node.model = Some("opus".into());
        node.custom_env
            .insert("CUDA_VISIBLE_DEVICES".into(), "0".into());

        let m = merge_agent_config(global, node);
        assert_eq!(m.model.as_deref(), Some("opus"), "节点级的标量要覆盖全局");
        assert_eq!(
            m.custom_env.get("HTTPS_PROXY").map(String::as_str),
            Some("http://proxy.internal:7890"),
            "全局的出口代理不能因为节点有自己的配置就丢了"
        );
        assert!(m.custom_env.contains_key("CUDA_VISIBLE_DEVICES"));
        assert_eq!(
            m.custom_args,
            vec!["--fallback-model", "haiku"],
            "全局参数要保留"
        );
    }

    #[test]
    fn unset_node_fields_fall_back_to_global() {
        let mut global = cfg(None);
        global.permission_mode = Some("acceptEdits".into());
        let m = merge_agent_config(global, cfg(Some("gpu1")));
        assert_eq!(m.permission_mode.as_deref(), Some("acceptEdits"));
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;

    #[test]
    fn editor_context_is_appended_but_not_to_commands() {
        let t = with_editor_context("修一下", Some("src/main.rs"));
        assert!(t.starts_with("修一下") && t.contains("`src/main.rs`"));
        assert_eq!(with_editor_context("/context", Some("a.rs")), "/context");
        assert_eq!(with_editor_context("hi", Some("a\nb")), "hi");
        assert_eq!(with_editor_context("hi", None), "hi");
    }

    #[test]
    fn control_requests_map_and_validate() {
        let r = control_requests(ControlRequest {
            output_style: Some("Learning".into()),
            fast_mode: Some(true),
            thinking: Some(false),
            mcp_reconnect: Some("github".into()),
            ..Default::default()
        })
        .unwrap();
        let kinds: Vec<_> = r.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, ["settings", "thinking", "mcp_reconnect"]);
        assert_eq!(r[0].1["settings"]["outputStyle"], "Learning");
        assert_eq!(r[0].1["settings"]["fastMode"], true);
        assert_eq!(r[1].1["max_thinking_tokens"], 0);
        let on = control_requests(ControlRequest {
            thinking: Some(true),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(on[0].1["settings"]["alwaysThinkingEnabled"], true);
        assert!(on[1].1["max_thinking_tokens"].is_null());
        assert!(
            control_requests(ControlRequest {
                output_style: Some("a\u{7}b".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            control_requests(ControlRequest {
                mcp_toggle: Some(McpToggle {
                    name: String::new(),
                    enabled: false
                }),
                ..Default::default()
            })
            .is_err()
        );
    }
}
