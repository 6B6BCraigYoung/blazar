use std::time::Duration;

use blazar_core_types::failure::FailureCause;
use blazar_core_types::{EntryKind, NormalizedEntry, SessionId, WorkspaceId};
use serde_json::{Value, json};

use crate::api::{PromptRequest, Shared};
use crate::state::ServerEvent;

pub const BACKOFF_SECS: [u64; 3] = [15, 30, 60];

pub const DEFAULT_ATTEMPTS: u64 = 20;

pub const MAX_ATTEMPTS: u64 = 100;

const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com";

#[must_use]
pub fn eligible(cause: FailureCause) -> bool {
    matches!(cause, FailureCause::Network | FailureCause::Busy)
}

#[must_use]
pub fn delay(attempt: u64) -> Duration {
    let at = usize::try_from(attempt).unwrap_or(usize::MAX);
    Duration::from_secs(BACKOFF_SECS[at.min(BACKOFF_SECS.len() - 1)])
}

#[must_use]
pub fn exhausted(attempt: u64, limit: u64) -> bool {
    attempt >= limit
}

#[must_use]
pub fn settings_of(saved: &Value) -> Value {
    json!({
        "enabled": saved["auto_resume"]["enabled"].as_bool().unwrap_or(true),
        "max_attempts": saved["auto_resume"]["max_attempts"]
            .as_u64()
            .filter(|n| (1..=MAX_ATTEMPTS).contains(n))
            .unwrap_or(DEFAULT_ATTEMPTS),
    })
}

pub fn merge(cur: &mut Value, body: &Value) -> Result<(), String> {
    if let Some(v) = body["auto_resume"]["enabled"].as_bool() {
        cur["auto_resume"]["enabled"] = json!(v);
    }
    if let Some(n) = body["auto_resume"].get("max_attempts") {
        match n.as_u64().filter(|n| (1..=MAX_ATTEMPTS).contains(n)) {
            Some(n) => cur["auto_resume"]["max_attempts"] = json!(n),
            None => return Err(format!("自动续跑次数要在 1 到 {MAX_ATTEMPTS} 之间")),
        }
    }
    Ok(())
}

async fn settings(st: &Shared) -> (bool, u64) {
    let s = settings_of(&crate::office::kv_get(st, "prefs").await);
    (
        s["enabled"].as_bool().unwrap_or(true),
        s["max_attempts"].as_u64().unwrap_or(DEFAULT_ATTEMPTS),
    )
}

pub async fn takes_over(st: &Shared, sid: SessionId) -> bool {
    settings(st).await.0
        && crate::failures::cause_of(st, &sid.to_string())
            .await
            .is_some_and(eligible)
}

#[must_use]
pub fn continue_request(mut req: Value, thread: &str) -> Value {
    req["text"] = json!("继续");
    req["images"] = json!([]);
    req["context_file"] = Value::Null;
    req["resume"] = json!(true);
    req["resume_at_last"] = json!(false);
    req["resume_session"] = json!(thread);
    req["interject"] = json!(false);
    req["wait_secs"] = json!(0);
    req
}

fn send_again(
    st: Shared,
    ws: String,
    req: PromptRequest,
) -> futures::future::BoxFuture<'static, Result<Value, String>> {
    Box::pin(crate::api::prompt_for_bot(st, ws, req))
}

async fn reachable() -> bool {
    let endpoint = std::env::var("ANTHROPIC_BASE_URL")
        .ok()
        .filter(|u| u.starts_with("http"))
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return false;
    };
    client.head(endpoint).send().await.is_ok()
}

struct Failed {
    ws: String,
    thread: String,
    created: String,
    request: Option<String>,
}

async fn load(st: &Shared, sid: SessionId) -> Option<Failed> {
    let row: (String, String, String, Option<String>) = sqlx::query_as(
        "SELECT workspace_id, COALESCE(thread_id, id), created_at, request FROM sessions WHERE id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()?;
    Some(Failed {
        ws: row.0,
        thread: row.1,
        created: row.2,
        request: row.3,
    })
}

async fn streak(st: &Shared, f: &Failed) -> u64 {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions s WHERE COALESCE(s.thread_id, s.id) = ?1
           AND s.status = 'failed' AND s.failure_cause IN ('network', 'busy')
           AND s.created_at < ?2
           AND s.created_at > COALESCE((SELECT MAX(d.created_at) FROM sessions d
                 WHERE COALESCE(d.thread_id, d.id) = ?1 AND d.status <> 'failed'
                   AND d.created_at < ?2), '')",
    )
    .bind(&f.thread)
    .bind(&f.created)
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0);
    u64::try_from(n).unwrap_or(0)
}

async fn still_waiting(st: &Shared, sid: SessionId) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sessions s WHERE s.id = ?1 AND s.status = 'failed'
           AND s.handled_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM sessions n WHERE n.workspace_id = s.workspace_id
                           AND n.created_at > s.created_at)",
    )
    .bind(sid.to_string())
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0)
        > 0
}

async fn note(st: &Shared, sid: SessionId, ws: &str, message: String) {
    let Ok(id) = ws.parse().map(WorkspaceId) else {
        return;
    };
    let Ok(seq) = st.db.max_seq(sid).await else {
        return;
    };
    let entry = NormalizedEntry {
        seq: seq + 1,
        ts: chrono::Utc::now(),
        parent_tool_use_id: None,
        kind: EntryKind::Notice { message },
    };
    if st.db.append_event(sid, id, &entry).await.is_ok() {
        st.emit(ServerEvent::Entry {
            workspace_id: id,
            session_id: sid,
            entry: Box::new(entry),
        });
    }
}

async fn give_up(st: &Shared, sid: SessionId, f: &Failed, why: &str) {
    note(st, sid, &f.ws, format!("自动续跑停了：{why}")).await;
    let label = sqlx::query_as::<_, (String, String)>(
        "SELECT w.name, n.name FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(&f.ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|(name, node)| blazar_core_types::api::workspace_label(&name, &node))
    .unwrap_or_default();
    let message = crate::failures::last_message(st, &sid.to_string()).await;
    crate::inbox::push(
        st,
        crate::inbox::Item {
            kind: "run_failed",
            title: format!("{label} · 网络没恢复，没能自动继续"),
            body: format!("{why}\n{}", message.chars().take(200).collect::<String>()),
            workspace_id: Some(f.ws.clone()),
            thread_id: Some(f.thread.clone()),
            session_id: Some(sid.to_string()),
            ref_id: None,
        },
    )
    .await;
}

pub async fn on_run_finished(st: Shared, sid: SessionId, status: &'static str) {
    if status != "failed" || !takes_over(&st, sid).await {
        return;
    }
    let limit = settings(&st).await.1;
    let Some(f) = load(&st, sid).await else {
        return;
    };
    let Some(req) = f
        .request
        .as_deref()
        .and_then(|r| serde_json::from_str::<Value>(r).ok())
    else {
        give_up(&st, sid, &f, "没有留下原始请求").await;
        return;
    };
    let start = streak(&st, &f).await;
    if exhausted(start, limit) {
        give_up(&st, sid, &f, &format!("已连续自动续跑 {start} 次")).await;
        return;
    }
    note(
        &st,
        sid,
        &f.ws,
        format!("网络恢复后会自动继续（最多再试 {} 次）", limit - start),
    )
    .await;
    let mut attempt = start;
    while !exhausted(attempt, limit) {
        tokio::time::sleep(delay(attempt)).await;
        attempt += 1;
        if !still_waiting(&st, sid).await {
            tracing::info!(target: "blazar::auto_resume", "{sid} 已被手动处理，取消自动续跑");
            return;
        }
        if !reachable().await {
            continue;
        }
        let Ok(parsed) =
            serde_json::from_value::<PromptRequest>(continue_request(req.clone(), &f.thread))
        else {
            give_up(&st, sid, &f, "原始请求读不回来").await;
            return;
        };
        note(
            &st,
            sid,
            &f.ws,
            format!("网络恢复了，自动继续（第 {attempt} 次）"),
        )
        .await;
        match send_again(st.clone(), f.ws.clone(), parsed).await {
            Ok(v) if v["admitted"] != json!(false) => {
                let _ = crate::failures::handle(&st, &f.ws, Some(&sid.to_string())).await;
                tracing::info!(target: "blazar::auto_resume", "{sid} 已自动继续");
                return;
            }
            Ok(v) => {
                tracing::info!(target: "blazar::auto_resume", "{sid} 自动继续没被接受：{v}");
            }
            Err(e) => tracing::warn!(target: "blazar::auto_resume", "{sid} 自动继续失败：{e}"),
        }
    }
    give_up(&st, sid, &f, &format!("试了 {limit} 次网络都没恢复")).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_network_kinds_resume() {
        let hit = [
            "API Error: getaddrinfo ENOTFOUND api.anthropic.com",
            "API Error: read ECONNRESET",
            "API Error: Connection lost while your computer was asleep",
            "API Error: Self-signed certificate detected",
            "API Error: 529 overloaded_error",
            "API Error: 503 Service Unavailable",
        ];
        for m in hit {
            assert!(eligible(FailureCause::of(m, true)), "{m}");
        }
        let miss = [
            "API Error: 429 rate_limit_error",
            "Not logged in · Please run /login",
            "No message found with message.uuid of: x",
            "ssh: connect to host gpu-1 port 22: No route to host",
            "agent 异常退出（退出码 1）",
        ];
        for m in miss {
            assert!(!eligible(FailureCause::of(m, true)), "{m}");
        }
        assert!(!eligible(FailureCause::of("API Error: ECONNRESET", false)));
    }

    #[test]
    fn backoff_caps_at_a_minute() {
        let secs: Vec<u64> = (0..6).map(|a| delay(a).as_secs()).collect();
        assert_eq!(secs, [15, 30, 60, 60, 60, 60]);
        assert_eq!(delay(u64::MAX).as_secs(), 60);
    }

    #[test]
    fn attempts_stop_at_the_limit() {
        assert!(!exhausted(0, DEFAULT_ATTEMPTS));
        assert!(!exhausted(19, DEFAULT_ATTEMPTS));
        assert!(exhausted(20, DEFAULT_ATTEMPTS));
        let saved = json!({ "auto_resume": { "max_attempts": 3 } });
        assert_eq!(settings_of(&saved)["max_attempts"], 3);
        assert_eq!(settings_of(&json!({}))["max_attempts"], DEFAULT_ATTEMPTS);
        assert_eq!(settings_of(&json!({}))["enabled"], true);
        let mut cur = json!({});
        assert!(merge(&mut cur, &json!({ "auto_resume": { "max_attempts": 0 } })).is_err());
        assert!(merge(&mut cur, &json!({ "auto_resume": { "max_attempts": 101 } })).is_err());
        merge(
            &mut cur,
            &json!({ "auto_resume": { "enabled": false, "max_attempts": 5 } }),
        )
        .unwrap();
        assert_eq!(
            cur["auto_resume"],
            json!({ "enabled": false, "max_attempts": 5 })
        );
    }

    #[test]
    fn continuing_keeps_the_permission_mode() {
        let stored = json!({ "text": "改一下", "permission_mode": "acceptEdits", "images": [{}], "model": "opus" });
        let r = continue_request(stored, "t1");
        assert_eq!(r["text"], "继续");
        assert_eq!(r["resume"], true);
        assert_eq!(r["resume_session"], "t1");
        assert_eq!(r["permission_mode"], "acceptEdits");
        assert_eq!(r["model"], "opus");
        assert_eq!(r["images"], json!([]));
    }
}
