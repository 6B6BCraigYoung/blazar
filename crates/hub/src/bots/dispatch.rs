use std::collections::HashSet;
use std::time::Duration;

use blazar_core_types::{SessionId, WorkspaceId};
use serde_json::Value;

use super::{Bot, Mail, status};
use crate::api::Shared;

pub const DEFAULT_MAX_CONCURRENT: u64 = 6;
pub const MAX_CONCURRENT_LIMIT: u64 = 16;
const TICK: Duration = Duration::from_secs(5);

#[must_use]
pub fn max_concurrent_of(prefs: &Value) -> u64 {
    prefs["bots"]["max_concurrent"]
        .as_u64()
        .filter(|n| (1..=64).contains(n))
        .unwrap_or(DEFAULT_MAX_CONCURRENT)
}

pub async fn max_concurrent(st: &Shared) -> u64 {
    max_concurrent_of(&crate::office::kv_get(st, "prefs").await)
}

pub async fn scheduler(st: Shared) {
    loop {
        tick(&st).await;
        tokio::select! {
            () = st.bots.wake.notified() => {}
            () = tokio::time::sleep(TICK) => {}
        }
    }
}

pub async fn bot_workspaces(st: &Shared) -> HashSet<WorkspaceId> {
    super::all(st)
        .await
        .into_iter()
        .flat_map(|b| {
            [Some(b.home_ws_id), b.brain_ws_id]
                .into_iter()
                .flatten()
                .filter_map(|w| w.parse().ok().map(WorkspaceId))
        })
        .collect()
}

pub async fn running_bots(st: &Shared) -> u64 {
    let ours = bot_workspaces(st).await;
    st.running
        .read()
        .await
        .values()
        .filter(|r| ours.contains(&r.workspace_id))
        .count() as u64
}

#[must_use]
pub fn order(bots: &mut [Bot], pending: &std::collections::HashMap<String, i64>) -> Vec<Bot> {
    let mut list: Vec<Bot> = bots
        .iter()
        .filter(|b| !b.paused() && pending.get(&b.id).copied().unwrap_or_default() > 0)
        .cloned()
        .collect();
    list.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(a.created_at.cmp(&b.created_at))
    });
    list
}

pub async fn tick(st: &Shared) {
    super::expire_due(st).await;
    let pending = super::pending_counts(st).await;
    let mut bots = super::all(st).await;
    let cap = max_concurrent(st).await;
    for bot in order(&mut bots, &pending) {
        let already_live = match run_ws(&bot) {
            Some(ws) => status::live_in(st, ws).await.is_some(),
            None => false,
        };
        if running_bots(st).await >= cap && !already_live {
            tracing::info!(target: "blazar::bots", "bot 并发已到上限 {cap}，{} 的邮件先排着", bot.handle);
            break;
        }
        dispatch_one(st, &bot).await;
    }
    release_cold(st).await;
    retire_idle(st).await;
}

pub async fn retire_idle(st: &Shared) {
    let now = chrono::Utc::now();
    for bot in super::all(st).await {
        let Some(ttl) = bot.idle_ttl_secs.filter(|_| bot.is_temporary()) else {
            continue;
        };
        if bot.hot() || !super::pending(st, &bot.id).await.is_empty() {
            continue;
        }
        if let Some(ws) = run_ws(&bot)
            && status::live_in(st, ws).await.is_some()
        {
            continue;
        }
        let since = bot.last_active_at.as_deref().unwrap_or(&bot.created_at);
        let Ok(at) = chrono::DateTime::parse_from_rfc3339(since) else {
            continue;
        };
        let idle = (now - at.with_timezone(&chrono::Utc)).num_seconds();
        if idle < ttl {
            continue;
        }
        if super::archive(st, &bot.id).await.is_ok() {
            tracing::info!(target: "blazar::bots", "临时 bot {} 闲置 {idle} 秒，已归档", bot.handle);
            crate::inbox::push(
                st,
                crate::inbox::Item {
                    kind: "run_done",
                    title: format!("临时 bot「{}」已归档", bot.display_name),
                    body: format!(
                        "闲置超过 {} 小时。家工作区和对话都还在，需要可以在 bot 页面恢复或转成固定 bot。",
                        ttl / 3600
                    ),
                    workspace_id: Some(bot.home_ws_id.clone()),
                    ref_id: Some(format!("bot:{}", bot.id)),
                    ..crate::inbox::Item::default()
                },
            )
            .await;
        }
    }
}

fn run_ws(bot: &Bot) -> Option<WorkspaceId> {
    bot.run_workspace().parse().ok().map(WorkspaceId)
}

async fn live_thread(st: &Shared, session: &str) -> Option<String> {
    let thread: String =
        sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
            .bind(session)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()?;
    super::thread_by_session_thread(st, &thread)
        .await
        .map(|t| t.id)
}

pub async fn dispatch_one(st: &Shared, bot: &Bot) {
    let mails = super::pending(st, &bot.id).await;
    if mails.is_empty() {
        return;
    }
    if let Some(node) = super::rules::offline_node_of(st, bot).await {
        tracing::debug!(target: "blazar::bots", "{} 的机器 {node} 离线，邮件先留着", bot.handle);
        return;
    }
    let Some(ws) = run_ws(bot) else {
        super::mark_failed(st, &mails.iter().map(|m| m.id.clone()).collect::<Vec<_>>()).await;
        return;
    };
    if let Some(live) = status::live_in(st, ws).await {
        let on_thread = live_thread(st, &live.session).await;
        let deliverable: Vec<Mail> = mails
            .into_iter()
            .filter(|m| m.thread_hint.is_none() || m.thread_hint == on_thread)
            .collect();
        if deliverable.is_empty() || !live.interactive {
            return;
        }
        let urgent = deliverable.iter().any(|m| m.priority == "urgent");
        if !urgent && !live.idle {
            return;
        }
        let text = super::render(st, &deliverable).await;
        let Ok(sid) = live.session.parse::<uuid::Uuid>().map(SessionId) else {
            return;
        };
        let delivered = if live.idle {
            crate::run::send_to_idle(st, ws, None, &text, &text, &[])
                .await
                .is_some()
        } else {
            crate::run::interject(st, sid, &text, &text, &[])
                .await
                .is_ok()
        };
        if delivered {
            let ids: Vec<String> = deliverable.iter().map(|m| m.id.clone()).collect();
            super::mark_delivered(st, &ids, sid).await;
            super::set_status(st, &bot.id, "running", "").await;
        }
        return;
    }
    {
        let mut starting = st
            .bots
            .starting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !starting.insert(bot.id.clone()) {
            return;
        }
    }
    let result = start_session(st, bot, &mails).await;
    st.bots
        .starting
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&bot.id);
    if let Err(e) = result {
        tracing::warn!(target: "blazar::bots", "{} 的会话起不来：{e}", bot.handle);
        super::mark_failed(st, &mails.iter().map(|m| m.id.clone()).collect::<Vec<_>>()).await;
        super::set_status(st, &bot.id, "idle", &format!("起不来：{e}")).await;
        crate::inbox::push(
            st,
            crate::inbox::Item {
                kind: "alert",
                title: format!("{} 没能启动", bot.display_name),
                body: e,
                workspace_id: Some(bot.home_ws_id.clone()),
                ref_id: Some(format!("bot:{}", bot.id)),
                ..crate::inbox::Item::default()
            },
        )
        .await;
    }
}

async fn start_session(st: &Shared, bot: &Bot, mails: &[Mail]) -> Result<(), String> {
    let first_hint = mails.iter().find_map(|m| m.thread_hint.clone());
    let thread = match first_hint {
        Some(id) => super::thread(st, &id)
            .await
            .filter(|t| t.closed_at.is_none())
            .ok_or("邮件指定的线程已经关闭")?,
        None => super::main_thread(st, &bot.id).await?,
    };
    let batch: Vec<Mail> = mails
        .iter()
        .filter(|m| m.thread_hint.is_none() || m.thread_hint.as_deref() == Some(thread.id.as_str()))
        .cloned()
        .collect();
    let text = super::render(st, &batch).await;
    let req = crate::api::PromptRequest {
        text,
        resume: thread.thread_id.is_some(),
        resume_session: thread.thread_id.clone(),
        profile: Some(bot.agent_id.clone()),
        brain: Some(if bot.brain == "node" { "node" } else { "local" }.into()),
        bot: Some(bot.id.clone()),
        ..crate::api::PromptRequest::default()
    };
    let out: Value =
        crate::api::prompt_for_bot(st.clone(), bot.run_workspace().to_owned(), req).await?;
    if out["admitted"] == false {
        return Err(out["reason"].as_str().unwrap_or("工作区被占着").to_owned());
    }
    if let Some(reason) = out["blocked_by_hook"].as_str() {
        return Err(format!("被 hook 拦下：{reason}"));
    }
    let sid = out["session_id"]
        .as_str()
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        .map(SessionId)
        .ok_or("hub 没有返回会话 id")?;
    if let Some(t) = out["thread_id"].as_str() {
        let runtime = crate::agents::get(st, &bot.agent_id)
            .await
            .map(|a| a.runtime)
            .unwrap_or_default();
        super::bind_thread(st, &thread.id, t, &runtime).await;
    }
    let ids: Vec<String> = batch.iter().map(|m| m.id.clone()).collect();
    super::mark_delivered(st, &ids, sid).await;
    super::set_status(st, &bot.id, "running", "").await;
    Ok(())
}

async fn release_cold(st: &Shared) {
    for bot in super::all(st).await {
        if bot.hot() {
            continue;
        }
        let Some(ws) = run_ws(&bot) else {
            continue;
        };
        if let Some(live) = status::live_in(st, ws).await
            && live.idle
            && super::pending(st, &bot.id).await.is_empty()
            && let Ok(sid) = live.session.parse::<uuid::Uuid>().map(SessionId)
        {
            crate::run::release_idle(st, sid).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot(id: &str, priority: &str, status: &str, created: &str) -> Bot {
        Bot {
            id: id.into(),
            handle: id.into(),
            display_name: id.into(),
            role: "worker".into(),
            agent_id: "a".into(),
            home_ws_id: "w".into(),
            brain: "hub".into(),
            brain_ws_id: None,
            priority: priority.into(),
            runtimes: Vec::new(),
            hot_until: None,
            status: status.into(),
            status_note: String::new(),
            last_active_at: None,
            archived_at: None,
            created_at: created.into(),
            updated_at: created.into(),
            lifetime: "fixed".into(),
            idle_ttl_secs: None,
        }
    }

    #[test]
    fn higher_priority_and_older_bots_go_first_and_paused_wait() {
        let mut bots = vec![
            bot("late-p1", "P1", "idle", "2026-10-02"),
            bot("p2", "P2", "idle", "2026-10-01"),
            bot("early-p1", "P1", "idle", "2026-10-01"),
            bot("paused-p0", "P0", "paused", "2026-10-01"),
            bot("quiet-p0", "P0", "idle", "2026-10-01"),
        ];
        let pending = [("late-p1", 1), ("p2", 3), ("early-p1", 1), ("paused-p0", 1)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        let got: Vec<String> = order(&mut bots, &pending)
            .into_iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(got, ["early-p1", "late-p1", "p2"]);
    }
}
