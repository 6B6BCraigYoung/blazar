use std::collections::BTreeMap;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use blazar_core_types::transfer::{
    AgentSpec, AutopilotSpec, BotSpec, ExportFile, FORMAT, HookSpec, ImportReport, McpSpec,
    MemorySpec, NodeSpec, RuleSpec, SkillSpec, SnippetSpec, VERSION, WorkspaceSpec,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::Shared;

mod apply;
mod export;
mod plan;

pub use apply::{apply, import};
pub use export::export_from;
pub use plan::plan;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn refuse_bots(headers: &HeaderMap) -> Option<Response> {
    crate::bots::caller_of(headers).map(|_| fail(StatusCode::FORBIDDEN, "bot 不能导出或导入配置"))
}

fn blank(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.into_keys().map(|k| (k, String::new())).collect()
}

fn keys(raw: &str) -> BTreeMap<String, String> {
    blank(serde_json::from_str(raw).unwrap_or_default())
}

pub fn parse(bytes: &[u8]) -> Result<ExportFile, String> {
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("文件不是合法的 JSON：{e}"))?;
    if raw["format"].as_str() != Some(FORMAT) {
        return Err("这不是 Blazar 导出的配置文件".into());
    }
    match raw["version"].as_u64() {
        Some(VERSION) => {}
        Some(v) => {
            return Err(format!(
                "导出文件的格式版本是 {v}，这个 hub 只认 {VERSION}；请升级 Blazar 后再导入"
            ));
        }
        None => return Err("导出文件缺少格式版本".into()),
    }
    serde_json::from_value(raw).map_err(|e| format!("导出文件内容不完整：{e}"))
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub local_node: Option<String>,
    pub self_node: Option<String>,
    pub bot_home: Option<std::path::PathBuf>,
}

enum Op {
    Prefs(Value),
    Node(NodeSpec),
    Project(String),
    Workspace(WorkspaceSpec),
    Skill(SkillSpec),
    Mcp(McpSpec),
    Agent(AgentSpec),
    Snippet(SnippetSpec),
    Hook(HookSpec),
    Rule(RuleSpec),
    Autopilot(AutopilotSpec),
    Bot(BotSpec),
    Memory(MemorySpec),
}

pub struct Plan {
    pub report: ImportReport,
    ops: Vec<Op>,
    bot_home: Option<std::path::PathBuf>,
}

fn agent_of(a: &AgentSpec) -> Result<crate::agents::Agent, String> {
    let agent = crate::agents::normalize(crate::agents::Agent {
        id: String::new(),
        name: a.name.clone(),
        runtime: a.runtime.clone(),
        description: a.description.clone(),
        instructions: a.instructions.clone(),
        model: a.model.clone(),
        permission_mode: a.permission_mode.clone(),
        env: blank(a.custom_env.clone()),
        color: a.color.clone(),
        avatar: a.avatar.clone(),
        starters: if a.starters.is_null() {
            Vec::new()
        } else {
            serde_json::from_value(a.starters.clone()).map_err(|_| "开场白格式不对".to_owned())?
        },
        thinking_level: a.thinking_level.clone(),
        custom_args: a.custom_args.clone(),
        max_concurrent: a.max_concurrent.unwrap_or(1),
        account: None,
        archived_at: None,
        created_at: String::new(),
        updated_at: String::new(),
    });
    crate::agents::validate(&agent)?;
    Ok(agent)
}

pub async fn export_handler(State(st): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(r) = refuse_bots(&headers) {
        return r;
    }
    let file = async {
        let mut tx = st.db.pool().begin().await?;
        export_from(&mut tx).await
    }
    .await;
    let file = match file {
        Ok(f) => f,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let body = match serde_json::to_vec_pretty(&file) {
        Ok(b) => b,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let name = format!(
        "blazar-export-{}.json",
        chrono::Local::now().format("%Y%m%d")
    );
    (
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct ImportQuery {
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub local_node: Option<String>,
    #[serde(default)]
    pub self_node: Option<String>,
}

pub async fn import_handler(
    State(st): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> Response {
    if let Some(r) = refuse_bots(&headers) {
        return r;
    }
    let file = match parse(&body) {
        Ok(f) => f,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let opts = Options {
        local_node: q.local_node,
        self_node: q.self_node,
        bot_home: None,
    };
    if let Err(e) = plan::node_names(&opts) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    match import(&st, &file, &opts, q.dry_run.unwrap_or(true)).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
