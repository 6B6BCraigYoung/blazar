use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_core_types::{SessionId, WorkspaceId};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::{PromptRequest, Shared};
use crate::state::ServerEvent;

mod queue_payload;

const MAX_QUEUED_BYTES: usize = 12 * 1024 * 1024;

const PREFACE_CHARS: usize = 8000;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn changed(st: &Shared, ws: WorkspaceId) {
    st.emit(ServerEvent::QueueChanged { workspace_id: ws });
}

fn view(r: &sqlx::sqlite::SqliteRow) -> Value {
    let req: Value = r
        .try_get::<String, _>("request")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    json!({
        "id": r.try_get::<String, _>("id").unwrap_or_default(),
        "thread_id": r.try_get::<Option<String>, _>("thread_id").ok().flatten(),
        "text": req["text"].as_str().unwrap_or_default(),
        "images": req["images"].as_array().map_or(0, Vec::len),
        "request": queue_payload::editable_request(&req),
        "held": r.try_get::<Option<String>, _>("held").ok().flatten(),
        "created_at": r.try_get::<String, _>("created_at").unwrap_or_default(),
    })
}

pub async fn list(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let rows = sqlx::query(
        "SELECT id, thread_id, request, held, created_at FROM queued_messages
         WHERE workspace_id = ?1 ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    Json(rows.iter().map(view).collect::<Vec<_>>()).into_response()
}

#[derive(Deserialize)]
pub struct QueueBody {
    #[serde(default)]
    pub thread_id: Option<String>,

    pub request: Value,

    #[serde(default)]
    pub append: bool,

    #[serde(default)]
    pub expected: Option<Value>,
}

pub async fn put(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<QueueBody>,
) -> Response {
    let Ok(ws) = id.parse().map(WorkspaceId) else {
        return fail(StatusCode::BAD_REQUEST, "Invalid workspace id");
    };
    let mut request = b.request;

    let parsed: PromptRequest = match serde_json::from_value(request.clone()) {
        Ok(p) => p,
        Err(e) => {
            return fail(
                StatusCode::BAD_REQUEST,
                format!("Invalid request body: {e}"),
            );
        }
    };
    if parsed.text.trim().is_empty() && parsed.images.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "Empty messages can't be queued");
    }
    let thread = b.thread_id.filter(|t| !t.is_empty());
    let mut tx = match st.db.pool().begin_with("BEGIN IMMEDIATE").await {
        Ok(tx) => tx,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let old = if b.append || b.expected.is_some() {
        match sqlx::query_scalar::<_, String>(
            "SELECT request FROM queued_messages WHERE workspace_id = ?1 AND COALESCE(thread_id, '') = ?2",
        )
        .bind(&id)
        .bind(thread.as_deref().unwrap_or(""))
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(old) => old,
            Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    } else {
        None
    };
    if let Some(expected) = b.expected {
        let stored = old
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        if stored.as_ref() != Some(&expected) {
            return fail(
                StatusCode::CONFLICT,
                "The queued message changed or was already sent; refresh and edit again",
            );
        }
    }
    if let Some(old) = old.filter(|_| b.append) {
        let old = match serde_json::from_str::<Value>(&old) {
            Ok(old) => old,
            Err(e) => {
                return fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Couldn't read the queued message, so nothing was appended: {e}"),
                );
            }
        };
        let joined = format!(
            "{}\n\n{}",
            old["text"].as_str().unwrap_or_default().trim_end(),
            parsed.text.trim_start()
        );
        request["text"] = json!(joined.trim());
        let mut images = old["images"].as_array().cloned().unwrap_or_default();
        images.extend(request["images"].as_array().cloned().unwrap_or_default());
        request["images"] = json!(images);
    }

    request["wait_secs"] = json!(0);
    let raw = request.to_string();
    if raw.len() > MAX_QUEUED_BYTES {
        return fail(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Queued message is too large (too many images?)",
        );
    }
    let qid = uuid::Uuid::now_v7().to_string();
    let r = sqlx::query(
        "INSERT INTO queued_messages (id, workspace_id, thread_id, request, held, created_at)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)
         ON CONFLICT (workspace_id, COALESCE(thread_id, ''))
         DO UPDATE SET request = excluded.request, held = NULL",
    )
    .bind(&qid)
    .bind(&id)
    .bind(thread.as_deref())
    .bind(&raw)
    .bind(Utc::now().to_rfc3339())
    .execute(&mut *tx)
    .await;
    if let Err(e) = r {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    if let Err(e) = tx.commit().await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    changed(&st, ws);

    if !busy(&st, ws).await {
        tokio::spawn(dispatch(st.clone(), ws, None));
    }
    list(State(st), Path(id)).await
}

pub async fn remove(
    State(st): State<Shared>,
    Path((id, qid)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Response {
    let expected = body
        .and_then(|Json(body)| body.get("expected").cloned())
        .map(|expected| expected.to_string());
    let result = sqlx::query("DELETE FROM queued_messages WHERE id = ?1 AND workspace_id = ?2 AND (?3 IS NULL OR request = ?3)")
        .bind(&qid)
        .bind(&id)
        .bind(&expected)
        .execute(st.db.pool())
        .await;
    match result {
        Err(error) => return fail(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        Ok(result) if result.rows_affected() == 0 => {
            return if expected.is_some() {
                fail(
                    StatusCode::CONFLICT,
                    "The queued message changed; the new content was not removed",
                )
            } else {
                fail(
                    StatusCode::NOT_FOUND,
                    "This queued message no longer exists",
                )
            };
        }
        Ok(_) => {}
    }
    if let Ok(ws) = id.parse().map(WorkspaceId) {
        changed(&st, ws);
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn send_now(
    State(st): State<Shared>,
    Path((id, qid)): Path<(String, String)>,
) -> Response {
    let Ok(ws) = id.parse().map(WorkspaceId) else {
        return fail(StatusCode::BAD_REQUEST, "Invalid workspace id");
    };
    if busy(&st, ws).await {
        return fail(
            StatusCode::CONFLICT,
            "Agent is still running: wait for this turn to finish, or send the message mid-turn",
        );
    }
    match dispatch_one(&st, ws, &qid).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}

async fn busy(st: &Shared, ws: WorkspaceId) -> bool {
    st.running
        .read()
        .await
        .values()
        .any(|r| r.workspace_id == ws)
}

fn dispatch_one<'a>(
    st: &'a Shared,
    ws: WorkspaceId,
    qid: &'a str,
) -> futures::future::BoxFuture<'a, Result<Value, String>> {
    Box::pin(dispatch_inner(st, ws, qid))
}

async fn dispatch_inner(st: &Shared, ws: WorkspaceId, qid: &str) -> Result<Value, String> {
    dispatch_using(st, ws, qid, |req| {
        crate::api::prompt(State(st.clone()), Path(ws.to_string()), Json(req))
    })
    .await
}

async fn dispatch_using<F, Fut>(
    st: &Shared,
    ws: WorkspaceId,
    qid: &str,
    send: F,
) -> Result<Value, String>
where
    F: FnOnce(PromptRequest) -> Fut,
    Fut: Future<Output = crate::api::ApiResult<Json<Value>>>,
{
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT request, thread_id FROM queued_messages WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(qid)
    .bind(ws.to_string())
    .fetch_optional(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let (raw, queued_thread) = row.ok_or("This queued message no longer exists")?;
    let req: PromptRequest = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let sent = send(req).await;
    let v = match sent {
        Ok(Json(v)) => v,
        Err(e) => {
            hold(st, ws, qid, &raw, &format!("Not sent: {}", e.message())).await;
            return Err(e.message());
        }
    };
    if v["admitted"] == json!(false) {
        return Err(v["reason"]
            .as_str()
            .unwrap_or("Workspace is busy")
            .to_owned());
    }
    let _ = sqlx::query(
        "DELETE FROM queued_messages WHERE id = ?1 AND workspace_id = ?2 AND request = ?3",
    )
    .bind(qid)
    .bind(ws.to_string())
    .bind(&raw)
    .execute(st.db.pool())
    .await;
    changed(st, ws);
    if let (Some(thread), Some(sid)) = (v["thread_id"].as_str(), v["session_id"].as_str()) {
        st.emit(ServerEvent::QueueSent {
            workspace_id: ws,
            queued_thread,
            thread_id: thread.to_owned(),
            session_id: sid.to_owned(),
        });
    }
    Ok(v)
}

async fn hold(st: &Shared, ws: WorkspaceId, qid: &str, raw: &str, why: &str) {
    let _ = sqlx::query(
        "UPDATE queued_messages SET held = ?2 WHERE id = ?1 AND workspace_id = ?3 AND request = ?4",
    )
    .bind(qid)
    .bind(why)
    .bind(ws.to_string())
    .bind(raw)
    .execute(st.db.pool())
    .await;
    changed(st, ws);
}

async fn dispatch(st: Shared, ws: WorkspaceId, thread: Option<String>) {
    let qid: Option<String> = sqlx::query_scalar(
        "SELECT id FROM queued_messages WHERE workspace_id = ?1 AND held IS NULL
         ORDER BY (COALESCE(thread_id, '') = ?2) DESC, created_at LIMIT 1",
    )
    .bind(ws.to_string())
    .bind(thread.unwrap_or_default())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    if let Some(qid) = qid
        && let Err(e) = dispatch_one(&st, ws, &qid).await
    {
        tracing::info!(target: "blazar::chat", "排队消息 {qid} 这次没发出去：{e}");
    }
}

pub async fn on_run_finished(
    st: Shared,
    ws: WorkspaceId,
    sid: SessionId,
    status: &'static str,
    interrupted_at: Option<String>,
) {
    let thread: Option<String> =
        sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten();
    if status == "done" {
        dispatch(st, ws, thread).await;
        return;
    }
    let why = if status == "interrupted" {
        "The previous turn was interrupted, so this was not sent automatically"
    } else {
        "The previous turn did not finish normally, so this was not sent automatically"
    };
    let r = sqlx::query(
        "UPDATE queued_messages SET held = ?2 WHERE workspace_id = ?1 AND held IS NULL
           AND (?3 IS NULL OR created_at < ?3)",
    )
    .bind(ws.to_string())
    .bind(why)
    .bind(
        interrupted_at
            .as_deref()
            .filter(|_| status == "interrupted"),
    )
    .execute(st.db.pool())
    .await;
    if r.is_ok_and(|r| r.rows_affected() > 0) {
        changed(&st, ws);
    }
    if status == "interrupted" && interrupted_at.is_some() {
        dispatch(st, ws, thread).await;
    }
}

#[derive(Deserialize, Default)]
pub struct WaitQuery {
    #[serde(default)]
    pub until: String,

    #[serde(default)]
    pub timeout: Option<u64>,

    #[serde(default)]
    pub stall: Option<u64>,
}

pub async fn wait(
    State(st): State<Shared>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<WaitQuery>,
) -> Response {
    let timeout = q.timeout.unwrap_or(600).min(3600);
    let stall = q.stall.unwrap_or(300).max(10);
    let want_blocked = q.until == "blocked";
    let started = std::time::Instant::now();
    let mut bus = st.bus.subscribe();
    loop {
        let row = sqlx::query(
            "SELECT w.activity,
                (SELECT e.ts FROM events e JOIN sessions s ON s.id = e.session_id
                 WHERE s.workspace_id = w.id
                 ORDER BY e.ts DESC, s.created_at DESC, e.seq DESC LIMIT 1) AS last_event,
                (SELECT json_extract(e.payload, '$.type') FROM events e
                 JOIN sessions s ON s.id = e.session_id
                 WHERE s.workspace_id = w.id
                   AND json_extract(e.payload, '$.type') NOT IN ('progress', 'rate_limit', 'token_usage')
                 ORDER BY e.ts DESC, s.created_at DESC, e.seq DESC LIMIT 1) AS last_kind
             FROM workspaces w WHERE w.id = ?1",
        )
        .bind(&id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
        let Some(row) = row else {
            return fail(StatusCode::NOT_FOUND, "No such workspace");
        };
        let activity: String = row.try_get("activity").unwrap_or_default();
        let running = activity == "running";
        let blocked = activity == "awaiting_approval";
        if (!running && !blocked) || (want_blocked && blocked) {
            let reply: Option<String> = sqlx::query_scalar(
                "SELECT e.payload FROM events e JOIN sessions s ON s.id = e.session_id
                 WHERE s.workspace_id = ?1 AND e.payload LIKE '{\"type\":\"assistant_message\"%'
                 ORDER BY s.created_at DESC, e.seq DESC LIMIT 1",
            )
            .bind(&id)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .and_then(|p: String| serde_json::from_str::<Value>(&p).ok())
            .and_then(|v| v["text"].as_str().map(str::to_owned));
            return Json(json!({ "reached": true, "activity": activity, "last_reply": reply }))
                .into_response();
        }
        let in_tool = row
            .try_get::<Option<String>, _>("last_kind")
            .ok()
            .flatten()
            .as_deref()
            == Some("tool_use");
        if running
            && !in_tool
            && let Some(last) = row
                .try_get::<Option<String>, _>("last_event")
                .ok()
                .flatten()
            && let Ok(t) = chrono::DateTime::parse_from_rfc3339(&last)
            && (Utc::now() - t.with_timezone(&Utc)).num_seconds()
                > i64::try_from(stall).unwrap_or(300)
        {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({ "reached": false, "stalled": true, "activity": activity, "error_code": "agent_prompt_stalled",
                             "error": format!("Agent is still running but has shown no activity for {stall}s") })),
            )
                .into_response();
        }
        if started.elapsed().as_secs() >= timeout {
            return (
                StatusCode::OK,
                Json(json!({ "reached": false, "timed_out": true, "activity": activity, "error_code": "wait_timeout",
                             "error": format!("Still running after waiting {timeout}s") })),
            )
                .into_response();
        }

        let left = timeout
            .saturating_sub(started.elapsed().as_secs())
            .clamp(1, 5);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(left), bus.recv()).await;
    }
}

#[derive(Deserialize)]
pub struct RetryBody {
    pub session_id: String,

    #[serde(default)]
    pub text: Option<String>,

    #[serde(default)]
    pub options: Value,
}

pub(crate) async fn preface(st: &Shared, thread: &str, before: &str, note: &str) -> String {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT e.payload FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE COALESCE(s.thread_id, s.id) = ?1 AND s.rewound_at IS NULL AND s.created_at < ?2
           AND e.parent_tool_id IS NULL
           AND (e.payload LIKE '{\"type\":\"user_message\"%' OR e.payload LIKE '{\"type\":\"assistant_message\"%')
         ORDER BY s.created_at, e.seq",
    )
    .bind(thread)
    .bind(before)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut turns = Vec::new();
    for p in rows {
        let Ok(v) = serde_json::from_str::<Value>(&p) else {
            continue;
        };
        let who = if v["type"] == "user_message" {
            "User"
        } else {
            "Assistant"
        };
        if let Some(t) = v["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
            turns.push(format!(
                "{who}: {}",
                t.chars().take(1500).collect::<String>()
            ));
        }
    }
    if turns.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for t in turns.iter().rev() {
        if out.chars().count() + t.chars().count() > PREFACE_CHARS {
            break;
        }
        out = format!("{t}\n\n{out}");
    }
    format!(
        "(Below is the earlier part of this conversation, for context; {note})\n\n{}\n(End of transcript. The user's current message follows.)\n\n",
        out.trim_end()
    )
}

pub async fn retry(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<RetryBody>,
) -> Response {
    let Ok(ws) = id.parse().map(WorkspaceId) else {
        return fail(StatusCode::BAD_REQUEST, "Invalid workspace id");
    };
    if busy(&st, ws).await {
        return fail(
            StatusCode::CONFLICT,
            "Agent is still running; interrupt it before retrying",
        );
    }
    let row = sqlx::query(
        "SELECT COALESCE(thread_id, id) AS thread, created_at, runtime_kind, rewound_at
         FROM sessions WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(&b.session_id)
    .bind(&id)
    .fetch_optional(st.db.pool())
    .await;
    let Ok(Some(row)) = row else {
        return fail(StatusCode::NOT_FOUND, "No such session");
    };
    if row
        .try_get::<Option<String>, _>("rewound_at")
        .ok()
        .flatten()
        .is_some()
    {
        return fail(
            StatusCode::CONFLICT,
            "This message has already been rewound",
        );
    }
    let thread: String = row.try_get("thread").unwrap_or_default();
    let created: String = row.try_get("created_at").unwrap_or_default();
    let runtime: String = row.try_get("runtime_kind").unwrap_or_default();

    let original: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_id = ?1 AND payload LIKE '{\"type\":\"user_message\"%'
         ORDER BY seq LIMIT 1",
    )
    .bind(&b.session_id)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p: String| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| v["text"].as_str().map(str::to_owned));
    let text = b
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or(original);
    let Some(text) = text else {
        return fail(
            StatusCode::CONFLICT,
            "Couldn't find the original text of that message",
        );
    };

    let cp: Option<String> = sqlx::query_scalar(
        "SELECT id FROM checkpoints WHERE workspace_id = ?1 AND session_id = ?2 ORDER BY seq, created_at LIMIT 1",
    )
    .bind(&id)
    .bind(&b.session_id)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let mut undo = None;
    if let Some(cp) = &cp {
        match crate::checkpoint::restore_to(&st, cp).await {
            Ok(u) => undo = u,
            Err((status, msg)) => return fail(status, msg),
        }
    }

    let stamp = Utc::now().to_rfc3339();
    let _ = sqlx::query(
        "UPDATE sessions SET rewound_at = ?3
         WHERE COALESCE(thread_id, id) = ?1 AND created_at >= ?2 AND rewound_at IS NULL",
    )
    .bind(&thread)
    .bind(&created)
    .bind(&stamp)
    .execute(st.db.pool())
    .await;

    let mut req = if b.options.is_object() {
        b.options
    } else {
        json!({})
    };
    let target_runtime = match req["profile"].as_str().filter(|p| !p.is_empty()) {
        Some(pid) => crate::agents::get(&st, pid)
            .await
            .map(|p| p.runtime)
            .unwrap_or_default(),
        None => req["agent"]
            .as_str()
            .filter(|a| !a.is_empty())
            .unwrap_or(&runtime)
            .to_owned(),
    };
    if req["agent"].is_null() && req["profile"].is_null() {
        req["agent"] = json!(runtime);
    }

    let prior: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT runtime_kind, last_uuid FROM sessions
         WHERE COALESCE(thread_id, id) = ?1 AND rewound_at IS NULL AND provider_session_id IS NOT NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let native = match &prior {
        None => true,
        Some((rt, uuid)) => rt == "claude" && target_runtime == "claude" && uuid.is_some(),
    };
    req["resume_session"] = json!(thread);
    req["retry_thread"] = json!(thread);
    if native {
        req["resume"] = json!(true);
        req["resume_at_last"] = json!(true);
        req["text"] = json!(text);
    } else {
        req["resume"] = json!(false);
        req["text"] = json!(format!(
            "{}{text}",
            preface(
                &st,
                &thread,
                &created,
                "Files have been restored to how they were at that point."
            )
            .await
        ));
    }
    let parsed: PromptRequest = match serde_json::from_value(req) {
        Ok(p) => p,
        Err(e) => {
            take_back(&st, &thread, &stamp, undo.as_deref()).await;
            return fail(
                StatusCode::BAD_REQUEST,
                format!("Invalid send options: {e}"),
            );
        }
    };
    match crate::api::prompt(State(st.clone()), Path(id), Json(parsed)).await {
        Ok(Json(mut v)) if v["admitted"] == json!(false) => {
            take_back(&st, &thread, &stamp, undo.as_deref()).await;
            v["files_restored"] = json!(false);
            Json(v).into_response()
        }
        Ok(Json(mut v)) => {
            v["files_restored"] = json!(cp.is_some());
            v["undo"] = json!(undo);
            Json(v).into_response()
        }
        Err(e) => {
            take_back(&st, &thread, &stamp, undo.as_deref()).await;
            fail(StatusCode::BAD_GATEWAY, e.message())
        }
    }
}

async fn take_back(st: &Shared, thread: &str, stamp: &str, undo: Option<&str>) {
    let _ = sqlx::query(
        "UPDATE sessions SET rewound_at = NULL WHERE COALESCE(thread_id, id) = ?1 AND rewound_at = ?2",
    )
    .bind(thread)
    .bind(stamp)
    .execute(st.db.pool())
    .await;
    if let Some(u) = undo
        && let Err((_, e)) = crate::checkpoint::restore_to(st, u).await
    {
        tracing::warn!(target: "blazar::chat", "重试没开起来，文件也没能改回去：{e}");
    }
    st.emit(ServerEvent::WorkspacesChanged);
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    async fn queued() -> (Shared, WorkspaceId) {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-queue-tests")),
            crate::services::Services::Isolated,
        );
        let ws = WorkspaceId::new();
        sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
            .execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, 'node', 'repo', '/home/me/repo', '0')")
            .bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO queued_messages (id, workspace_id, request, created_at) VALUES ('queued', ?1, ?2, '0')")
            .bind(ws.to_string()).bind(json!({"text": "original"}).to_string())
            .execute(st.db.pool()).await.unwrap();
        (st, ws)
    }

    async fn edit_queued(st: &Shared) {
        sqlx::query("UPDATE queued_messages SET request = ?1, held = NULL WHERE id = 'queued'")
            .bind(json!({"text": "edited while sending"}).to_string())
            .execute(st.db.pool())
            .await
            .unwrap();
    }

    async fn queue_contents(st: &Shared) -> Vec<(String, Option<String>)> {
        sqlx::query_as("SELECT request, held FROM queued_messages")
            .fetch_all(st.db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn remove_queue_reports_database_failure_without_emitting_change() {
        let (st, ws) = queued().await;
        sqlx::query("DROP TABLE queued_messages")
            .execute(st.db.pool())
            .await
            .unwrap();
        let mut events = st.bus.subscribe();
        let response = remove(
            State(st.clone()),
            Path((ws.to_string(), "queued".into())),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn remove_queue_reports_missing_message_without_emitting_change() {
        let (st, ws) = queued().await;
        let mut events = st.bus.subscribe();
        let response = remove(
            State(st.clone()),
            Path((ws.to_string(), "missing".into())),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(queue_contents(&st).await.len(), 1);
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn remove_queue_emits_change_only_after_deleting_the_message() {
        let (st, ws) = queued().await;
        let mut events = st.bus.subscribe();
        let response = remove(
            State(st.clone()),
            Path((ws.to_string(), "queued".into())),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(queue_contents(&st).await.is_empty());
        assert!(
            matches!(events.try_recv().map(|f| f.event), Ok(ServerEvent::QueueChanged { workspace_id }) if workspace_id == ws)
        );
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn queued_edits_and_steers_cannot_replace_or_delete_newer_payloads() {
        let (st, ws) = queued().await;
        let old = json!({"text": "original"});
        edit_queued(&st).await;
        let before = queue_contents(&st).await;
        let response = put(
            State(st.clone()),
            Path(ws.to_string()),
            Json(QueueBody {
                thread_id: None,
                request: json!({"text": "edited draft"}),
                append: false,
                expected: Some(old.clone()),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = remove(
            State(st.clone()),
            Path((ws.to_string(), "queued".into())),
            Some(Json(json!({"expected":old}))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(queue_contents(&st).await, before);
    }

    #[tokio::test]
    async fn concurrent_appends_preserve_both_messages() {
        let (st, ws) = queued().await;
        let sid = SessionId::new();
        st.running.write().await.insert(
            sid,
            crate::state::RunningSession {
                workspace_id: ws,
                task: tokio::spawn(std::future::pending()),
                killer: None,
                live: None,
            },
        );
        let append = |text: &str| {
            put(
                State(st.clone()),
                Path(ws.to_string()),
                Json(QueueBody {
                    thread_id: None,
                    request: json!({"text": text, "images": [{"media_type": "image/png", "data": "YQ=="}]}),
                    append: true,
                    expected: None,
                }),
            )
        };
        let (first, second) = tokio::join!(append("first addition"), append("second addition"));
        st.running.write().await.remove(&sid).unwrap().task.abort();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        let rows = queue_contents(&st).await;
        assert_eq!(rows.len(), 1);
        let request: Value = serde_json::from_str(&rows[0].0).unwrap();
        let parts: Vec<_> = request["text"].as_str().unwrap().split("\n\n").collect();
        assert_eq!(parts.len(), 3, "{request}");
        assert!(parts.contains(&"original"));
        assert!(parts.contains(&"first addition"));
        assert!(parts.contains(&"second addition"));
        assert_eq!(request["images"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn oversized_append_preserves_the_previous_message() {
        let (st, ws) = queued().await;
        let before = queue_contents(&st).await;
        let response = put(
            State(st.clone()),
            Path(ws.to_string()),
            Json(QueueBody {
                thread_id: None,
                request: json!({"text": "a".repeat(MAX_QUEUED_BYTES)}),
                append: true,
                expected: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(queue_contents(&st).await, before);
    }

    #[tokio::test]
    async fn successful_dispatch_preserves_a_concurrent_edit() {
        let (st, ws) = queued().await;
        let result = dispatch_using(&st, ws, "queued", |req| {
            assert_eq!(req.text, "original");
            async {
                edit_queued(&st).await;
                Ok(Json(json!({"session_id": "sent", "thread_id": "thread"})))
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(
            queue_contents(&st).await,
            [(json!({"text": "edited while sending"}).to_string(), None)]
        );
        dispatch_using(&st, ws, "queued", |req| async move {
            assert_eq!(req.text, "edited while sending");
            Ok(Json(json!({"session_id": "next"})))
        })
        .await
        .unwrap();
        assert!(queue_contents(&st).await.is_empty());
    }

    #[tokio::test]
    async fn failed_dispatch_does_not_hold_a_concurrent_edit() {
        let (st, ws) = queued().await;
        let result = dispatch_using(&st, ws, "queued", |req| {
            assert_eq!(req.text, "original");
            async {
                edit_queued(&st).await;
                Err(anyhow::anyhow!("launch failed").into())
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(
            queue_contents(&st).await,
            [(json!({"text": "edited while sending"}).to_string(), None)]
        );
    }

    #[tokio::test]
    async fn unchanged_dispatch_is_removed_on_success_and_held_on_failure() {
        let (st, ws) = queued().await;
        assert!(
            dispatch_using(&st, ws, "queued", |_| async {
                Err(anyhow::anyhow!("launch failed").into())
            })
            .await
            .is_err()
        );
        assert!(
            queue_contents(&st).await[0]
                .1
                .as_deref()
                .unwrap()
                .contains("launch failed")
        );
        dispatch_using(&st, ws, "queued", |_| async {
            Ok(Json(json!({"session_id": "sent"})))
        })
        .await
        .unwrap();
        assert!(queue_contents(&st).await.is_empty());
    }

    #[tokio::test]
    async fn rejected_admission_keeps_the_queued_message() {
        let (st, ws) = queued().await;
        let before = queue_contents(&st).await;
        assert!(
            dispatch_using(&st, ws, "queued", |_| async {
                Ok(Json(json!({"admitted": false, "reason": "busy"})))
            })
            .await
            .is_err()
        );
        assert_eq!(queue_contents(&st).await, before);
    }
}

#[cfg(test)]
mod wait_tests {
    use super::*;

    async fn quiet_since(last: &str) -> Response {
        quiet_with(&[last]).await
    }

    async fn quiet_with(events: &[&str]) -> Response {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-wait-tests")),
            crate::services::Services::Isolated,
        );
        for setup in [
            "INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')",
            "INSERT INTO workspaces (id, node_id, name, path, created_at, activity)
             VALUES ('w', 'node', 'repo', '/repo', '0', 'running')",
            "INSERT INTO sessions (id, workspace_id, runtime_kind, created_at) VALUES ('s', 'w', 'claude', '0')",
        ] {
            sqlx::query(setup).execute(st.db.pool()).await.unwrap();
        }
        let ten_minutes_ago = (Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        for (seq, payload) in (1i64..).zip(events) {
            sqlx::query(
                "INSERT INTO events (session_id, seq, ts, payload) VALUES ('s', ?1, ?2, ?3)",
            )
            .bind(seq)
            .bind(&ten_minutes_ago)
            .bind(payload)
            .execute(st.db.pool())
            .await
            .unwrap();
        }
        wait(
            State(st),
            Path("w".into()),
            axum::extract::Query(WaitQuery {
                timeout: Some(1),
                stall: Some(60),
                ..WaitQuery::default()
            }),
        )
        .await
    }

    #[tokio::test]
    async fn a_long_tool_call_is_not_a_stall() {
        let busy = quiet_since(
            r#"{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"python train.py"}}"#,
        )
        .await;
        assert_eq!(busy.status(), StatusCode::OK);
        let quiet = quiet_since(r#"{"type":"assistant_message","text":"开始训练"}"#).await;
        assert_eq!(quiet.status(), StatusCode::REQUEST_TIMEOUT);
    }

    const TOOL: &str =
        r#"{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"sleep 300"}}"#;
    const PROGRESS: &str = r#"{"type":"progress","text":"running"}"#;
    const RATE: &str = r#"{"type":"rate_limit","allowed":true,"windows":[]}"#;
    const SAID: &str = r#"{"type":"assistant_message","text":"好的"}"#;

    #[tokio::test]
    async fn a_tool_call_sharing_a_timestamp_with_the_event_before_it_is_not_a_stall() {
        let busy = quiet_with(&[PROGRESS, TOOL]).await;
        assert_eq!(busy.status(), StatusCode::OK);
        let busy = quiet_with(&[SAID, PROGRESS, TOOL]).await;
        assert_eq!(busy.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bookkeeping_events_after_a_tool_call_do_not_hide_it() {
        let busy = quiet_with(&[TOOL, PROGRESS]).await;
        assert_eq!(busy.status(), StatusCode::OK);
        let busy = quiet_with(&[TOOL, PROGRESS, RATE, PROGRESS]).await;
        assert_eq!(busy.status(), StatusCode::OK);
        let quiet = quiet_with(&[TOOL, SAID, PROGRESS]).await;
        assert_eq!(
            quiet.status(),
            StatusCode::REQUEST_TIMEOUT,
            "工具结束后又安静下来，还是算卡住"
        );
    }

    #[tokio::test]
    async fn a_wait_that_runs_out_is_an_answer_not_an_error() {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-wait-tests")),
            crate::services::Services::Isolated,
        );
        for setup in [
            "INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')",
            "INSERT INTO workspaces (id, node_id, name, path, created_at, activity)
             VALUES ('w', 'node', 'repo', '/repo', '0', 'running')",
        ] {
            sqlx::query(setup).execute(st.db.pool()).await.unwrap();
        }
        let response = wait(
            State(st),
            Path("w".into()),
            axum::extract::Query(WaitQuery {
                timeout: Some(1),
                ..WaitQuery::default()
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["reached"], false);
        assert_eq!(v["timed_out"], true);
        assert_eq!(v["activity"], "running");
    }
}
