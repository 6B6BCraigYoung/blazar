use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;
use crate::state::ServerEvent;

pub const KINDS: &[&str] = &[
    "run_done",
    "run_failed",
    "approval",
    "question",
    "autopilot_paused",
    "rate_limit",
    "alert",
    "bot_reply",
    "job",
];

#[derive(Debug, Default, Clone)]
pub struct Item {
    pub kind: &'static str,
    pub title: String,
    pub body: String,
    pub workspace_id: Option<String>,
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub ref_id: Option<String>,
}

fn changed(st: &Shared) {
    st.emit(ServerEvent::InboxChanged);
}

pub async fn push(st: &Shared, it: Item) {
    push_id(st, it).await;
}

pub async fn push_id(st: &Shared, it: Item) -> Option<String> {
    let muted = crate::office::prefs(st).await["inbox"]["muted"]
        .as_array()
        .is_some_and(|m| m.iter().any(|k| k == it.kind));
    let id = uuid::Uuid::now_v7().to_string();
    let r = sqlx::query(
        "INSERT INTO inbox (id, kind, title, body, workspace_id, thread_id, session_id, ref_id, created_at, read_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CASE WHEN ?10 THEN ?9 ELSE NULL END)",
    )
    .bind(&id)
    .bind(it.kind)
    .bind(it.title.chars().take(200).collect::<String>())
    .bind(it.body.chars().take(600).collect::<String>())
    .bind(&it.workspace_id)
    .bind(&it.thread_id)
    .bind(&it.session_id)
    .bind(&it.ref_id)
    .bind(Utc::now().to_rfc3339())
    .bind(muted)
    .execute(st.db.pool())
    .await;
    if let Err(e) = r {
        tracing::warn!(target: "blazar::inbox", "收件箱写入失败: {e}");
        return None;
    }
    changed(st);
    if !muted {
        tokio::spawn(crate::office::forward(
            st.clone(),
            id.clone(),
            it.kind,
            it.title.clone(),
            it.body.clone(),
        ));
    }
    Some(id)
}

pub async fn mark_resolved(st: &Shared, id: &str) {
    let r = sqlx::query("UPDATE inbox SET resolved_at = ?2 WHERE id = ?1 AND resolved_at IS NULL")
        .bind(id)
        .bind(Utc::now().to_rfc3339())
        .execute(st.db.pool())
        .await;
    if r.is_ok_and(|r| r.rows_affected() > 0) {
        changed(st);
    }
}

pub async fn resolve_ref(st: &Shared, ref_id: &str) {
    let now = Utc::now().to_rfc3339();
    let r = sqlx::query(
        "UPDATE inbox SET read_at = COALESCE(read_at, ?2), archived_at = ?2
         WHERE ref_id = ?1 AND archived_at IS NULL AND kind IN ('approval', 'question')",
    )
    .bind(ref_id)
    .bind(now)
    .execute(st.db.pool())
    .await;
    if r.is_ok_and(|r| r.rows_affected() > 0) {
        changed(st);
    }
}

pub async fn resolve_session(st: &Shared, sid: blazar_core_types::SessionId) {
    let now = Utc::now().to_rfc3339();
    let r = sqlx::query(
        "UPDATE inbox SET read_at = COALESCE(read_at, ?2), archived_at = ?2
         WHERE session_id = ?1 AND archived_at IS NULL AND kind IN ('approval', 'question')",
    )
    .bind(sid.to_string())
    .bind(now)
    .execute(st.db.pool())
    .await;
    if r.is_ok_and(|r| r.rows_affected() > 0) {
        changed(st);
    }
}

pub async fn on_run_finished(st: Shared, sid: blazar_core_types::SessionId, status: &'static str) {
    if status == "interrupted" {
        return;
    }
    if status == "failed"
        && crate::failures::cause_of(&st, &sid.to_string()).await
            == Some(blazar_core_types::failure::FailureCause::Resume)
    {
        return;
    }
    let row = sqlx::query(
        "SELECT s.workspace_id, COALESCE(s.thread_id, s.id) AS thread, w.name AS ws, n.name AS node,
                w.parent_id,
                (SELECT title FROM sessions r WHERE r.id = COALESCE(s.thread_id, s.id)) AS title
         FROM sessions s JOIN workspaces w ON w.id = s.workspace_id
         JOIN nodes n ON n.id = w.node_id WHERE s.id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let Some(row) = row else { return };
    if row
        .try_get::<Option<String>, _>("parent_id")
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    let ws_name = blazar_core_types::api::workspace_label(
        &row.try_get::<String, _>("ws").unwrap_or_default(),
        &row.try_get::<String, _>("node").unwrap_or_default(),
    );
    let thread: String = row.try_get("thread").unwrap_or_default();
    let title: Option<String> = row.try_get("title").ok().flatten();

    let first: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE COALESCE(s.thread_id, s.id) = ?1 AND e.payload LIKE '{\"type\":\"user_message\"%'
         ORDER BY s.created_at, e.seq LIMIT 1",
    )
    .bind(&thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p: String| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| {
        v["text"]
            .as_str()
            .map(|t| t.lines().next().unwrap_or_default().to_owned())
    });
    let what = title
        .filter(|t| !t.is_empty())
        .or(first)
        .unwrap_or_default();

    let reply: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_id = ?1
           AND (payload LIKE '{\"type\":\"assistant_message\"%' OR payload LIKE '{\"type\":\"finished\"%')
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p: String| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| {
        v["text"]
            .as_str()
            .or_else(|| v["message"].as_str())
            .map(str::to_owned)
    });
    let ok = status == "done";
    push(
        &st,
        Item {
            kind: if ok { "run_done" } else { "run_failed" },
            title: format!(
                "{ws_name} · {}{}",
                if ok { "跑完了" } else { "没跑成" },
                if what.is_empty() {
                    String::new()
                } else {
                    format!("：{what}")
                }
            ),
            body: reply.unwrap_or_default(),
            workspace_id: row.try_get("workspace_id").ok(),
            thread_id: Some(thread),
            session_id: Some(sid.to_string()),
            ref_id: None,
        },
    )
    .await;
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub kind: String,
}

pub async fn list(State(st): State<Shared>, Query(q): Query<ListQuery>) -> Response {
    let cond = match q.filter.as_str() {
        "unread" => "i.archived_at IS NULL AND i.read_at IS NULL",
        "archived" => "i.archived_at IS NOT NULL",
        _ => "i.archived_at IS NULL",
    };
    let kind = Some(q.kind.as_str()).filter(|k| KINDS.contains(k));
    let resets = crate::failures::RESETS_AT;
    let rows = sqlx::query(&format!(
        "SELECT i.*, w.name AS workspace_name, n.name AS node, s.runtime_kind, s.account_id,
                s.failure_cause, s.handled_at, {resets} AS resets_at
         FROM inbox i LEFT JOIN workspaces w ON w.id = i.workspace_id
         LEFT JOIN nodes n ON n.id = w.node_id
         LEFT JOIN sessions s ON s.id = i.session_id
         WHERE {cond} AND (?1 IS NULL OR i.kind = ?1) ORDER BY i.created_at DESC LIMIT 200"
    ))
    .bind(kind)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let unread: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inbox WHERE archived_at IS NULL AND read_at IS NULL",
    )
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0);
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox")
        .fetch_one(st.db.pool())
        .await
        .unwrap_or(0);
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let s = |k: &str| r.try_get::<Option<String>, _>(k).ok().flatten();
            json!({
                "id": s("id"), "kind": s("kind"), "title": s("title"), "body": s("body"),
                "workspace_id": s("workspace_id"), "workspace_name": s("workspace_name"),
                "thread_id": s("thread_id"), "session_id": s("session_id"), "ref_id": s("ref_id"),
                "read": s("read_at").is_some(), "archived": s("archived_at").is_some(),
                "resolved_at": s("resolved_at"), "created_at": s("created_at"),
                "node": s("node"), "runtime": s("runtime_kind"), "account": s("account_id"),
                "cause": s("failure_cause"), "handled": s("handled_at").is_some(),
                "resets_at": s("resets_at"),
            })
        })
        .collect();
    Json(json!({ "unread": unread, "total": total, "items": items })).into_response()
}

pub async fn unread(st: &Shared) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM inbox WHERE archived_at IS NULL AND read_at IS NULL")
        .fetch_one(st.db.pool())
        .await
        .unwrap_or(0)
}

pub async fn count(State(st): State<Shared>) -> Response {
    Json(json!({ "unread": unread(&st).await })).into_response()
}

#[derive(Deserialize, Default)]
pub struct MarkBody {
    pub action: String,

    #[serde(default)]
    pub ids: Vec<String>,
}

pub async fn mark(State(st): State<Shared>, Json(b): Json<MarkBody>) -> Response {
    let now = Utc::now().to_rfc3339();
    let set = match b.action.as_str() {
        "read" => "read_at = COALESCE(read_at, ?1)",
        "unread" => "read_at = NULL",
        "archive" => "archived_at = ?1, read_at = COALESCE(read_at, ?1)",
        "unarchive" => "archived_at = NULL",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "不认识的操作" })),
            )
                .into_response();
        }
    };
    if b.ids.is_empty() {
        let scope = if b.action == "archive" {
            "archived_at IS NULL AND read_at IS NOT NULL"
        } else {
            "archived_at IS NULL"
        };
        let _ = sqlx::query(&format!(
            "UPDATE inbox SET {set} WHERE {scope} AND ?1 IS NOT NULL"
        ))
        .bind(&now)
        .execute(st.db.pool())
        .await;
    } else {
        for id in b.ids.iter().take(500) {
            let _ = sqlx::query(&format!(
                "UPDATE inbox SET {set} WHERE id = ?2 AND ?1 IS NOT NULL"
            ))
            .bind(&now)
            .bind(id)
            .execute(st.db.pool())
            .await;
        }
    }
    changed(&st);
    count(State(st)).await
}

pub async fn seen_thread(State(st): State<Shared>, Path(thread): Path<String>) -> Response {
    let r = sqlx::query(
        "UPDATE inbox SET read_at = ?2 WHERE thread_id = ?1 AND read_at IS NULL
           AND kind IN ('run_done', 'run_failed')",
    )
    .bind(&thread)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    if r.is_ok_and(|r| r.rows_affected() > 0) {
        changed(&st);
    }
    count(State(st)).await
}
