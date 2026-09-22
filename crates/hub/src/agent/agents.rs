use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::api::Shared;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Starter {
    pub label: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub runtime: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub color: String,

    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub starters: Vec<Starter>,

    #[serde(default)]
    pub thinking_level: Option<String>,

    #[serde(default)]
    pub custom_args: Vec<String>,
    #[serde(default = "one")]
    pub max_concurrent: i64,
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

const fn one() -> i64 {
    1
}

const PERMISSION_MODES: &[&str] = &[
    "acceptEdits",
    "default",
    "auto",
    "plan",
    "bypassPermissions",
    "read-only",
    "workspace-write",
    "danger-full-access",
];

const PROTECTED_ARGS: &[&str] = &[
    "-p",
    "--print",
    "--output-format",
    "--input-format",
    "--mcp-config",
    "--strict-mcp-config",
    "--permission-prompt-tool",
    "--resume",
    "--continue",
    "--session-id",
    "--verbose",
    "--json",
    "-C",
    "--cwd",
];

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

fn validate(a: &Agent) -> Result<(), String> {
    let name = a.name.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err("名字 1–40 个字".into());
    }
    if blazar_runtime::spec::find(&a.runtime).is_none() {
        return Err(format!("未知运行时 {}", a.runtime));
    }
    if a.description.chars().count() > 255 {
        return Err("职责描述不超过 255 字".into());
    }
    if a.instructions.chars().count() > 8000 {
        return Err("角色说明不超过 8000 字".into());
    }
    if let Some(m) = &a.permission_mode
        && !m.is_empty()
        && !PERMISSION_MODES.contains(&m.as_str())
    {
        return Err(format!("未知权限模式 {m}"));
    }
    if let Some(m) = &a.model
        && (m.chars().count() > 80 || m.chars().any(|c| c.is_whitespace() || c.is_control()))
    {
        return Err("模型名不能含空白".into());
    }
    if let Some(t) = &a.thinking_level
        && !t.is_empty()
        && !blazar_runtime::cli_runtime::EFFORTS.contains(&t.as_str())
    {
        return Err(format!("未知推理强度 {t}"));
    }
    for k in a.env.keys() {
        let ok = !k.is_empty()
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !k.starts_with(|c: char| c.is_ascii_digit());
        if !ok {
            return Err(format!("环境变量名 `{k}` 不合法"));
        }
    }
    if a.color.len() > 16
        || a.color
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '#'))
    {
        return Err("颜色值不合法".into());
    }
    if a.avatar.chars().count() > 4 || a.avatar.chars().any(char::is_control) {
        return Err("头像是一个 emoji".into());
    }
    if a.starters.len() > 3 {
        return Err("开场白最多 3 条".into());
    }
    for s in &a.starters {
        if s.label.trim().is_empty() || s.label.chars().count() > 80 {
            return Err("开场白标题 1–80 字".into());
        }
        if s.prompt.trim().is_empty() || s.prompt.chars().count() > 4000 {
            return Err("开场白内容 1–4000 字".into());
        }
    }
    for arg in &a.custom_args {
        if arg.chars().any(char::is_control) || arg.chars().count() > 400 {
            return Err(format!("参数不合法：{arg}"));
        }
        let head = arg.split('=').next().unwrap_or(arg);
        if PROTECTED_ARGS.contains(&head) {
            return Err(format!("参数 {head} 由 Blazar 管理，不能自定义"));
        }
    }
    if !(1..=50).contains(&a.max_concurrent) {
        return Err("并发数 1–50".into());
    }
    Ok(())
}

fn normalize(mut a: Agent) -> Agent {
    a.name = a.name.trim().to_owned();
    a.description = a.description.trim().to_owned();
    a.avatar = a.avatar.trim().to_owned();
    a.model = a
        .model
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    a.permission_mode = a.permission_mode.filter(|m| !m.is_empty());
    a.thinking_level = a
        .thinking_level
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    a.starters = a
        .starters
        .into_iter()
        .map(|s| Starter {
            label: s.label.trim().to_owned(),
            prompt: s.prompt.trim().to_owned(),
        })
        .filter(|s| !s.label.is_empty() || !s.prompt.is_empty())
        .collect();
    a.custom_args = a
        .custom_args
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    a
}

fn from_row(r: &sqlx::sqlite::SqliteRow) -> Agent {
    let json = |col: &str| r.try_get::<String, _>(col).ok();
    Agent {
        id: r.try_get("id").unwrap_or_default(),
        name: r.try_get("name").unwrap_or_default(),
        runtime: r.try_get("runtime").unwrap_or_default(),
        description: r.try_get("description").unwrap_or_default(),
        instructions: r.try_get("instructions").unwrap_or_default(),
        model: r.try_get("model").ok().flatten(),
        permission_mode: r.try_get("permission_mode").ok().flatten(),
        env: json("custom_env")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        color: r.try_get("color").unwrap_or_default(),
        avatar: r.try_get("avatar").unwrap_or_default(),
        starters: json("starters")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        thinking_level: r.try_get("thinking_level").ok().flatten(),
        custom_args: json("custom_args")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        max_concurrent: r.try_get("max_concurrent").unwrap_or(1),
        archived_at: r.try_get("archived_at").ok().flatten(),
        created_at: r.try_get("created_at").unwrap_or_default(),
        updated_at: r.try_get("updated_at").unwrap_or_default(),
    }
}

pub async fn get(st: &Shared, id: &str) -> Option<Agent> {
    sqlx::query("SELECT * FROM agents WHERE id = ?1")
        .bind(id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| from_row(&r))
}

#[derive(Debug, Default, Serialize)]
pub struct Activity {
    pub running: i64,
    pub runs_30d: i64,
    pub succeeded_30d: i64,
    pub failed_30d: i64,

    pub cancelled_30d: i64,
    pub cost_30d: f64,
    pub last_used_at: Option<String>,

    pub avg_secs_30d: Option<f64>,

    pub activity_7d: Vec<i64>,
    pub failed_7d: i64,
}

async fn activity(st: &Shared, id: &str) -> Activity {
    let since = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    let row = sqlx::query(
        "SELECT COUNT(*) AS n,
                SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END) AS running,
                SUM(CASE WHEN status = 'done' THEN 1 ELSE 0 END) AS ok,
                SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS bad,
                SUM(CASE WHEN status = 'interrupted' THEN 1 ELSE 0 END) AS cancelled,
                MAX(created_at) AS last
         FROM sessions WHERE agent_profile = ?1 AND created_at >= ?2",
    )
    .bind(id)
    .bind(&since)
    .fetch_one(st.db.pool())
    .await
    .ok();
    let running_all: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions WHERE agent_profile = ?1 AND status = 'running'",
    )
    .bind(id)
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0);
    let last: Option<String> =
        sqlx::query_scalar("SELECT MAX(created_at) FROM sessions WHERE agent_profile = ?1")
            .bind(id)
            .fetch_one(st.db.pool())
            .await
            .ok()
            .flatten();
    let cost: f64 = sqlx::query_scalar::<_, String>(
        "SELECT e.payload FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE s.agent_profile = ?1 AND s.created_at >= ?2 AND e.payload LIKE '%token_usage%'",
    )
    .bind(id)
    .bind(&since)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .filter_map(|p| serde_json::from_str::<serde_json::Value>(p).ok())
    .filter_map(|v| v.get("cost_usd").and_then(serde_json::Value::as_f64))
    .sum();
    let get = |k: &str| {
        row.as_ref()
            .and_then(|r| r.try_get::<Option<i64>, _>(k).ok().flatten())
            .unwrap_or(0)
    };

    let avg_secs: Option<f64> = sqlx::query_scalar(
        "SELECT AVG(d) FROM (
           SELECT (julianday(MAX(e.ts)) - julianday(MIN(e.ts))) * 86400.0 AS d
           FROM sessions s JOIN events e ON e.session_id = s.id
           WHERE s.agent_profile = ?1 AND s.created_at >= ?2 AND s.status IN ('done','failed')
           GROUP BY s.id)",
    )
    .bind(id)
    .bind(&since)
    .fetch_one(st.db.pool())
    .await
    .ok()
    .flatten();

    let today = Utc::now().date_naive();
    let week_ago = (Utc::now() - chrono::Duration::days(6)).date_naive();
    let per_day: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT substr(created_at, 1, 10) AS day, COUNT(*),
                SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END)
         FROM sessions WHERE agent_profile = ?1 AND substr(created_at, 1, 10) >= ?2
         GROUP BY day",
    )
    .bind(id)
    .bind(week_ago.to_string())
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut activity_7d = vec![0_i64; 7];
    let mut failed_7d = 0;
    for (day, n, bad) in per_day {
        if let Ok(d) = day.parse::<chrono::NaiveDate>() {
            let idx = 6 - (today - d).num_days();
            if (0..7).contains(&idx) {
                activity_7d[usize::try_from(idx).unwrap_or(0)] = n;
                failed_7d += bad;
            }
        }
    }
    Activity {
        running: running_all,
        runs_30d: row
            .as_ref()
            .and_then(|r| r.try_get::<i64, _>("n").ok())
            .unwrap_or(0),
        succeeded_30d: get("ok"),
        failed_30d: get("bad"),
        cancelled_30d: get("cancelled"),
        cost_30d: cost,
        last_used_at: last,
        avg_secs_30d: avg_secs,
        activity_7d,
        failed_7d,
    }
}

#[derive(Serialize)]
struct AgentView {
    #[serde(flatten)]
    agent: Agent,
    runtime_label: String,

    status: &'static str,
    #[serde(flatten)]
    activity: Activity,
}

async fn view(st: &Shared, agent: Agent) -> AgentView {
    let activity = activity(st, &agent.id).await;
    let status = if agent.archived_at.is_some() {
        "archived"
    } else if activity.running > 0 {
        "working"
    } else {
        "idle"
    };
    AgentView {
        runtime_label: blazar_runtime::spec::find(&agent.runtime)
            .map_or_else(|| agent.runtime.clone(), |s| s.label.to_owned()),
        status,
        activity,
        agent,
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub scope: String,
}

pub async fn list(State(st): State<Shared>, Query(q): Query<ListQuery>) -> Response {
    let sql = match q.scope.as_str() {
        "archived" => {
            "SELECT * FROM agents WHERE archived_at IS NOT NULL ORDER BY archived_at DESC"
        }
        "all" => "SELECT * FROM agents ORDER BY created_at",
        _ => "SELECT * FROM agents WHERE archived_at IS NULL ORDER BY created_at",
    };
    let rows = match sqlx::query(sql).fetch_all(st.db.pool()).await {
        Ok(r) => r,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(view(&st, from_row(r)).await);
    }
    Json(out).into_response()
}

pub async fn detail(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(agent) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个 Agent（可能已被删除）");
    };
    let runs = sqlx::query(
        "SELECT s.id, s.workspace_id, w.name AS workspace, s.status, s.created_at, s.title,
                COALESCE(s.thread_id, s.id) AS thread,
                (SELECT COUNT(*) FROM events e WHERE e.session_id = s.id) AS events,
                (SELECT MAX(e.ts) FROM events e WHERE e.session_id = s.id) AS last_at
         FROM sessions s LEFT JOIN workspaces w ON w.id = s.workspace_id
         WHERE s.agent_profile = ?1 ORDER BY s.created_at DESC LIMIT 30",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let runs: Vec<serde_json::Value> = runs
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.try_get::<String, _>("id").unwrap_or_default(),
                "thread": r.try_get::<String, _>("thread").unwrap_or_default(),
                "workspace_id": r.try_get::<String, _>("workspace_id").unwrap_or_default(),
                "workspace": r.try_get::<Option<String>, _>("workspace").unwrap_or(None),
                "status": r.try_get::<String, _>("status").unwrap_or_default(),
                "created_at": r.try_get::<String, _>("created_at").unwrap_or_default(),
                "last_at": r.try_get::<Option<String>, _>("last_at").unwrap_or(None),
                "events": r.try_get::<i64, _>("events").unwrap_or(0),
                "title": r.try_get::<Option<String>, _>("title").unwrap_or(None),
            })
        })
        .collect();
    let v = view(&st, agent).await;
    let mut j = serde_json::to_value(v).unwrap_or_default();
    j["runs"] = serde_json::Value::Array(runs);
    Json(j).into_response()
}

async fn save(st: &Shared, a: &Agent, create: bool) -> Result<(), Response> {
    let now = Utc::now().to_rfc3339();
    let env = serde_json::to_string(&a.env).unwrap_or_else(|_| "{}".into());
    let starters = serde_json::to_string(&a.starters).unwrap_or_else(|_| "[]".into());
    let args = serde_json::to_string(&a.custom_args).unwrap_or_else(|_| "[]".into());
    let res = if create {
        sqlx::query(
            "INSERT INTO agents (id, name, runtime, description, instructions, model, permission_mode,
                                 custom_env, color, avatar, starters, thinking_level, custom_args,
                                 max_concurrent, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
        )
        .bind(&a.id)
        .bind(&a.name)
        .bind(&a.runtime)
        .bind(&a.description)
        .bind(&a.instructions)
        .bind(&a.model)
        .bind(&a.permission_mode)
        .bind(&env)
        .bind(&a.color)
        .bind(&a.avatar)
        .bind(&starters)
        .bind(&a.thinking_level)
        .bind(&args)
        .bind(a.max_concurrent)
        .bind(&now)
        .execute(st.db.pool())
        .await
    } else {
        sqlx::query(
            "UPDATE agents SET name = ?2, runtime = ?3, description = ?4, instructions = ?5,
                    model = ?6, permission_mode = ?7, custom_env = ?8, color = ?9, avatar = ?10,
                    starters = ?11, thinking_level = ?12, custom_args = ?13, max_concurrent = ?14,
                    updated_at = ?15
             WHERE id = ?1",
        )
        .bind(&a.id)
        .bind(&a.name)
        .bind(&a.runtime)
        .bind(&a.description)
        .bind(&a.instructions)
        .bind(&a.model)
        .bind(&a.permission_mode)
        .bind(&env)
        .bind(&a.color)
        .bind(&a.avatar)
        .bind(&starters)
        .bind(&a.thinking_level)
        .bind(&args)
        .bind(a.max_concurrent)
        .bind(&now)
        .execute(st.db.pool())
        .await
    };
    match res {
        Ok(r) if !create && r.rows_affected() == 0 => {
            Err(fail(StatusCode::NOT_FOUND, "没有这个 Agent"))
        }
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("UNIQUE") => Err(fail(
            StatusCode::CONFLICT,
            format!("已经有叫「{}」的 Agent 了", a.name),
        )),
        Err(e) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

pub async fn create(State(st): State<Shared>, Json(a): Json<Agent>) -> Response {
    let mut a = normalize(a);
    if let Err(e) = validate(&a) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    a.id = uuid::Uuid::now_v7().to_string();
    a.archived_at = None;
    match save(&st, &a, true).await {
        Ok(()) => Json(view(&st, a).await).into_response(),
        Err(r) => r,
    }
}

pub async fn update(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(a): Json<Agent>,
) -> Response {
    let mut a = normalize(a);
    a.id = id;
    if let Err(e) = validate(&a) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    match save(&st, &a, false).await {
        Ok(()) => match get(&st, &a.id).await {
            Some(fresh) => Json(view(&st, fresh).await).into_response(),
            None => fail(StatusCode::NOT_FOUND, "没有这个 Agent"),
        },
        Err(r) => r,
    }
}

async fn cancel_active(st: &Shared, id: &str) -> usize {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE agent_profile = ?1 AND status = 'running'",
    )
    .bind(id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut n = 0;
    for sid in ids {
        let Ok(u) = sid.parse() else { continue };
        let sid = blazar_core_types::SessionId(u);
        let live = st
            .running
            .read()
            .await
            .get(&sid)
            .and_then(|r| r.live.clone());
        if let Some(live) = live {
            crate::run::interrupt(st, live, sid).await;
            n += 1;
        }
    }
    n
}

pub async fn cancel_runs(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    if get(&st, &id).await.is_none() {
        return fail(StatusCode::NOT_FOUND, "没有这个 Agent");
    }
    let n = cancel_active(&st, &id).await;
    Json(serde_json::json!({ "cancelled": n })).into_response()
}

pub async fn archive(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个 Agent");
    };
    if a.archived_at.is_some() {
        return fail(StatusCode::CONFLICT, "已经归档了");
    }
    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query("UPDATE agents SET archived_at = ?2, updated_at = ?2 WHERE id = ?1")
        .bind(&id)
        .bind(&now)
        .execute(st.db.pool())
        .await;
    let cancelled = cancel_active(&st, &id).await;
    Json(serde_json::json!({ "ok": true, "archived_at": now, "cancelled": cancelled }))
        .into_response()
}

pub async fn restore(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个 Agent");
    };
    if a.archived_at.is_none() {
        return fail(StatusCode::CONFLICT, "没有归档，不用恢复");
    }
    let now = Utc::now().to_rfc3339();
    match sqlx::query("UPDATE agents SET archived_at = NULL, updated_at = ?2 WHERE id = ?1")
        .bind(&id)
        .bind(&now)
        .execute(st.db.pool())
        .await
    {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) if e.to_string().contains("UNIQUE") => fail(
            StatusCode::CONFLICT,
            format!("已经有叫「{}」的 Agent 了，先把它改名", a.name),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn duplicate(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(src) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个 Agent");
    };
    let mut a = src.clone();
    a.id = uuid::Uuid::now_v7().to_string();
    a.archived_at = None;
    a.env = BTreeMap::new();

    let base = format!("{} (副本)", src.name);
    a.name = base.clone();
    for n in 2..20 {
        if save(&st, &a, true).await.is_ok() {
            return Json(view(&st, a).await).into_response();
        }
        a.name = format!("{base} {n}");
    }
    fail(StatusCode::CONFLICT, "起不出不重名的名字了")
}

pub async fn delete(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match get(&st, &id).await {
        None => return fail(StatusCode::NOT_FOUND, "没有这个 Agent"),
        Some(a) if a.archived_at.is_none() => {
            return fail(
                StatusCode::CONFLICT,
                "先归档再删除：归档后历史还在，可以恢复",
            );
        }
        Some(_) => {}
    }
    match sqlx::query("DELETE FROM agents WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a() -> Agent {
        Agent {
            id: String::new(),
            name: "代码审查员".into(),
            runtime: "claude".into(),
            description: String::new(),
            instructions: "只提意见".into(),
            model: None,
            permission_mode: Some("plan".into()),
            env: BTreeMap::new(),
            color: "#6d5efc".into(),
            avatar: "🔍".into(),
            starters: vec![Starter {
                label: "审查改动".into(),
                prompt: "看一下 git diff".into(),
            }],
            thinking_level: Some("high".into()),
            custom_args: vec!["--no-memory".into()],
            max_concurrent: 2,
            archived_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn validation() {
        assert!(validate(&a()).is_ok());
        let mut x = a();
        x.runtime = "nope".into();
        assert!(validate(&x).is_err());
        let mut x = a();
        x.permission_mode = Some("yolo".into());
        assert!(validate(&x).is_err());
        let mut x = a();
        x.env.insert("BAD KEY".into(), "v".into());
        assert!(validate(&x).is_err());
        let mut x = a();
        x.model = Some("opus; rm -rf".into());
        assert!(validate(&x).is_err(), "模型名会进命令行参数，不能有空白");
        let mut x = a();
        x.name = "   ".into();
        assert!(validate(&normalize(x)).is_err());
        let mut x = a();
        x.thinking_level = Some("turbo".into());
        assert!(validate(&x).is_err());
        let mut x = a();
        x.custom_args = vec!["--output-format=json".into()];
        assert!(validate(&x).is_err(), "协议参数不能被自定义参数覆盖");
        let mut x = a();
        x.starters = vec![Starter::default(); 4];
        assert!(validate(&x).is_err());
        let mut x = a();
        x.max_concurrent = 0;
        assert!(validate(&x).is_err());
    }
}
