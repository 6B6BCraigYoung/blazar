use std::collections::BTreeMap;
use std::time::Duration;

use blazar_core_types::{SessionId, WorkspaceId};
use blazar_runtime::cli_runtime::CliRuntime;
use blazar_transport::detached::{DetachedRun, ProbeItem, RunState, probe_many};
use chrono::Utc;

use crate::api::Shared;
use crate::run::Rejoin;
use crate::state::ServerEvent;

const LOOKBACK_DAYS: i64 = 14;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

const STOP_TIMEOUT: Duration = Duration::from_secs(20);

static GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub const TAKEN_BACK: &str = "The agent was still running after all, so Blazar picked this run back up. Ignore the failure above.";

pub const STOPPED: &str = "An agent from this run was still going in the background. Blazar stopped it and everything it started, so it can't clash with newer work in this conversation.";

const TAKEN_BACK_TITLE: &str = "接回了还在运行的一轮";

const TAKEN_BACK_BODY: &str = "这一轮之前被判成「进程没了」，其实智能体还在运行。Blazar 已经把它接回来继续跟进，不再算作失败。";

const STOPPED_TITLE: &str = "停掉了后台残留的智能体";

const STOPPED_BODY: &str = "这条对话里有一轮早先的智能体还在后台运行，而对话已经往下走了。为免两个智能体同时改同一份代码，Blazar 已经把它连同它启动的子进程一起结束。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    TakeBack,
    Stop,
}

#[must_use]
pub fn verdict(
    sid: &str,
    newest: Option<&str>,
    running: bool,
    took_one: bool,
    may_take_back: bool,
) -> Verdict {
    if may_take_back && !running && !took_one && newest == Some(sid) {
        Verdict::TakeBack
    } else {
        Verdict::Stop
    }
}

#[derive(Debug, Clone)]
struct Stale {
    sid: String,
    ws: String,
    thread: String,
    node: String,
    dir: String,
    agent: String,
    offset: i64,
    created: String,
}

impl Stale {
    fn run(&self, st: &Shared) -> DetachedRun {
        DetachedRun::attach(st.transport(&self.node), self.dir.clone(), self.sid.clone())
    }
}

#[derive(Debug, Default)]
struct Settled {
    took_back: bool,
    failed: Option<String>,
}

type Row = (String, String, String, String, String, String, i64, String);

async fn stale(st: &Shared, thread: Option<(&str, &str)>) -> Vec<Stale> {
    let since = (Utc::now() - chrono::Duration::days(LOOKBACK_DAYS)).to_rfc3339();
    let (ws, thread) = thread.unzip();
    sqlx::query_as::<_, Row>(
        "SELECT id, workspace_id, COALESCE(thread_id, id), COALESCE(node, 'local'), run_dir,
                runtime_kind, out_offset, created_at
         FROM sessions
         WHERE status <> 'running' AND exit_code IS NULL AND run_dir IS NOT NULL
           AND created_at > ?1
           AND (?2 IS NULL OR (workspace_id = ?2 AND COALESCE(thread_id, id) = ?3))
         ORDER BY created_at DESC",
    )
    .bind(since)
    .bind(ws)
    .bind(thread)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .into_iter()
    .map(
        |(sid, ws, thread, node, dir, agent, offset, created)| Stale {
            sid,
            ws,
            thread,
            node,
            dir,
            agent,
            offset,
            created,
        },
    )
    .collect()
}

async fn still_running(st: &Shared, rows: Vec<Stale>) -> Vec<Stale> {
    let mut by_node: BTreeMap<String, Vec<Stale>> = BTreeMap::new();
    for r in rows {
        by_node.entry(r.node.clone()).or_default().push(r);
    }
    let mut live = Vec::new();
    for (node, rows) in by_node {
        let items: Vec<ProbeItem> = rows
            .iter()
            .map(|r| ProbeItem {
                dir: r.dir.clone(),
                ..ProbeItem::default()
            })
            .collect();
        let transport = st.transport(&node);
        match tokio::time::timeout(PROBE_TIMEOUT, probe_many(&transport, &items)).await {
            Ok(Ok(probed)) => live.extend(
                rows.into_iter()
                    .zip(probed)
                    .filter(|(_, p)| matches!(p.state, RunState::Alive { .. }))
                    .map(|(r, _)| r),
            ),
            Ok(Err(e)) => {
                tracing::info!(target: "blazar::leftover", "{node} 上的旧运行这次查不了: {e}");
            }
            Err(_) => tracing::info!(
                target: "blazar::leftover",
                "{node} {} 秒内没有回应，旧运行这次不查",
                PROBE_TIMEOUT.as_secs()
            ),
        }
    }
    live.sort_by(|a, b| b.created.cmp(&a.created));
    live
}

async fn alive_now(st: &Shared, r: &Stale) -> bool {
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, r.run(st).probe()).await,
        Ok(Ok(RunState::Alive { .. }))
    )
}

async fn newest_in(st: &Shared, ws: &str, thread: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT id FROM sessions WHERE workspace_id = ?1 AND COALESCE(thread_id, id) = ?2
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(ws)
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
}

async fn running_in(st: &Shared, ws: &str, thread: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE workspace_id = ?1
           AND COALESCE(thread_id, id) = ?2 AND status = 'running')",
    )
    .bind(ws)
    .bind(thread)
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(true)
}

async fn stop(st: &Shared, r: &Stale) -> Result<(), String> {
    match tokio::time::timeout(STOP_TIMEOUT, r.run(st).hard_kill()).await {
        Ok(Ok(())) => {
            tracing::info!(target: "blazar::leftover", "停掉了 {} 上还在后台运行的 {}", r.node, r.sid);
            if let Ok(sid) = r.sid.parse().map(SessionId) {
                crate::auto_resume::note(st, sid, &r.ws, STOPPED.to_owned()).await;
            }
            Ok(())
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {} s", STOP_TIMEOUT.as_secs())),
    }
}

async fn take_back_one(st: &Shared, r: &Stale) -> bool {
    let (Ok(sid), Ok(ws)) = (r.sid.parse().map(SessionId), r.ws.parse().map(WorkspaceId)) else {
        return false;
    };
    let _ = crate::failures::handle(st, &r.ws, Some(&r.sid)).await;
    let claimed = sqlx::query(
        "UPDATE sessions SET status = 'running', exit_code = NULL, failure_cause = NULL
         WHERE id = ?1 AND status <> 'running'",
    )
    .bind(&r.sid)
    .execute(st.db.pool())
    .await
    .is_ok_and(|done| done.rows_affected() == 1);
    if !claimed {
        return false;
    }
    crate::auto_resume::note(st, sid, &r.ws, TAKEN_BACK.to_owned()).await;
    let joined = rejoin(
        st.clone(),
        Rejoin {
            sid,
            ws,
            node: r.node.clone(),
            dir: r.dir.clone(),
            offset: u64::try_from(r.offset).unwrap_or(0),
            agent: r.agent.clone(),
        },
    )
    .await;
    st.emit(ServerEvent::WorkspacesChanged);
    tracing::info!(target: "blazar::leftover", "{} 上的 {} 其实还在运行，已接回", r.node, r.sid);
    joined
}

fn rejoin(st: Shared, r: Rejoin) -> futures::future::BoxFuture<'static, bool> {
    Box::pin(async move { crate::run::rejoin(&st, r, true).await })
}

async fn report(st: &Shared, r: &Stale, title: &str, body: &str) {
    let (label, _) = crate::auto_resume::place(st, &r.ws).await;
    crate::inbox::push(
        st,
        crate::inbox::Item {
            kind: "report",
            title: format!("{label} · {title}"),
            body: body.to_owned(),
            workspace_id: Some(r.ws.clone()),
            thread_id: Some(r.thread.clone()),
            session_id: Some(r.sid.clone()),
            ref_id: None,
        },
    )
    .await;
}

async fn settle(
    st: &Shared,
    ws: &str,
    thread: &str,
    live: Vec<Stale>,
    may_take_back: bool,
    announce: bool,
) -> Settled {
    let _gate = GATE.lock().await;
    let newest = newest_in(st, ws, thread).await;
    let running = running_in(st, ws, thread).await;
    let mut out = Settled::default();
    for r in &live {
        if !alive_now(st, r).await {
            continue;
        }
        let followable = CliRuntime::by_id(st.transport(&r.node), &r.agent).is_some();
        let wanted = verdict(
            &r.sid,
            newest.as_deref(),
            running,
            out.took_back,
            may_take_back && followable,
        );
        if wanted == Verdict::TakeBack {
            if take_back_one(st, r).await {
                out.took_back = true;
                if announce {
                    report(st, r, TAKEN_BACK_TITLE, TAKEN_BACK_BODY).await;
                }
            }
            continue;
        }
        match stop(st, r).await {
            Ok(()) => {
                if announce {
                    report(st, r, STOPPED_TITLE, STOPPED_BODY).await;
                }
            }
            Err(e) => {
                tracing::warn!(target: "blazar::leftover", "{} 上的 {} 停不掉: {e}", r.node, r.sid);
                out.failed.get_or_insert(e);
            }
        }
    }
    out
}

pub async fn sweep(st: Shared) {
    let rows = stale(&st, None).await;
    if rows.is_empty() {
        return;
    }
    let mut threads: Vec<((String, String), Vec<Stale>)> = Vec::new();
    for r in still_running(&st, rows).await {
        let key = (r.ws.clone(), r.thread.clone());
        match threads.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.push(r),
            None => threads.push((key, vec![r])),
        }
    }
    for ((ws, thread), group) in threads {
        settle(&st, &ws, &thread, group, true, true).await;
    }
}

pub async fn take_back(st: &Shared, ws: &str, thread: &str) -> bool {
    let rows = stale(st, Some((ws, thread))).await;
    if rows.is_empty() {
        return false;
    }
    let live = still_running(st, rows).await;
    !live.is_empty() && settle(st, ws, thread, live, true, true).await.took_back
}

pub async fn clear(st: &Shared, ws: &str, thread: &str) -> Result<(), String> {
    let rows = stale(st, Some((ws, thread))).await;
    if rows.is_empty() {
        return Ok(());
    }
    let live = still_running(st, rows).await;
    if live.is_empty() {
        return Ok(());
    }
    match settle(st, ws, thread, live, false, false).await.failed {
        Some(e) => Err(format!(
            "An earlier run of this conversation is still going and could not be stopped ({e}). Stop it from the machine, then try again."
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "leftover_tests.rs"]
mod tests;
