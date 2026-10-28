use blazar_core_types::SessionId;
use blazar_core_types::failure::{FailureCause, ResumeLoss, resume_loss};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use crate::api::{PromptRequest, Shared};
use crate::state::{AppState, ServerEvent};

pub const NOTE: &str =
    "The original session transcript could not be found; here is the conversation so far.";

#[must_use]
pub fn fallback_request(mut req: Value, message: &str, thread: &str) -> Option<(Value, bool)> {
    let loss = resume_loss(message)?;
    let anchored = req["resume_at_last"].as_bool() == Some(true);
    let fresh = !(loss == ResumeLoss::Message && anchored);
    req["resume"] = json!(!fresh);
    req["resume_at_last"] = json!(false);
    req["resume_session"] = json!(thread);
    req["retry_thread"] = json!(thread);
    req["interject"] = json!(false);
    req["wait_secs"] = json!(0);
    Some((req, fresh))
}

fn send_again(
    st: Shared,
    ws: String,
    req: PromptRequest,
) -> BoxFuture<'static, Result<Value, String>> {
    Box::pin(crate::api::prompt_for_bot(st, ws, req))
}

pub async fn on_run_finished(st: Shared, sid: SessionId, status: &'static str) {
    let sender = st.clone();
    fallback_using(&st, sid, status, move |ws, req| send_again(sender, ws, req)).await;
}

pub(crate) async fn fallback_using<F, Fut>(
    st: &Shared,
    sid: SessionId,
    status: &str,
    send: F,
) -> Option<Value>
where
    F: FnOnce(String, PromptRequest) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    if status != "failed"
        || crate::failures::cause_of(st, &sid.to_string()).await != Some(FailureCause::Resume)
    {
        return None;
    }
    let row: Option<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT workspace_id, request, COALESCE(thread_id, id), created_at FROM sessions WHERE id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let (ws, raw, thread, created) = row?;
    if crate::chat::did_work(st, &sid.to_string()).await
        || crate::chat::superseded(st, &thread, &created).await
    {
        tracing::info!(target: "blazar::resume", "{sid} 已经干过活或对话已往下走，不自动重发，留给失败卡片");
        return None;
    }
    let Some(req) = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()) else {
        tracing::warn!(target: "blazar::resume", "{sid} 的续接点丢了，但没有留下原始请求，没法自动重发");
        return None;
    };
    let message = crate::failures::last_message(st, &sid.to_string()).await;
    let (mut req, fresh) = fallback_request(req, &message, &thread)?;
    if let Err(e) = crate::failures::handle(st, &ws, Some(&sid.to_string())).await {
        tracing::warn!(target: "blazar::resume", "收起 {sid} 的续接失败时出错：{}", e.message());
    }
    let _ = sqlx::query("UPDATE sessions SET provider_session_id = NULL WHERE id = ?1")
        .bind(sid.to_string())
        .execute(st.db.pool())
        .await;
    if fresh {
        let text = req["text"].as_str().unwrap_or_default().to_owned();
        let preface = crate::chat::preface(st, &thread, &created, NOTE).await;
        req["text"] = json!(format!("{preface}{text}"));
    }
    let parsed = match serde_json::from_value::<PromptRequest>(req) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(target: "blazar::resume", "{sid} 的原始请求读不回来：{e}");
            reopen(st, sid, &ws, &e.to_string()).await;
            return None;
        }
    };
    let how = if fresh {
        "另开一个会话并带上前情"
    } else {
        "不带续接点再试一次"
    };
    match send(ws.clone(), parsed).await {
        Ok(v) if v["admitted"] != json!(false) => {
            tracing::info!(target: "blazar::resume", "{sid} 的续接点丢了，已{how}");
            Some(v)
        }
        Ok(v) => {
            tracing::warn!(target: "blazar::resume", "{sid} 续接回退没被接受：{v}");
            reopen(
                st,
                sid,
                &ws,
                v["reason"].as_str().unwrap_or("Workspace is busy"),
            )
            .await;
            None
        }
        Err(e) => {
            tracing::warn!(target: "blazar::resume", "{sid} 续接回退失败：{e}");
            reopen(st, sid, &ws, &e).await;
            None
        }
    }
}

async fn reopen(st: &AppState, sid: SessionId, ws: &str, why: &str) {
    let _ = sqlx::query("UPDATE sessions SET handled_at = NULL, failure_cause = ?2 WHERE id = ?1")
        .bind(sid.to_string())
        .bind(FailureCause::Other.as_str())
        .execute(st.db.pool())
        .await;
    let _ = sqlx::query("UPDATE workspaces SET activity = 'errored' WHERE id = ?1")
        .bind(ws)
        .execute(st.db.pool())
        .await;
    if let Ok(id) = ws.parse().map(blazar_core_types::WorkspaceId)
        && let Ok(seq) = st.db.max_seq(sid).await
    {
        let entry = blazar_core_types::NormalizedEntry {
            seq: seq + 1,
            ts: chrono::Utc::now(),
            parent_tool_use_id: None,
            kind: blazar_core_types::EntryKind::Error {
                message: format!(
                    "The original session transcript could not be found, and resending automatically failed: {why}"
                ),
            },
        };
        if st.db.append_event(sid, id, &entry).await.is_ok() {
            st.emit(ServerEvent::Entry {
                workspace_id: id,
                session_id: sid,
                entry: Box::new(entry),
            });
        }
    }
    st.emit(ServerEvent::WorkspacesChanged);
}

#[cfg(test)]
mod tests {
    use super::*;
    use blazar_core_types::{EntryKind, NormalizedEntry, Outcome, WorkspaceId};
    use chrono::Utc;
    use std::sync::{Arc, Mutex};

    const MISSING: &str = "agent exited unexpectedly (exit code 1). stderr: No message found with message.uuid of: 9b915e37-5c1e-4a7e-9d3b-2f1c0a8e7b6d";
    const GONE: &str =
        "No conversation found with session ID: 019a2c1e-7f3b-7b2a-9c4d-1e2f3a4b5c6d";

    #[test]
    fn a_missing_anchor_is_retried_without_it_and_a_missing_conversation_starts_over() {
        let stored = json!({ "text": "继续", "resume": true, "resume_at_last": true, "model": "claude-fable-5-1", "wait_secs": 20 });
        let (r, fresh) = fallback_request(stored.clone(), MISSING, "t1").unwrap();
        assert!(!fresh);
        assert_eq!(r["resume"], true);
        assert_eq!(r["resume_at_last"], false, "第二次不带 --resume-session-at");
        assert_eq!(r["resume_session"], "t1");
        assert_eq!(r["retry_thread"], "t1");
        assert_eq!(r["wait_secs"], 0);
        assert_eq!(r["model"], "claude-fable-5-1", "其他选项照旧");

        let (r, fresh) = fallback_request(stored.clone(), GONE, "t1").unwrap();
        assert!(fresh, "整段会话都没了就只能开新会话");
        assert_eq!(r["resume"], false);
        assert_eq!(r["retry_thread"], "t1", "新会话仍然留在同一个对话里");

        let unanchored = json!({ "text": "继续", "resume": true, "resume_at_last": false });
        let (_, fresh) = fallback_request(unanchored, MISSING, "t1").unwrap();
        assert!(fresh, "没带续接点还找不到消息，说明记录本身不对，开新会话");

        assert!(fallback_request(stored, "API Error: 529 overloaded", "t1").is_none());
    }

    async fn fixture() -> (Shared, WorkspaceId) {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-resume-tests")),
            crate::services::Services::Isolated,
        );
        let ws = WorkspaceId::new();
        sqlx::query("INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('node', 'local', 'local', 'online', '0')")
            .execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, activity, created_at) VALUES (?1, 'node', 'repo', '/home/me/repo', 'errored', '0')")
            .bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        (st, ws)
    }

    async fn event(st: &Shared, sid: SessionId, ws: WorkspaceId, seq: u64, kind: EntryKind) {
        st.db
            .append_event(
                sid,
                ws,
                &NormalizedEntry {
                    seq,
                    ts: Utc::now(),
                    parent_tool_use_id: None,
                    kind,
                },
            )
            .await
            .unwrap();
    }

    async fn failed_session(
        st: &Shared,
        ws: WorkspaceId,
        thread: &str,
        request: &Value,
        message: &str,
    ) -> SessionId {
        let sid = SessionId::new();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, thread_id, provider_session_id, request, failure_cause)
             VALUES (?1, ?2, 'claude', 'failed', '2026-10-25T10:00:00+00:00', ?3, 'p1', ?4, 'resume')",
        )
        .bind(sid.to_string()).bind(ws.to_string()).bind(thread).bind(request.to_string())
        .execute(st.db.pool()).await.unwrap();
        event(
            st,
            sid,
            ws,
            1,
            EntryKind::UserMessage {
                text: "继续".into(),
            },
        )
        .await;
        event(
            st,
            sid,
            ws,
            2,
            EntryKind::Finished(Outcome::Failed {
                message: message.into(),
            }),
        )
        .await;
        sid
    }

    type Seen = Arc<Mutex<Vec<(String, PromptRequest)>>>;

    fn capture() -> (
        Seen,
        impl FnOnce(String, PromptRequest) -> std::future::Ready<Result<Value, String>>,
    ) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        (seen, move |ws, req| {
            sink.lock().unwrap().push((ws, req));
            std::future::ready(Ok(json!({ "session_id": "new", "thread_id": "t1" })))
        })
    }

    async fn session_row(st: &Shared, sid: SessionId) -> (Option<String>, Option<String>) {
        sqlx::query_as("SELECT handled_at, provider_session_id FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_one(st.db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_lost_anchor_is_resent_in_the_same_conversation_without_the_anchor() {
        let (st, ws) = fixture().await;
        let thread = SessionId::new().to_string();
        let stored = json!({ "text": "继续", "resume": true, "resume_at_last": true, "model": "claude-fable-5-1" });
        let sid = failed_session(&st, ws, &thread, &stored, MISSING).await;
        let (seen, send) = capture();
        let out = fallback_using(&st, sid, "failed", send).await;
        assert!(out.is_some());
        {
            let sent = seen.lock().unwrap();
            let (to, req) = sent.first().expect("重发了一次");
            assert_eq!(*to, ws.to_string());
            assert_eq!(req.text, "继续");
            assert!(req.resume);
            assert!(!req.resume_at_last, "第二次不再带 --resume-session-at");
            assert_eq!(req.resume_session.as_deref(), Some(thread.as_str()));
            assert_eq!(req.retry_thread.as_deref(), Some(thread.as_str()));
            assert_eq!(req.model.as_deref(), Some("claude-fable-5-1"));
        }
        let (handled, pid) = session_row(&st, sid).await;
        assert!(handled.is_some(), "失败的那次对用户不可见");
        assert!(pid.is_none(), "也不再被当作续接的起点");
    }

    #[tokio::test]
    async fn a_lost_conversation_starts_a_new_session_with_the_story_so_far() {
        let (st, ws) = fixture().await;
        let earlier = SessionId::new();
        let thread = earlier.to_string();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, provider_session_id)
             VALUES (?1, ?2, 'claude', 'done', '2026-10-25T09:00:00+00:00', 'p1')",
        )
        .bind(earlier.to_string()).bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        event(
            &st,
            earlier,
            ws,
            1,
            EntryKind::UserMessage {
                text: "把首页改成三句话".into(),
            },
        )
        .await;
        event(
            &st,
            earlier,
            ws,
            2,
            EntryKind::AssistantMessage {
                text: "改好了两句，还差一句。".into(),
            },
        )
        .await;
        let stored = json!({ "text": "继续", "resume": true, "resume_at_last": false });
        let sid = failed_session(&st, ws, &thread, &stored, GONE).await;
        let (seen, send) = capture();
        assert!(fallback_using(&st, sid, "failed", send).await.is_some());
        let sent = seen.lock().unwrap();
        let (_, req) = sent.first().expect("开了新会话");
        assert!(!req.resume, "会话记录没了，不再续接");
        assert!(
            req.text.contains("把首页改成三句话"),
            "前情摘要带着之前的对话"
        );
        assert!(req.text.contains("改好了两句"));
        assert!(req.text.contains(NOTE));
        assert!(
            req.text.trim_end().ends_with("继续"),
            "最后才是用户这次的话"
        );
        assert_eq!(req.retry_thread.as_deref(), Some(thread.as_str()));
    }

    #[tokio::test]
    async fn a_failed_resend_puts_the_failure_back_in_front_of_the_user() {
        let (st, ws) = fixture().await;
        let thread = SessionId::new().to_string();
        let stored = json!({ "text": "继续", "resume": true, "resume_at_last": true });
        let sid = failed_session(&st, ws, &thread, &stored, MISSING).await;
        let out = fallback_using(&st, sid, "failed", |_, _| {
            std::future::ready(Err::<Value, String>("hub 没启动".into()))
        })
        .await;
        assert!(out.is_none());
        let (handled, _) = session_row(&st, sid).await;
        assert!(handled.is_none());
        let (cause, activity): (Option<String>, String) = sqlx::query_as(
            "SELECT s.failure_cause, w.activity FROM sessions s JOIN workspaces w ON w.id = s.workspace_id WHERE s.id = ?1",
        )
        .bind(sid.to_string())
        .fetch_one(st.db.pool())
        .await
        .unwrap();
        assert_eq!(cause.as_deref(), Some("other"));
        assert_eq!(activity, "errored");
    }

    #[tokio::test]
    async fn other_failures_and_finished_runs_are_left_alone() {
        let (st, ws) = fixture().await;
        let thread = SessionId::new().to_string();
        let stored = json!({ "text": "继续", "resume": true });
        let sid = failed_session(&st, ws, &thread, &stored, "API Error: 529 overloaded").await;
        sqlx::query("UPDATE sessions SET failure_cause = 'busy' WHERE id = ?1")
            .bind(sid.to_string())
            .execute(st.db.pool())
            .await
            .unwrap();
        let (seen, send) = capture();
        assert!(fallback_using(&st, sid, "failed", send).await.is_none());
        assert!(seen.lock().unwrap().is_empty());
        let (seen, send) = capture();
        assert!(fallback_using(&st, sid, "done", send).await.is_none());
        assert!(seen.lock().unwrap().is_empty());
    }
}
