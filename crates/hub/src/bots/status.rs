use blazar_core_types::WorkspaceId;
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::Row;

use super::{Bot, claims, pending_counts, s, threads};
use crate::api::Shared;

#[must_use]
pub fn summarize(kind: &Value) -> Option<String> {
    let short = |t: &str, n: usize| {
        let mut out: String = t.trim().chars().take(n).collect();
        if t.trim().chars().count() > n {
            out.push('…');
        }
        out.replace('\n', " ")
    };
    match kind["type"].as_str()? {
        "user_message" => Some(format!("用户：{}", short(kind["text"].as_str()?, 160))),
        "assistant_message" => Some(format!("回复：{}", short(kind["text"].as_str()?, 160))),
        "tool_use" => {
            let name = kind["name"].as_str()?;
            let input = &kind["input"];
            let detail = input["command"]
                .as_str()
                .or(input["file_path"].as_str())
                .or(input["path"].as_str())
                .or(input["pattern"].as_str())
                .or(input["description"].as_str())
                .map(|d| short(d, 100))
                .unwrap_or_default();
            Some(if detail.is_empty() {
                format!("工具 {name}")
            } else {
                format!("工具 {name}：{detail}")
            })
        }
        "approval" => Some("等待审批".into()),
        "finished" => Some("这一轮结束".into()),
        _ => None,
    }
}

pub struct LiveInfo {
    pub session: String,
    pub idle: bool,
    pub interactive: bool,
}

pub async fn live_in(st: &Shared, ws: WorkspaceId) -> Option<LiveInfo> {
    let running = st.running.read().await;
    let (sid, run) = running.iter().find(|(_, r)| r.workspace_id == ws)?;
    let (idle, interactive) = match &run.live {
        Some(l) => (l.state.lock().await.idle, l.interactive),
        None => (false, false),
    };
    Some(LiveInfo {
        session: sid.to_string(),
        idle,
        interactive,
    })
}

async fn latest_session(st: &Shared, ws: &str) -> Option<(String, String, String)> {
    sqlx::query(
        "SELECT id, status, created_at FROM sessions WHERE workspace_id = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|r| (s(&r, "id"), s(&r, "status"), s(&r, "created_at")))
}

async fn recent_entries(st: &Shared, session: &str, n: usize) -> (Option<String>, Vec<String>) {
    let rows =
        sqlx::query("SELECT payload FROM events WHERE session_id = ?1 ORDER BY seq DESC LIMIT 400")
            .bind(session)
            .fetch_all(st.db.pool())
            .await
            .unwrap_or_default();
    let kinds: Vec<Value> = rows
        .iter()
        .filter_map(|r| serde_json::from_str(&s(r, "payload")).ok())
        .collect();
    let last_instruction = kinds
        .iter()
        .find(|k| k["type"] == "user_message")
        .and_then(|k| k["text"].as_str())
        .map(|t| t.chars().take(300).collect::<String>());
    let mut recent: Vec<String> = kinds
        .iter()
        .filter(|k| k["type"] != "user_message")
        .filter_map(summarize)
        .take(n)
        .collect();
    recent.reverse();
    (last_instruction, recent)
}

pub async fn node_status(st: &Shared, ws: &str) -> Option<(String, String)> {
    sqlx::query(
        "SELECT n.name, n.status FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|r| (s(&r, "name"), s(&r, "status")))
}

pub async fn bot_status(st: &Shared, bot: &Bot, recent: usize) -> Value {
    let run_ws = bot.run_workspace().to_owned();
    let live = match run_ws.parse::<uuid::Uuid>() {
        Ok(u) => live_in(st, WorkspaceId(u)).await,
        Err(_) => None,
    };
    let activity: String = sqlx::query_scalar("SELECT activity FROM workspaces WHERE id = ?1")
        .bind(&run_ws)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let node = node_status(st, &run_ws).await;
    let node_offline = node
        .as_ref()
        .is_some_and(|(n, st)| n != "local" && st != "online");
    let latest = latest_session(st, &run_ws).await;
    let state = if bot.paused() {
        "paused"
    } else if bot.archived_at.is_some() {
        "archived"
    } else if node_offline && bot.brain == "node" {
        "lost"
    } else if activity == "awaiting_approval" {
        "awaiting"
    } else if live.is_some() || activity == "running" {
        "running"
    } else {
        "idle"
    };
    let (last_instruction, steps) = match (&live, &latest) {
        (Some(l), _) if recent > 0 => recent_entries(st, &l.session, recent).await,
        (_, Some((id, _, _))) if recent > 0 => recent_entries(st, id, recent).await,
        _ => (None, Vec::new()),
    };
    let pending = pending_counts(st).await;
    let claims: Vec<Value> = claims::of_bot(st, &bot.id)
        .await
        .into_iter()
        .map(|c| {
            json!({
                "id": c.id, "node": c.node, "scope": c.scope, "mode": c.mode,
                "workspace_id": c.workspace_id, "hands": c.hands, "note": c.note,
                "expires_at": c.expires_at,
            })
        })
        .collect();
    let open_threads: Vec<Value> = threads(st, &bot.id)
        .await
        .into_iter()
        .filter(|t| t.closed_at.is_none())
        .map(|t| {
            json!({
                "id": t.id, "kind": t.kind, "title": t.title, "thread_id": t.thread_id,
                "runtime": t.runtime, "has_summary": !t.summary.is_empty(),
            })
        })
        .collect();
    json!({
        "id": bot.id,
        "handle": bot.handle,
        "display_name": bot.display_name,
        "role": bot.role,
        "agent_id": bot.agent_id,
        "priority": bot.priority,
        "brain": bot.brain,
        "home_ws_id": bot.home_ws_id,
        "brain_ws_id": bot.brain_ws_id,
        "run_ws_id": run_ws,
        "runtimes": bot.runtimes,
        "state": state,
        "status_note": bot.status_note,
        "hot": bot.hot(),
        "hot_until": bot.hot_until,
        "paused": bot.paused(),
        "archived": bot.archived_at.is_some(),
        "archived_at": bot.archived_at,
        "lifetime": bot.lifetime,
        "temporary": bot.is_temporary(),
        "idle_ttl_secs": bot.idle_ttl_secs,
        "node": node.as_ref().map(|(n, _)| n.clone()),
        "node_status": node.as_ref().map(|(_, s)| s.clone()),
        "live_session": live.as_ref().map(|l| l.session.clone()),
        "live_idle": live.as_ref().map(|l| l.idle),
        "live_interactive": live.as_ref().map(|l| l.interactive),
        "last_session": latest.as_ref().map(|(id, status, at)| json!({ "id": id, "status": status, "created_at": at })),
        "last_instruction": last_instruction,
        "recent": steps,
        "pending_mail": pending.get(&bot.id).copied().unwrap_or(0),
        "claims": claims,
        "threads": open_threads,
        "last_active_at": bot.last_active_at,
        "created_at": bot.created_at,
    })
}

pub async fn fleet_status(st: &Shared, recent: usize) -> Value {
    let mut bots = Vec::new();
    for b in super::all(st).await {
        bots.push(bot_status(st, &b, recent).await);
    }
    let nodes: Vec<Value> = sqlx::query("SELECT name, status, last_seen_at FROM nodes ORDER BY name")
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| json!({ "name": s(r, "name"), "status": s(r, "status"), "last_seen_at": super::opt(r, "last_seen_at") }))
        .collect();
    let quota: Vec<Value> = sqlx::query(
        "SELECT a.id, a.provider, a.display_name, a.status, a.disabled, r.window_name, r.utilization, r.resets_at
         FROM accounts a LEFT JOIN rate_limit_snapshots r ON r.account_id = a.id
         ORDER BY a.provider, a.display_name, r.window_name",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| {
        json!({
            "account": s(r, "id"), "provider": s(r, "provider"), "label": s(r, "display_name"),
            "status": s(r, "status"), "disabled": r.try_get::<i64, _>("disabled").unwrap_or(0) != 0,
            "window": super::opt(r, "window_name"),
            "utilization": r.try_get::<f64, _>("utilization").ok(),
            "resets_at": super::opt(r, "resets_at"),
        })
    })
    .collect();
    let alerts: Vec<Value> = sqlx::query(
        "SELECT kind, title, body, created_at FROM inbox
         WHERE archived_at IS NULL AND kind IN ('alert', 'run_failed', 'approval', 'question')
         ORDER BY created_at DESC LIMIT 20",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| json!({ "kind": s(r, "kind"), "title": s(r, "title"), "body": s(r, "body"), "at": s(r, "created_at") }))
    .collect();
    let facts: Vec<Value> = super::facts::list(st, "", 200)
        .await
        .into_iter()
        .filter(|f| f.status != "ok")
        .map(|f| json!({ "key": f.key, "status": f.status, "owner": f.owner_handle, "updated_at": f.updated_at }))
        .collect();
    json!({
        "generated_at": Utc::now().to_rfc3339(),
        "bots": bots,
        "nodes": nodes,
        "quota": quota,
        "alerts": alerts,
        "unhealthy_facts": facts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_summarize_to_one_line() {
        assert_eq!(
            summarize(
                &json!({ "type": "tool_use", "name": "Bash", "input": { "command": "ls -la\nwc" } })
            )
            .unwrap(),
            "工具 Bash：ls -la wc"
        );
        assert_eq!(
            summarize(&json!({ "type": "assistant_message", "text": "好的" })).unwrap(),
            "回复：好的"
        );
        assert!(summarize(&json!({ "type": "token_usage" })).is_none());
        let long = "x".repeat(200);
        let out = summarize(&json!({ "type": "user_message", "text": long })).unwrap();
        assert!(out.ends_with('…') && out.chars().count() < 170);
    }
}
