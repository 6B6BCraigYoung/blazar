use axum::Json;
use axum::extract::{Path, State};
use blazar_core_types::api::WorkspaceFailure;
use blazar_core_types::failure::FailureCause;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::{ApiResult, Shared};
use crate::state::AppState;
use crate::state::ServerEvent;

pub(crate) async fn last_message(st: &AppState, sid: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT payload FROM events WHERE session_id = ?1 AND (
           payload LIKE '{\"type\":\"finished\"%' OR payload LIKE '{\"type\":\"error\"%')
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(sid)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| v["message"].as_str().map(str::to_owned))
    .unwrap_or_default()
}

fn online(node: &str, status: &str) -> bool {
    node == "local" || status == "online"
}

pub async fn record(st: &AppState, sid: blazar_core_types::SessionId) -> Option<FailureCause> {
    let row = sqlx::query(
        "SELECT n.name AS node, n.status FROM sessions s
         JOIN workspaces w ON w.id = s.workspace_id JOIN nodes n ON n.id = w.node_id
         WHERE s.id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()?;
    let node: String = row.try_get("node").unwrap_or_default();
    let status: String = row.try_get("status").unwrap_or_default();
    let message = last_message(st, &sid.to_string()).await;
    let cause = FailureCause::of(&message, online(&node, &status));
    let _ = sqlx::query("UPDATE sessions SET failure_cause = ?2 WHERE id = ?1")
        .bind(sid.to_string())
        .bind(cause.as_str())
        .execute(st.db.pool())
        .await;
    Some(cause)
}

pub async fn cause_of(st: &AppState, sid: &str) -> Option<FailureCause> {
    sqlx::query_scalar::<_, Option<String>>("SELECT failure_cause FROM sessions WHERE id = ?1")
        .bind(sid)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .flatten()
        .and_then(|c| FailureCause::parse(&c))
}

pub(crate) const RESETS_AT: &str = "(SELECT r.resets_at FROM rate_limit_snapshots r
    WHERE r.account_id = s.account_id AND r.utilization >= 1 AND r.resets_at IS NOT NULL
    ORDER BY r.resets_at LIMIT 1)";

pub async fn of_workspace(st: &AppState, ws: &str, node_online: bool) -> Option<WorkspaceFailure> {
    let row = sqlx::query(&format!(
        "SELECT s.id, COALESCE(s.thread_id, s.id) AS thread, s.runtime_kind, s.account_id,
                s.failure_cause, s.handled_at, s.status, {RESETS_AT} AS resets_at
         FROM sessions s WHERE s.workspace_id = ?1 ORDER BY s.created_at DESC LIMIT 1"
    ))
    .bind(ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()?;
    let status: String = row.try_get("status").unwrap_or_default();
    let handled: Option<String> = row.try_get("handled_at").unwrap_or(None);
    if status != "failed" || handled.is_some() {
        return None;
    }
    let sid: String = row.try_get("id").unwrap_or_default();
    let message = last_message(st, &sid).await;
    let cause = row
        .try_get::<Option<String>, _>("failure_cause")
        .unwrap_or(None)
        .and_then(|c| FailureCause::parse(&c))
        .unwrap_or_else(|| FailureCause::of(&message, node_online));
    if cause == FailureCause::Resume {
        return None;
    }
    Some(WorkspaceFailure {
        cause,
        message: message
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned(),
        session_id: sid,
        thread_id: row.try_get("thread").unwrap_or_default(),
        runtime: row.try_get("runtime_kind").unwrap_or_default(),
        account: row.try_get("account_id").unwrap_or(None),
        resets_at: (cause == FailureCause::Quota)
            .then(|| {
                row.try_get::<Option<String>, _>("resets_at")
                    .unwrap_or(None)
            })
            .flatten(),
    })
}

pub async fn handle(st: &AppState, ws: &str, session: Option<&str>) -> ApiResult<Vec<String>> {
    let now = Utc::now().to_rfc3339();
    let candidates: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE workspace_id = ?1 AND status = 'failed' AND handled_at IS NULL
           AND (?2 IS NULL OR id = ?2)",
    )
    .bind(ws)
    .bind(session)
    .fetch_all(st.db.pool())
    .await?;
    let mut handled = Vec::with_capacity(candidates.len());
    for sid in candidates {
        // 只认自己真正改到的那一行：两处同时处理同一个失败时只有一处算数。
        let claimed =
            sqlx::query("UPDATE sessions SET handled_at = ?2 WHERE id = ?1 AND handled_at IS NULL")
                .bind(&sid)
                .bind(&now)
                .execute(st.db.pool())
                .await?
                .rows_affected();
        if claimed == 0 {
            continue;
        }
        sqlx::query(
            "UPDATE inbox SET resolved_at = COALESCE(resolved_at, ?2), read_at = COALESCE(read_at, ?2)
             WHERE kind = 'run_failed' AND session_id = ?1",
        )
        .bind(&sid)
        .bind(&now)
        .execute(st.db.pool())
        .await?;
        handled.push(sid);
    }
    let calmed = sqlx::query(
        "UPDATE workspaces SET activity = 'idle' WHERE id = ?1 AND activity = 'errored'",
    )
    .bind(ws)
    .execute(st.db.pool())
    .await?
    .rows_affected();
    if calmed > 0 || !handled.is_empty() {
        st.emit(ServerEvent::WorkspacesChanged);
        st.emit(ServerEvent::InboxChanged);
    }
    Ok(handled)
}

/// 机器重新连上了：因它中断的活不悄悄清掉，按断网一样处理——
/// 开着自动续跑就接着做，关着就把卡片改成“机器回来了，可以继续”。
pub async fn node_back(st: &Shared, node: &str) {
    node_back_with(st, node, &crate::auto_resume::send_again).await;
}

pub(crate) async fn node_back_with<S>(st: &Shared, node: &str, send: &S)
where
    S: Fn(
        Shared,
        String,
        crate::api::PromptRequest,
    ) -> futures::future::BoxFuture<'static, Result<Value, String>>,
{
    let rows = sqlx::query(
        "SELECT s.id, s.failure_cause FROM sessions s
         JOIN workspaces w ON w.id = s.workspace_id JOIN nodes n ON n.id = w.node_id
         WHERE n.name = ?1 AND s.status = 'failed' AND s.handled_at IS NULL
           AND s.id = (SELECT x.id FROM sessions x WHERE x.workspace_id = w.id
                       ORDER BY x.created_at DESC LIMIT 1)",
    )
    .bind(node)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    for row in rows {
        let sid: String = row.try_get("id").unwrap_or_default();
        let cause = match row
            .try_get::<Option<String>, _>("failure_cause")
            .unwrap_or(None)
            .and_then(|c| FailureCause::parse(&c))
        {
            Some(c) => c,
            None => FailureCause::of(&last_message(st, &sid).await, true),
        };
        if cause != FailureCause::Node {
            continue;
        }
        let Ok(id) = sid.parse().map(blazar_core_types::SessionId) else {
            continue;
        };
        tracing::info!(target: "blazar::failures", "{node} 重新连上，接着处理 {sid}");
        crate::auto_resume::on_node_back(st, id, node, send).await;
    }
}

#[derive(Deserialize, Default)]
pub struct HandledBody {
    #[serde(default)]
    pub session_id: Option<String>,
}

pub async fn mark_handled(
    State(st): State<Shared>,
    Path(id): Path<String>,
    body: Option<Json<HandledBody>>,
) -> ApiResult<Json<Value>> {
    let session = body
        .and_then(|Json(b)| b.session_id)
        .filter(|s| !s.is_empty());
    let handled = handle(&st, &id, session.as_deref()).await?;
    Ok(Json(json!({ "handled": handled })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blazar_core_types::{SessionId, WorkspaceId};

    async fn fixture(node_status: &str) -> (Shared, WorkspaceId, SessionId) {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-failure-tests")),
            crate::services::Services::Isolated,
        );
        let ws = WorkspaceId::new();
        let sid = SessionId::new();
        sqlx::query("INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n1', 'gpu-x', 'ssh', ?1, '0')")
            .bind(node_status).execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, activity, created_at) VALUES (?1, 'n1', 'repo', '/home/me/repo', 'errored', '0')")
            .bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES (?1, ?2, 'claude', 'failed', '1')")
            .bind(sid.to_string()).bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        (st, ws, sid)
    }

    async fn fail_with(st: &AppState, sid: SessionId, ws: WorkspaceId, message: &str) {
        let entry = blazar_core_types::NormalizedEntry {
            seq: 1,
            ts: Utc::now(),
            parent_tool_use_id: None,
            kind: blazar_core_types::EntryKind::Finished(blazar_core_types::Outcome::Failed {
                message: message.to_owned(),
            }),
        };
        st.db.append_event(sid, ws, &entry).await.unwrap();
    }

    async fn activity(st: &AppState, ws: WorkspaceId) -> String {
        sqlx::query_scalar("SELECT activity FROM workspaces WHERE id = ?1")
            .bind(ws.to_string())
            .fetch_one(st.db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_failure_on_an_offline_machine_is_recorded_as_node() {
        let (st, ws, sid) = fixture("offline").await;
        fail_with(
            &st,
            sid,
            ws,
            "agent 进程已经不在了（机器重启、被整组杀掉或目录被删）",
        )
        .await;
        assert_eq!(record(&st, sid).await, Some(FailureCause::Node));
        assert_eq!(
            cause_of(&st, &sid.to_string()).await,
            Some(FailureCause::Node)
        );
        let view = of_workspace(&st, &ws.to_string(), false).await.unwrap();
        assert_eq!(view.cause, FailureCause::Node);
        assert_eq!(view.session_id, sid.to_string());
        assert_eq!(view.runtime, "claude");
    }

    #[tokio::test]
    async fn handling_calms_the_workspace_and_resolves_the_notification() {
        let (st, ws, sid) = fixture("online").await;
        fail_with(&st, sid, ws, "API Error: 529 overloaded").await;
        record(&st, sid).await;
        sqlx::query("INSERT INTO inbox (id, kind, title, workspace_id, session_id, created_at) VALUES ('i1', 'run_failed', 't', ?1, ?2, '2')")
            .bind(ws.to_string()).bind(sid.to_string()).execute(st.db.pool()).await.unwrap();
        let handled = handle(&st, &ws.to_string(), None)
            .await
            .map_err(|e| e.message())
            .unwrap();
        assert_eq!(handled, vec![sid.to_string()]);
        assert_eq!(activity(&st, ws).await, "idle");
        assert!(of_workspace(&st, &ws.to_string(), true).await.is_none());
        let (resolved, read): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT resolved_at, read_at FROM inbox WHERE id = 'i1'")
                .fetch_one(st.db.pool())
                .await
                .unwrap();
        assert!(resolved.is_some() && read.is_some());
        assert!(
            handle(&st, &ws.to_string(), None)
                .await
                .map_err(|e| e.message())
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_machine_coming_back_resumes_only_node_failures() {
        let (st, ws, sid) = fixture("offline").await;
        fail_with(&st, sid, ws, "agent 异常退出（退出码 255）。stderr: ssh: connect to host gpu-x port 22: No route to host").await;
        record(&st, sid).await;
        let other = WorkspaceId::new();
        let busy = SessionId::new();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, activity, created_at) VALUES (?1, 'n1', 'two', '/home/me/two', 'errored', '0')")
            .bind(other.to_string()).execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, status, failure_cause, created_at) VALUES (?1, ?2, 'claude', 'failed', 'busy', '1')")
            .bind(busy.to_string()).bind(other.to_string()).execute(st.db.pool()).await.unwrap();
        sqlx::query("UPDATE sessions SET request = '{\"text\":\"go\"}' WHERE id = ?1")
            .bind(sid.to_string())
            .execute(st.db.pool())
            .await
            .unwrap();
        let resumed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = resumed.clone();
        let send = move |_: Shared, ws: String, _: crate::api::PromptRequest| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(!ws.is_empty());
            Box::pin(async { Ok(json!({ "admitted": true })) })
                as futures::future::BoxFuture<'static, Result<Value, String>>
        };
        node_back_with(&st, "gpu-x", &send).await;
        assert_eq!(resumed.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(activity(&st, ws).await, "idle");
        assert_eq!(activity(&st, other).await, "errored");
        assert!(of_workspace(&st, &ws.to_string(), true).await.is_none());
        assert_eq!(
            of_workspace(&st, &other.to_string(), true)
                .await
                .map(|f| f.cause),
            Some(FailureCause::Busy)
        );
    }

    #[tokio::test]
    async fn resume_failures_never_surface_and_old_rows_classify_on_read() {
        let (st, ws, sid) = fixture("online").await;
        fail_with(&st, sid, ws, "agent 异常退出（退出码 1）。stderr: No message found with message.uuid of: 9b915e37-5c1e-4a7e-9d3b-2f1c0a8e7b6d").await;
        assert!(of_workspace(&st, &ws.to_string(), true).await.is_none());
        let (st, ws, sid) = fixture("online").await;
        fail_with(
            &st,
            sid,
            ws,
            "API Error: Connection lost while your computer was asleep",
        )
        .await;
        let view = of_workspace(&st, &ws.to_string(), true).await.unwrap();
        assert_eq!(view.cause, FailureCause::Network);
        assert_eq!(
            view.message,
            "API Error: Connection lost while your computer was asleep"
        );
    }
}
