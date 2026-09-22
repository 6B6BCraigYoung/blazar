use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_core_types::SessionId;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;
use crate::state::ServerEvent;

pub const STATUSES: &[&str] = &[
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "done",
    "cancelled",
];
pub const PRIORITIES: &[&str] = &["urgent", "high", "medium", "low", "none"];

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn changed(st: &Shared) {
    st.emit(ServerEvent::TasksChanged);
}

const SELECT: &str = "SELECT t.*, w.name AS workspace_name, n.name AS node,
        a.name AS agent_name, a.avatar AS agent_avatar, a.runtime AS agent_runtime,
        (SELECT COUNT(*) FROM tasks c WHERE c.parent_id = t.id) AS children,
        (SELECT COUNT(*) FROM tasks c WHERE c.parent_id = t.id AND c.status IN ('done','cancelled')) AS children_done,
        (SELECT COUNT(*) FROM sessions s WHERE t.thread_id IS NOT NULL
            AND COALESCE(s.thread_id, s.id) = t.thread_id AND s.status = 'running') AS running,
        (SELECT COUNT(*) FROM task_comments c WHERE c.task_id = t.id) AS comments
    FROM tasks t
    LEFT JOIN workspaces w ON w.id = t.workspace_id
    LEFT JOIN nodes n ON n.id = w.node_id
    LEFT JOIN agents a ON a.id = t.agent_profile";

fn row_json(r: &sqlx::sqlite::SqliteRow) -> Value {
    let s = |k: &str| r.try_get::<Option<String>, _>(k).unwrap_or(None);
    let n = |k: &str| r.try_get::<i64, _>(k).unwrap_or(0);
    let number = n("number");
    json!({
        "id": s("id"), "number": number, "key": format!("BLZ-{number}"),
        "title": s("title"), "description": s("description"),
        "status": s("status"), "priority": s("priority"),
        "labels": s("labels").and_then(|l| serde_json::from_str::<Value>(&l).ok()).unwrap_or_else(|| json!([])),
        "parent_id": s("parent_id"),
        "workspace_id": s("workspace_id"), "workspace_name": s("workspace_name"), "node": s("node"),
        "agent_profile": s("agent_profile"), "agent_name": s("agent_name"),
        "agent_avatar": s("agent_avatar"),
        "runtime": s("runtime").or_else(|| s("agent_runtime")),
        "thread_id": s("thread_id"),
        "position": r.try_get::<f64, _>("position").unwrap_or(0.0),
        "created_at": s("created_at"), "updated_at": s("updated_at"),
        "started_at": s("started_at"), "completed_at": s("completed_at"),
        "children": n("children"), "children_done": n("children_done"),
        "running": n("running") > 0, "comments": n("comments"),
    })
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
}

pub async fn list(State(st): State<Shared>, Query(q): Query<ListQuery>) -> Response {
    let sql = format!(
        "{SELECT} WHERE (?1 IS NULL OR t.workspace_id = ?1)
           AND (?2 IS NULL OR t.status = ?2)
           AND (?3 IS NULL OR t.parent_id = ?3)
           AND (?4 IS NULL OR t.title LIKE '%' || ?4 || '%' OR t.description LIKE '%' || ?4 || '%'
                OR ('BLZ-' || t.number) LIKE '%' || ?4 || '%')
         ORDER BY t.position, t.created_at DESC LIMIT 500"
    );
    let none_if_empty = |v: Option<String>| v.filter(|s| !s.is_empty());
    match sqlx::query(&sql)
        .bind(none_if_empty(q.workspace))
        .bind(none_if_empty(q.status))
        .bind(none_if_empty(q.parent))
        .bind(none_if_empty(q.q))
        .fetch_all(st.db.pool())
        .await
    {
        Ok(rows) => Json(rows.iter().map(row_json).collect::<Vec<_>>()).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn fetch(st: &Shared, id: &str) -> Option<Value> {
    sqlx::query(&format!("{SELECT} WHERE t.id = ?1"))
        .bind(id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| row_json(&r))
}

pub async fn detail(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(mut t) = fetch(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个任务");
    };
    let comments = sqlx::query(
        "SELECT id, author, body, note, session_id, created_at FROM task_comments
         WHERE task_id = ?1 ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    t["comment_list"] = Value::Array(
        comments
            .iter()
            .map(|r| {
                json!({
                    "id": r.try_get::<String, _>("id").unwrap_or_default(),
                    "author": r.try_get::<String, _>("author").unwrap_or_default(),
                    "body": r.try_get::<String, _>("body").unwrap_or_default(),
                    "note": r.try_get::<i64, _>("note").unwrap_or(0) == 1,
                    "session_id": r.try_get::<Option<String>, _>("session_id").unwrap_or(None),
                    "created_at": r.try_get::<String, _>("created_at").unwrap_or_default(),
                })
            })
            .collect(),
    );

    let runs = match t["thread_id"].as_str() {
        Some(thread) => sqlx::query(
            "SELECT s.id, s.status, s.runtime_kind, s.created_at,
                    (SELECT COUNT(*) FROM events e WHERE e.session_id = s.id) AS events,
                    (SELECT MIN(e.ts) FROM events e WHERE e.session_id = s.id) AS first_at,
                    (SELECT MAX(e.ts) FROM events e WHERE e.session_id = s.id) AS last_at,
                    (SELECT e.payload FROM events e WHERE e.session_id = s.id
                       AND e.payload LIKE '%token_usage%' ORDER BY e.seq DESC LIMIT 1) AS usage
             FROM sessions s WHERE COALESCE(s.thread_id, s.id) = ?1 ORDER BY s.created_at",
        )
        .bind(thread)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default(),
        None => Vec::new(),
    };
    t["runs"] = Value::Array(
        runs.iter()
            .enumerate()
            .map(|(i, r)| {
                let usage = r
                    .try_get::<Option<String>, _>("usage")
                    .unwrap_or(None)
                    .and_then(|u| serde_json::from_str::<Value>(&u).ok());
                json!({
                    "id": r.try_get::<String, _>("id").unwrap_or_default(),
                    "status": r.try_get::<String, _>("status").unwrap_or_default(),
                    "runtime": r.try_get::<String, _>("runtime_kind").unwrap_or_default(),
                    "created_at": r.try_get::<String, _>("created_at").unwrap_or_default(),
                    "first_at": r.try_get::<Option<String>, _>("first_at").unwrap_or(None),
                    "last_at": r.try_get::<Option<String>, _>("last_at").unwrap_or(None),
                    "events": r.try_get::<i64, _>("events").unwrap_or(0),
                    "cost_usd": usage.as_ref().and_then(|u| u.get("cost_usd")).cloned(),

                    "trigger": if i == 0 { "initial" } else { "follow_up" },
                })
            })
            .collect(),
    );
    let children = sqlx::query(&format!(
        "{SELECT} WHERE t.parent_id = ?1 ORDER BY t.position, t.created_at"
    ))
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    t["child_list"] = Value::Array(children.iter().map(row_json).collect());
    Json(t).into_response()
}

#[derive(Deserialize, Default)]
pub struct TaskBody {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub labels: Option<Vec<String>>,

    #[serde(default, deserialize_with = "double_option")]
    pub parent_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub workspace_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub agent_profile: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub runtime: Option<Option<String>>,
    pub position: Option<f64>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

fn check(b: &TaskBody) -> Result<(), String> {
    if let Some(t) = &b.title
        && (t.trim().is_empty() || t.chars().count() > 200)
    {
        return Err("标题 1–200 字".into());
    }
    if let Some(d) = &b.description
        && d.chars().count() > 20000
    {
        return Err("描述太长（上限 20000 字）".into());
    }
    if let Some(s) = &b.status
        && !STATUSES.contains(&s.as_str())
    {
        return Err(format!("未知状态 {s}"));
    }
    if let Some(p) = &b.priority
        && !PRIORITIES.contains(&p.as_str())
    {
        return Err(format!("未知优先级 {p}"));
    }
    if let Some(l) = &b.labels
        && (l.len() > 12
            || l.iter()
                .any(|x| x.trim().is_empty() || x.chars().count() > 30))
    {
        return Err("标签最多 12 个，每个 1–30 字".into());
    }
    if let Some(Some(r)) = &b.runtime
        && blazar_runtime::spec::find(r).is_none()
    {
        return Err(format!("未知运行时 {r}"));
    }
    Ok(())
}

pub async fn create(State(st): State<Shared>, Json(b): Json<TaskBody>) -> Response {
    if let Err(e) = check(&b) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    let Some(title) = b.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
        return fail(StatusCode::BAD_REQUEST, "标题必填");
    };
    let id = uuid::Uuid::now_v7().to_string();
    let status = b.status.clone().unwrap_or_else(|| "todo".into());
    let ts = now();

    if let Some(Some(p)) = &b.parent_id {
        let grand: Option<Option<String>> =
            sqlx::query_scalar("SELECT parent_id FROM tasks WHERE id = ?1")
                .bind(p)
                .fetch_optional(st.db.pool())
                .await
                .ok()
                .flatten();
        match grand {
            None => return fail(StatusCode::BAD_REQUEST, "父任务不存在"),
            Some(Some(_)) => return fail(StatusCode::BAD_REQUEST, "子任务只支持一层"),
            Some(None) => {}
        }
    }
    let res = sqlx::query(
        "INSERT INTO tasks (id, number, title, description, status, priority, labels, parent_id,
                            workspace_id, agent_profile, runtime, position, created_at, updated_at)
         VALUES (?1, (SELECT COALESCE(MAX(number), 0) + 1 FROM tasks), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 (SELECT COALESCE(MIN(position), 0) - 1 FROM tasks WHERE status = ?4), ?11, ?11)",
    )
    .bind(&id)
    .bind(title)
    .bind(b.description.clone().unwrap_or_default())
    .bind(&status)
    .bind(b.priority.clone().unwrap_or_else(|| "none".into()))
    .bind(serde_json::to_string(&b.labels.clone().unwrap_or_default()).unwrap_or_else(|_| "[]".into()))
    .bind(b.parent_id.clone().flatten())
    .bind(b.workspace_id.clone().flatten())
    .bind(b.agent_profile.clone().flatten())
    .bind(b.runtime.clone().flatten())
    .bind(&ts)
    .execute(st.db.pool())
    .await;
    if let Err(e) = res {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    changed(&st);
    Json(fetch(&st, &id).await.unwrap_or_default()).into_response()
}

pub async fn update(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<TaskBody>,
) -> Response {
    if let Err(e) = check(&b) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    if fetch(&st, &id).await.is_none() {
        return fail(StatusCode::NOT_FOUND, "没有这个任务");
    }
    if let Some(Some(p)) = &b.parent_id {
        if p == &id {
            return fail(StatusCode::BAD_REQUEST, "不能把任务设成自己的子任务");
        }
        let has_children: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE parent_id = ?1")
                .bind(&id)
                .fetch_one(st.db.pool())
                .await
                .unwrap_or(0);
        if has_children > 0 {
            return fail(
                StatusCode::BAD_REQUEST,
                "这个任务自己有子任务，不能再当别人的子任务",
            );
        }
    }
    let ts = now();
    macro_rules! set {
        ($col:literal, $val:expr) => {
            let _ = sqlx::query(concat!(
                "UPDATE tasks SET ",
                $col,
                " = ?2, updated_at = ?3 WHERE id = ?1"
            ))
            .bind(&id)
            .bind($val)
            .bind(&ts)
            .execute(st.db.pool())
            .await;
        };
    }
    if let Some(v) = &b.title {
        set!("title", v.trim());
    }
    if let Some(v) = &b.description {
        set!("description", v);
    }
    if let Some(v) = &b.priority {
        set!("priority", v);
    }
    if let Some(v) = &b.labels {
        set!(
            "labels",
            serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
        );
    }
    if let Some(v) = &b.parent_id {
        set!("parent_id", v.clone());
    }
    if let Some(v) = &b.workspace_id {
        set!("workspace_id", v.clone());
    }
    if let Some(v) = &b.agent_profile {
        set!("agent_profile", v.clone());
    }
    if let Some(v) = &b.runtime {
        set!("runtime", v.clone());
    }
    if let Some(v) = b.position {
        set!("position", v);
    }
    if let Some(v) = &b.status {
        set!("status", v);
        let done = matches!(v.as_str(), "done" | "cancelled");
        let _ = sqlx::query("UPDATE tasks SET completed_at = ?2 WHERE id = ?1")
            .bind(&id)
            .bind(done.then(|| ts.clone()))
            .execute(st.db.pool())
            .await;
    }
    changed(&st);
    Json(fetch(&st, &id).await.unwrap_or_default()).into_response()
}

pub async fn delete(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match sqlx::query("DELETE FROM tasks WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await
    {
        Ok(r) if r.rows_affected() == 0 => fail(StatusCode::NOT_FOUND, "没有这个任务"),
        Ok(_) => {
            changed(&st);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn add_comment(
    st: &Shared,
    task: &str,
    author: &str,
    body: &str,
    note: bool,
    session: Option<&str>,
) {
    let _ = sqlx::query(
        "INSERT INTO task_comments (id, task_id, author, body, note, session_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(task)
    .bind(author)
    .bind(body)
    .bind(i64::from(note))
    .bind(session)
    .bind(now())
    .execute(st.db.pool())
    .await;
}

async fn run(st: &Shared, task: &Value, text: &str) -> Result<Value, String> {
    run_with(st, task, text, None).await
}

async fn run_with(
    st: &Shared,
    task: &Value,
    text: &str,
    overrides: Option<&Value>,
) -> Result<Value, String> {
    let ws = task["workspace_id"]
        .as_str()
        .ok_or("先给任务选一个工作区，才知道在哪里做")?;
    let thread = task["thread_id"].as_str();
    let mut req = json!({
        "text": text,
        "resume": thread.is_some(),
        "resume_session": thread,
        "brain": "local",
        "wait_secs": 15,
    });
    if let Some(p) = task["agent_profile"].as_str() {
        req["profile"] = json!(p);
    } else if let Some(r) = task["runtime"].as_str() {
        req["agent"] = json!(r);
    }
    for k in ["model", "permission_mode"] {
        if let Some(v) = overrides
            .and_then(|o| o[k].as_str())
            .filter(|v| !v.is_empty())
        {
            req[k] = json!(v);
        }
    }
    let parsed: crate::api::PromptRequest =
        serde_json::from_value(req).map_err(|e| e.to_string())?;
    let res = crate::api::prompt(State(st.clone()), Path(ws.to_owned()), Json(parsed))
        .await
        .map_err(|e| e.message())?;
    let v = res.0;
    if v.get("admitted").and_then(Value::as_bool) == Some(false) {
        return Err(format!(
            "{}。{}",
            v["reason"].as_str().unwrap_or("工作区正忙"),
            v["hint"].as_str().unwrap_or("等它结束再开始")
        ));
    }
    if thread.is_none()
        && let (Some(id), Some(t)) = (task["id"].as_str(), v["thread_id"].as_str())
    {
        let _ = sqlx::query("UPDATE tasks SET thread_id = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(id)
            .bind(t)
            .bind(now())
            .execute(st.db.pool())
            .await;

        on_run_started(st, t).await;
    }
    Ok(v)
}

pub struct AutoTask<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub workspace: &'a str,
    pub profile: Option<&'a str>,
    pub runtime: Option<&'a str>,

    pub origin: &'a str,

    pub overrides: &'a Value,
}

pub async fn create_and_start(st: &Shared, t: AutoTask<'_>) -> Result<Value, String> {
    let AutoTask {
        title,
        description,
        workspace,
        profile,
        runtime,
        origin,
        overrides,
    } = t;
    let id = uuid::Uuid::now_v7().to_string();
    let ts = now();
    sqlx::query(
        "INSERT INTO tasks (id, number, title, description, status, priority, labels,
                            workspace_id, agent_profile, runtime, position, created_at, updated_at)
         VALUES (?1, (SELECT COALESCE(MAX(number), 0) + 1 FROM tasks), ?2, ?3, 'todo', 'none', ?4, ?5, ?6, ?7,
                 (SELECT COALESCE(MIN(position), 0) - 1 FROM tasks WHERE status = 'todo'), ?8, ?8)",
    )
    .bind(&id)
    .bind(title)
    .bind(description)
    .bind(serde_json::to_string(&["自动化", origin]).unwrap_or_else(|_| "[]".into()))
    .bind(workspace)
    .bind(profile)
    .bind(runtime.filter(|_| profile.is_none()))
    .bind(&ts)
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let task = fetch(st, &id).await.ok_or("任务刚建好就找不到了")?;
    let text = format!("{title}\n\n{description}");
    let out = run_with(st, &task, &text, Some(overrides)).await;
    changed(st);
    let mut v = out?;
    v["task_id"] = json!(id);
    Ok(v)
}

#[derive(Deserialize, Default)]
pub struct StartBody {
    #[serde(default)]
    pub text: Option<String>,
}

pub async fn start(
    State(st): State<Shared>,
    Path(id): Path<String>,
    body: Option<Json<StartBody>>,
) -> Response {
    let Some(task) = fetch(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个任务");
    };
    let extra = body.and_then(|b| b.0.text).filter(|t| !t.trim().is_empty());
    let text = match (&extra, task["thread_id"].as_str()) {
        (Some(t), _) => t.clone(),
        (None, Some(_)) => "继续这个任务。".to_owned(),
        (None, None) => {
            let d = task["description"].as_str().unwrap_or_default().trim();
            let title = task["title"].as_str().unwrap_or_default();
            if d.is_empty() {
                title.to_owned()
            } else {
                format!("{title}\n\n{d}")
            }
        }
    };
    match run(&st, &task, &text).await {
        Ok(v) => {
            changed(&st);
            Json(
                json!({ "ok": true, "session_id": v["session_id"], "thread_id": v["thread_id"],
                         "activity": v["activity"] }),
            )
            .into_response()
        }
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}

#[derive(Deserialize)]
pub struct CommentBody {
    pub body: String,

    #[serde(default)]
    pub note: bool,
}

pub async fn comment(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<CommentBody>,
) -> Response {
    let text = b.body.trim();
    if text.is_empty() || text.chars().count() > 20000 {
        return fail(StatusCode::BAD_REQUEST, "评论 1–20000 字");
    }
    let Some(task) = fetch(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个任务");
    };
    let mut sent = None;
    if !b.note {
        match run(&st, &task, text).await {
            Ok(v) => sent = v["session_id"].as_str().map(str::to_owned),
            Err(e) => return fail(StatusCode::CONFLICT, e),
        }
    }
    add_comment(&st, &id, "user", text, b.note, sent.as_deref()).await;
    changed(&st);
    Json(json!({ "ok": true, "triggered": sent.is_some(), "session_id": sent })).into_response()
}

pub async fn complete_for_workspace(st: &Shared, ws: &str, note: &str) -> u64 {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM tasks WHERE workspace_id = ?1 AND status = 'in_review'")
            .bind(ws)
            .fetch_all(st.db.pool())
            .await
            .unwrap_or_default();
    let ts = now();
    for id in &ids {
        let _ = sqlx::query(
            "UPDATE tasks SET status = 'done', completed_at = ?2, updated_at = ?2 WHERE id = ?1",
        )
        .bind(id)
        .bind(&ts)
        .execute(st.db.pool())
        .await;
        add_comment(st, id, "system", note, true, None).await;
    }
    if !ids.is_empty() {
        changed(st);
    }
    ids.len() as u64
}

pub async fn on_run_started(st: &Shared, thread: &str) {
    let ts = now();
    let r = sqlx::query(
        "UPDATE tasks SET status = 'in_progress', started_at = COALESCE(started_at, ?2),
                completed_at = NULL, updated_at = ?2
         WHERE thread_id = ?1 AND status IN ('backlog','todo','in_review')",
    )
    .bind(thread)
    .bind(&ts)
    .execute(st.db.pool())
    .await;
    if r.is_ok_and(|r| r.rows_affected() > 0) {
        changed(st);
    }
}

pub async fn on_run_finished(st: Shared, sid: SessionId, status: &'static str) {
    let task: Option<(String, String)> = sqlx::query_as(
        "SELECT t.id, t.status FROM tasks t JOIN sessions s ON COALESCE(s.thread_id, s.id) = t.thread_id
         WHERE s.id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let Some((task, cur)) = task else {
        return;
    };

    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_id = ?1
           AND (payload LIKE '%assistant_message%' OR payload LIKE '%\"finished\"%')
         ORDER BY seq DESC LIMIT 6",
    )
    .bind(sid.to_string())
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let reply = rows
        .iter()
        .filter_map(|p| serde_json::from_str::<Value>(p).ok())
        .find_map(|v| {
            v["text"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
        });
    let body = match (status, reply) {
        ("done", Some(r)) => r.chars().take(6000).collect::<String>(),
        ("done", None) => "（这一轮没有文字回复）".to_owned(),
        ("interrupted", _) => "这次运行被中断了。".to_owned(),
        (_, r) => format!("这次运行失败了。{}", r.unwrap_or_default()),
    };
    add_comment(
        &st,
        &task,
        if status == "done" { "agent" } else { "system" },
        &body,
        false,
        Some(&sid.to_string()),
    )
    .await;
    if cur == "in_progress" {
        let _ = sqlx::query("UPDATE tasks SET status = 'in_review', updated_at = ?2 WHERE id = ?1")
            .bind(&task)
            .bind(now())
            .execute(st.db.pool())
            .await;
    }
    changed(&st);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_validation() {
        let ok = TaskBody {
            title: Some("修登录".into()),
            ..Default::default()
        };
        assert!(check(&ok).is_ok());
        assert!(
            check(&TaskBody {
                title: Some("  ".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            check(&TaskBody {
                status: Some("doing".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            check(&TaskBody {
                priority: Some("p0".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            check(&TaskBody {
                runtime: Some(Some("nope".into())),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            check(&TaskBody {
                labels: Some(vec![String::new()]),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn null_clears_and_absent_keeps() {
        let b: TaskBody = serde_json::from_str(r#"{"parent_id": null}"#).unwrap();
        assert_eq!(b.parent_id, Some(None), "null = 清空");
        let b: TaskBody = serde_json::from_str("{}").unwrap();
        assert_eq!(b.parent_id, None, "缺省 = 不动");
    }
}
