use std::sync::Arc;
use std::time::Duration;

use blazar_core_types::{
    ApprovalDecision, ApprovalId, EntryKind, NormalizedEntry, Outcome, ProviderSessionId,
    SessionId, WorkspaceId,
};
use blazar_db::NewApproval;
use blazar_runtime::ImageInput;
use blazar_runtime::cli_runtime::{
    CliRuntime, approval_response_json, control_json, interrupt_json, user_input_line_with_images,
};
use blazar_transport::detached::{DetachedRun, RunState};
use chrono::Utc;
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::state::{AppState, RunningSession, ServerEvent};

type Shared = Arc<AppState>;

pub struct Live {
    pub run: DetachedRun,

    pub interactive: bool,
    pub state: Mutex<LiveState>,
}

pub struct LiveState {
    pub next_seq: u64,

    pub pending_user: u32,

    pub user_no: u32,
    pub eof_sent: bool,
    pub interrupt_requested: bool,
    pub interrupted_at: Option<String>,

    pub bg: std::collections::HashSet<String>,
    pub idle: bool,

    pub app: AppTurn,
}

#[derive(Debug, Default, Clone)]
pub struct AppTurn {
    pub enabled: bool,
    pub thread: Option<String>,
    pub turn: Option<String>,
    pub first: Option<(String, Vec<ImageInput>)>,
    pub steers: std::collections::HashMap<String, (String, Vec<ImageInput>)>,
}

pub fn apply_ids(app: &mut AppTurn, ids: &blazar_runtime::codex_app::Ids) {
    if let Some(t) = &ids.thread {
        app.thread = Some(t.clone());
    }
    if let Some(t) = &ids.turn {
        app.turn = Some(t.clone());
    }
    if ids.turn_ended {
        app.turn = None;
    }
}

async fn retry_steer(ctx: &Ctx, line: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let Some(cid) = v
        .get("id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| id.strip_prefix("blazar-steer-"))
    else {
        return;
    };
    let retry = {
        let mut s = ctx.live.state.lock().await;
        let pending = s.app.steers.remove(cid);
        match (v.get("error").is_some(), pending, s.app.thread.clone()) {
            (true, Some(p), Some(thread)) => Some((thread, p)),
            _ => None,
        }
    };
    if let Some((thread, (text, images))) = retry {
        let json = blazar_runtime::codex_app::turn_start(&thread, &text, &images, cid);
        if let Err(e) = ctx.live.run.append(&format!("t-{cid}"), &json).await {
            tracing::warn!(target: "blazar::run", "插话没赶上这一轮，另起一轮也失败了：{e}");
        }
    }
}

async fn drive_app(ctx: &Ctx, line: &str) {
    if line.contains("\"blazar-steer-") {
        retry_steer(ctx, line).await;
    }
    let ids = blazar_runtime::codex_app::ids(line);
    if ids == blazar_runtime::codex_app::Ids::default() {
        return;
    }
    let first = {
        let mut s = ctx.live.state.lock().await;
        apply_ids(&mut s.app, &ids);
        match (s.app.thread.clone(), ids.thread.is_some()) {
            (Some(thread), true) => s.app.first.take().map(|f| (thread, f)),
            _ => None,
        }
    };
    if let Some((thread, (text, images))) = first {
        let json = blazar_runtime::codex_app::first_turn(&thread, &text, &images, "u-1");
        if let Err(e) = ctx.live.run.append("u-1", &json).await {
            tracing::warn!(target: "blazar::run", "发不出第一轮：{e}");
        }
    }
}

#[must_use]
pub fn bg_alive(status: &str) -> bool {
    matches!(status, "started" | "running" | "pending")
}

const BG_SETTLE: Duration = Duration::from_secs(20);

fn settle_later(live: Arc<Live>) {
    tokio::spawn(async move {
        let seq = live.state.lock().await.next_seq;
        tokio::time::sleep(BG_SETTLE).await;
        let mut s = live.state.lock().await;
        if s.idle && s.bg.is_empty() && s.pending_user == 0 && !s.eof_sent && s.next_seq == seq {
            match live.run.eof().await {
                Ok(_) => s.eof_sent = true,
                Err(e) => tracing::warn!(target: "blazar::run", "发 EOF 失败: {e}"),
            }
        }
    });
}

pub async fn release_idle(st: &Shared, sid: SessionId) -> bool {
    let live = {
        let r = st.running.read().await;
        r.get(&sid).and_then(|x| x.live.clone())
    };
    let Some(live) = live else {
        return false;
    };
    let mut s = live.state.lock().await;
    if !live.interactive || !s.idle || s.eof_sent || !s.bg.is_empty() || s.pending_user > 0 {
        return false;
    }
    match live.run.eof().await {
        Ok(_) => {
            s.eof_sent = true;
            s.idle = false;
            true
        }
        Err(e) => {
            tracing::warn!(target: "blazar::run", "发 EOF 失败: {e}");
            false
        }
    }
}

pub async fn send_to_idle(
    st: &Shared,
    ws: WorkspaceId,
    thread: Option<&str>,
    runtime: Option<&str>,
    text: &str,
    sent: &str,
    images: &[ImageInput],
) -> Option<(SessionId, String)> {
    let _guard = st.quiesce(ws).ok()?;
    let (sid, live) = {
        let r = st.running.read().await;
        let (sid, run) = r.iter().find(|(_, x)| x.workspace_id == ws)?;
        (*sid, run.live.clone()?)
    };
    {
        let s = live.state.lock().await;
        if !live.interactive || !s.idle || s.eof_sent || s.interrupt_requested {
            return None;
        }
    }
    let (its_thread, its_runtime): (String, String) =
        sqlx::query_as("SELECT COALESCE(thread_id, id), runtime_kind FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()?;
    if thread.is_some_and(|t| t != its_thread) || runtime.is_some_and(|r| r != its_runtime) {
        return None;
    }
    live.state.lock().await.idle = false;
    interject(st, sid, text, sent, images).await.ok()?;
    Some((sid, its_thread))
}

impl Live {
    pub async fn take_seq(&self) -> u64 {
        let mut s = self.state.lock().await;
        let n = s.next_seq;
        s.next_seq += 1;
        n
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub st: Shared,
    pub sid: SessionId,
    pub ws: WorkspaceId,
    pub node: String,
    pub live: Arc<Live>,
    pub runtime: Arc<CliRuntime>,
    pub hook: blazar_hooks::HookContext,
}

#[must_use]
pub fn chain_uuid(line: &str) -> Option<String> {
    if !line.contains(r#""uuid""#) {
        return None;
    }
    let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
    if !matches!(v["type"].as_str(), Some("assistant" | "user"))
        || !v["parent_tool_use_id"].is_null()
        || !v["error"].is_null()
        || v["message"]["model"] == "<synthetic>"
        || v["isApiErrorMessage"] == true
    {
        return None;
    }
    v["uuid"].as_str().map(str::to_owned)
}

async fn note_chain_uuid(ctx: &Ctx, line: &str) {
    if let Some(uuid) = chain_uuid(line) {
        let _ = sqlx::query("UPDATE sessions SET last_uuid = ?2 WHERE id = ?1")
            .bind(ctx.sid.to_string())
            .bind(uuid)
            .execute(ctx.st.db.pool())
            .await;
    }
}

enum Line {
    Done,

    Partial,

    Exit,
}

struct Progress {
    started: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    saw_finished: bool,

    partial_at: Option<(u64, u32)>,
}

async fn process_line(ctx: &Ctx, line: &str, at: u64, p: &mut Progress) -> Line {
    let next = at + line.len() as u64 + 1;
    let t = line.trim();
    if t.is_empty() {
        return Line::Done;
    }
    if t.contains(r#""type":"blazar_exit""#) {
        return Line::Exit;
    }

    if t.starts_with('{') && serde_json::from_str::<serde_json::Value>(t).is_err() {
        let n = match p.partial_at {
            Some((a, n)) if a == at => n + 1,
            _ => 1,
        };
        p.partial_at = Some((at, n));
        if n < 3 {
            return Line::Partial;
        }
        tracing::warn!(target: "blazar::run", "偏移 {at} 处的行反复解析失败，跳过");
    }

    if ctx.runtime.app_server() {
        drive_app(ctx, t).await;
    }
    let parsed = ctx.runtime.parse_line(line);
    let mut entries = Vec::with_capacity(parsed.len());
    let mut approvals = Vec::new();
    let mut state = ctx.live.state.lock().await;
    for (mut kind, parent) in parsed {
        if state.interrupt_requested
            && matches!(&kind, EntryKind::BackgroundTask { status, .. }
                if matches!(status.as_str(), "killed" | "stopped"))
        {
            continue;
        }
        match &kind {
            EntryKind::Approval { id, request } => approvals.push(NewApproval {
                id: *id,
                provider_request_id: request["provider_request_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                src_offset: at,
                request: request.clone(),
            }),
            EntryKind::Finished(_) if state.interrupt_requested => {
                kind = EntryKind::Finished(Outcome::Interrupted);
            }
            _ => {}
        }
        entries.push(NormalizedEntry {
            seq: state.next_seq + entries.len() as u64,
            ts: Utc::now(),
            parent_tool_use_id: parent,
            kind,
        });
    }

    if let Err(e) = ctx
        .st
        .db
        .record_line(ctx.sid, ctx.ws, &entries, &approvals, next)
        .await
    {
        tracing::error!(target: "blazar::run", "落库失败，稍后重读: {e}");
        return Line::Partial;
    }
    state.next_seq += entries.len() as u64;
    for entry in &entries {
        match &entry.kind {
            EntryKind::InputConsumed { .. } => {
                state.pending_user = state.pending_user.saturating_sub(1);
            }
            EntryKind::Finished(_) => p.saw_finished = true,
            EntryKind::BackgroundTask {
                task_id, status, ..
            } => {
                if bg_alive(status) {
                    state.bg.insert(task_id.clone());
                } else if state.bg.remove(task_id) && state.bg.is_empty() && state.idle {
                    settle_later(ctx.live.clone());
                }
            }
            EntryKind::AssistantMessage { .. }
            | EntryKind::ToolUse { .. }
            | EntryKind::Thinking { .. } => state.idle = false,
            _ => {}
        }
    }
    drop(state);
    note_chain_uuid(ctx, t).await;
    for entry in &entries {
        if let EntryKind::SessionStarted {
            provider_session_id,
            ..
        } = &entry.kind
        {
            let _ = sqlx::query("UPDATE sessions SET provider_session_id = ?1 WHERE id = ?2")
                .bind(&provider_session_id.0)
                .bind(ctx.sid.to_string())
                .execute(ctx.st.db.pool())
                .await;
        }
    }

    triage_approvals(ctx, &approvals).await;
    for e in &entries {
        match &e.kind {
            EntryKind::ApprovalResolved { id, .. } => {
                crate::inbox::resolve_ref(&ctx.st, &id.to_string()).await;
            }
            EntryKind::RateLimit(rl) => {
                crate::accounts::note_rate_limit(&ctx.st, ctx.sid, rl).await;
            }
            EntryKind::Finished(Outcome::Failed { message }) | EntryKind::Error { message } => {
                crate::accounts::note_failure(&ctx.st, ctx.sid, message).await;
            }
            _ => {}
        }
    }

    for e in &entries {
        if let Some(tx) = p.started.take() {
            let verdict = match &e.kind {
                EntryKind::Finished(Outcome::Failed { message }) | EntryKind::Error { message } => {
                    Err(message.clone())
                }
                _ => Ok(()),
            };
            let _ = tx.send(verdict);
        }
    }
    let finished = entries
        .iter()
        .any(|e| matches!(e.kind, EntryKind::Finished(_)));
    let activity_changed = entries
        .iter()
        .any(|e| blazar_core_types::ActivityState::from_entry(&e.kind).is_some());
    for e in entries {
        ctx.st.emit(ServerEvent::Entry {
            workspace_id: ctx.ws,
            session_id: ctx.sid,
            entry: Box::new(e),
        });
    }
    if activity_changed {
        ctx.st.emit(ServerEvent::WorkspacesChanged);
    }

    if finished {
        if ctx.live.interactive {
            let hot = crate::bots::is_hot(&ctx.st, ctx.ws).await;
            let mut s = ctx.live.state.lock().await;
            if s.pending_user == 0 && !s.eof_sent {
                if s.bg.is_empty() && !hot {
                    match ctx.live.run.eof().await {
                        Ok(_) => s.eof_sent = true,
                        Err(e) => tracing::warn!(target: "blazar::run", "发 EOF 失败: {e}"),
                    }
                } else {
                    s.idle = true;
                }
            }
        }
        let (st, node, hook) = (ctx.st.clone(), ctx.node.clone(), ctx.hook.clone());
        ctx.st.services.spawn(async move {
            let t = st.transport(&node);
            crate::api::fire_hooks(&st, blazar_hooks::HookEvent::TurnEnd, hook, Some(t)).await;
        });
    }
    Line::Done
}

pub async fn register(
    ctx: Ctx,
    offset: u64,
    started: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) {
    let st = ctx.st.clone();
    let sid = ctx.sid;
    let ws = ctx.ws;
    let live = ctx.live.clone();
    let mut running = st.running.write().await;
    let task = tokio::spawn(async move {
        tokio::select! {
            () = supervise(ctx.clone(), offset, started) => {},
            () = retry_approvals(ctx) => {},
        }
    });
    running.insert(
        sid,
        RunningSession {
            workspace_id: ws,
            task,
            killer: None,
            live: Some(live),
        },
    );
}

/// 远端跟读这么久没有新输出，就探一下机器还在不在。
pub const QUIET_CHECK: Duration = Duration::from_secs(12);

/// 跟读时探测机器最多等多久；加上 [`QUIET_CHECK`]，断线约 20 秒内就能发现。
pub const LINK_TIMEOUT: Duration = Duration::from_secs(8);

async fn probe_link(ctx: &Ctx) -> Result<RunState, String> {
    match tokio::time::timeout(LINK_TIMEOUT, ctx.live.run.probe()).await {
        Ok(Ok(state)) => Ok(state),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {} s", LINK_TIMEOUT.as_secs())),
    }
}

/// 跟读连不上机器了：会话还在远端跑，记成“重连中”而不是失败。
pub(crate) fn link_lost(st: &Shared, sid: SessionId, node: &str, why: &str) {
    if st.health.set_reconnecting(sid, node) {
        tracing::info!(target: "blazar::run", "{node} 暂时连不上，{sid} 重连中: {why}");
        st.emit(ServerEvent::WorkspacesChanged);
    }
}

/// 跟读重新连上了。
pub(crate) fn link_back(st: &Shared, sid: SessionId) {
    if st.health.clear_reconnecting(sid) {
        tracing::info!(target: "blazar::run", "{sid} 重新连上");
        st.emit(ServerEvent::WorkspacesChanged);
    }
}

pub async fn supervise(
    ctx: Ctx,
    mut offset: u64,
    started: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
) {
    let mut p = Progress {
        started,
        saw_finished: false,
        partial_at: None,
    };
    let remote = ctx.node != "local";
    let mut backoff = 1u64;
    loop {
        match ctx.live.run.follow(offset).await {
            Ok(mut stream) => loop {
                let line = if remote {
                    match tokio::time::timeout(QUIET_CHECK, stream.stdout.next()).await {
                        Ok(line) => line,
                        Err(_) => match probe_link(&ctx).await {
                            Ok(_) => {
                                link_back(&ctx.st, ctx.sid);
                                continue;
                            }
                            Err(e) => {
                                link_lost(&ctx.st, ctx.sid, &ctx.node, &e);
                                break;
                            }
                        },
                    }
                } else {
                    stream.stdout.next().await
                };
                let Some(line) = line else {
                    break;
                };
                link_back(&ctx.st, ctx.sid);
                let at = offset;
                match process_line(&ctx, &line, at, &mut p).await {
                    Line::Done => {
                        offset = at + line.len() as u64 + 1;
                        backoff = 1;
                    }
                    Line::Exit => offset = at + line.len() as u64 + 1,
                    Line::Partial => break,
                }
            },
            Err(e) => tracing::warn!(target: "blazar::run", "跟读 {} 失败: {e}", ctx.sid),
        }

        let terminal = match probe_link(&ctx).await {
            Ok(state) => {
                link_back(&ctx.st, ctx.sid);
                match state {
                    RunState::Alive { .. } => None,
                    RunState::Exited { code, size } => Some((Some(code), size)),
                    RunState::Lost { size } => Some((None, size)),
                    RunState::Missing => Some((None, offset)),
                }
            }
            Err(e) => {
                link_lost(&ctx.st, ctx.sid, &ctx.node, &e);
                None
            }
        };
        if let Some((code, size)) = terminal
            && drain_remaining(&ctx, &mut offset, size, &mut p).await
        {
            match finalize(&ctx, code, &mut p).await {
                Ok(()) => {
                    link_back(&ctx.st, ctx.sid);
                    return;
                }
                Err(e) => tracing::error!(target: "blazar::run", "收尾落库失败，保留日志重试: {e}"),
            }
        }
        let wait = if ctx.st.health.reconnecting(ctx.sid).is_some() {
            backoff.min(10)
        } else {
            backoff
        };
        tokio::time::sleep(Duration::from_secs(wait)).await;
        backoff = (backoff * 2).min(30);
    }
}

/// 最后一轮已经正常收尾、之后也没有新消息：进程随后怎么退出的（hub 重启、被杀）
/// 都不算这段会话失败，否则重试会把干完的活整段回卷。
async fn last_turn_succeeded(ctx: &Ctx) -> bool {
    sqlx::query_scalar::<_, String>(
        "SELECT payload FROM events WHERE session_id = ?1
           AND (payload LIKE '{\"type\":\"finished\"%' OR payload LIKE '{\"type\":\"user_message\"%')
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(ctx.sid.to_string())
    .fetch_optional(ctx.st.db.pool())
    .await
    .ok()
    .flatten()
    .is_some_and(|p| p.starts_with(r#"{"type":"finished","status":"success""#))
}

async fn drain_remaining(ctx: &Ctx, offset: &mut u64, size: u64, p: &mut Progress) -> bool {
    if *offset == size {
        return true;
    }
    if *offset > size {
        tracing::error!(target: "blazar::run", "原始日志长度小于已提交位置，保留会话待恢复");
        return false;
    }
    let rest = match ctx.live.run.drain(*offset).await {
        Ok(rest) => rest,
        Err(e) => {
            tracing::warn!(target: "blazar::run", "重读剩余日志失败，稍后重试: {e}");
            return false;
        }
    };
    for chunk in rest.split_inclusive('\n') {
        let Some(line) = chunk.strip_suffix('\n') else {
            return false;
        };
        if matches!(process_line(ctx, line, *offset, p).await, Line::Partial) {
            return false;
        }
        *offset += chunk.len() as u64;
    }
    *offset >= size
}

async fn finalize(ctx: &Ctx, code: Option<i32>, p: &mut Progress) -> Result<(), String> {
    let interrupted = ctx.live.state.lock().await.interrupt_requested;

    if !p.saw_finished {
        p.saw_finished = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM events WHERE session_id = ?1 AND payload LIKE '{\"type\":\"finished\"%')",
        )
        .bind(ctx.sid.to_string())
        .fetch_one(ctx.st.db.pool())
        .await
        .map_err(|e| e.to_string())?;
    }
    if !p.saw_finished {
        let outcome = if interrupted {
            Outcome::Interrupted
        } else {
            let tail = ctx.live.run.stderr_tail().await.unwrap_or_default();
            let tail = tail.trim();
            Outcome::Failed {
                message: match code {
                    Some(c) => format!(
                        "agent exited unexpectedly (exit code {c}){}{}",
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!(". stderr: {tail}")
                        },
                        if c == 127 && tail.contains("not found") {
                            let program = ctx.runtime.spec().label;
                            if ctx.node == "local" {
                                format!(". {program} was not found on this machine: install it, or pick another agent")
                            } else {
                                format!(
                                    ". {program} is not installed on {}: install it under Machines → {}, or pick another machine or agent",
                                    ctx.node, ctx.node
                                )
                            }
                        } else if ctx.node != "local" && tail.contains("unknown option") {
                            format!(
                                ". The CLI on {} is older than the local one and does not accept this option: update it under Machines → {} to match this machine",
                                ctx.node, ctx.node
                            )
                        } else {
                            String::new()
                        }
                    ),
                    None => "agent process is gone (machine rebooted, process group killed, or directory removed); \
                             resume the previous conversation to try again"
                        .to_owned(),
                },
            }
        };
        let mut state = ctx.live.state.lock().await;
        let e = NormalizedEntry {
            seq: state.next_seq,
            ts: Utc::now(),
            parent_tool_use_id: None,
            kind: EntryKind::Finished(outcome),
        };
        ctx.st
            .db
            .append_event(ctx.sid, ctx.ws, &e)
            .await
            .map_err(|e| e.to_string())?;
        state.next_seq += 1;
        p.saw_finished = true;
        drop(state);
        if let EntryKind::Finished(Outcome::Failed { message }) = &e.kind {
            crate::accounts::note_failure(&ctx.st, ctx.sid, message).await;
            if let Some(tx) = p.started.take() {
                let _ = tx.send(Err(message.clone()));
            }
        }
        ctx.st.emit(ServerEvent::Entry {
            workspace_id: ctx.ws,
            session_id: ctx.sid,
            entry: Box::new(e),
        });
    }
    let status = if interrupted {
        "interrupted"
    } else if code == Some(0) || last_turn_succeeded(ctx).await {
        "done"
    } else {
        "failed"
    };
    let mut tx = ctx
        .st
        .db
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query(
        "UPDATE approvals SET decision = 'aborted', decided_at = ?1
         WHERE session_id = ?2 AND decision IS NULL",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(ctx.sid.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query("UPDATE sessions SET status = ?1, exit_code = ?2 WHERE id = ?3")
        .bind(status)
        .bind(code)
        .bind(ctx.sid.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    ctx.st.emit(ServerEvent::WorkspacesChanged);
    if status == "failed" {
        crate::failures::record(&ctx.st, ctx.sid).await;
    }
    crate::inbox::resolve_session(&ctx.st, ctx.sid).await;
    ctx.st
        .services
        .spawn(crate::titles::maybe_title(ctx.st.clone(), ctx.sid, ctx.ws));
    ctx.st.running.write().await.remove(&ctx.sid);

    ctx.st.services.spawn(crate::tasks::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));
    ctx.st.services.spawn(crate::brief::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        ctx.ws,
        status,
    ));
    ctx.st.services.spawn(crate::scripts::on_run_finished(
        ctx.st.clone(),
        ctx.ws,
        status,
    ));
    ctx.st.services.spawn(crate::autopilot::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));

    crate::fleet::revoke(&ctx.st, ctx.sid).await;
    ctx.st.services.spawn(crate::bots::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));
    ctx.st.services.spawn(crate::accounts::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));

    let interrupted_at = ctx.live.state.lock().await.interrupted_at.clone();
    ctx.st.services.spawn(crate::chat::on_run_finished(
        ctx.st.clone(),
        ctx.ws,
        ctx.sid,
        status,
        interrupted_at,
    ));
    ctx.st.services.spawn(crate::auto_resume::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));
    ctx.st.services.spawn(crate::resume::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));

    if code.is_some()
        && let Err(e) = ctx.live.run.remove().await
    {
        tracing::warn!(target: "blazar::run", "清理已保存的日志失败: {e}");
    }
    ctx.st.emit(ServerEvent::WorkspacesChanged);
    Ok(())
}

pub async fn interject(
    st: &Shared,
    sid: SessionId,
    text: &str,
    sent: &str,
    images: &[ImageInput],
) -> Result<u64, String> {
    let (live, ws) = {
        let r = st.running.read().await;
        let run = r.get(&sid).ok_or("Session is not running")?;
        (
            run.live
                .clone()
                .ok_or("This session is not persistent, so messages can't be sent mid-turn")?,
            run.workspace_id,
        )
    };
    if !live.interactive {
        return Err("This agent doesn't support messages mid-turn".into());
    }
    let seq = {
        let mut s = live.state.lock().await;

        if s.eof_sent {
            return Err("This turn has already finished; start a new one".into());
        }
        if s.interrupt_requested {
            return Err("This turn is being interrupted; the message will start a new turn".into());
        }
        s.user_no += 1;
        let id = format!("u-{}", s.user_no);

        let json = if s.app.enabled {
            if crate::api::is_slash_command(text) {
                return Err(
                    "Codex can't run slash commands mid-turn; send it after this turn ends".into(),
                );
            }
            let (Some(thread), Some(turn)) = (s.app.thread.clone(), s.app.turn.clone()) else {
                return Err("Codex hasn't started this turn yet; try again in a moment".into());
            };
            s.app
                .steers
                .insert(id.clone(), (sent.to_owned(), images.to_vec()));
            blazar_runtime::codex_app::steer(&thread, &turn, sent, images, &id)
        } else {
            let line = user_input_line_with_images(&id, sent, images);
            line.split_once('\t').map_or("", |x| x.1).to_owned()
        };
        live.run
            .append(&id, &json)
            .await
            .map_err(|e| format!("Write failed: {e}"))?;
        if !crate::api::is_slash_command(text) {
            s.pending_user += 1;
        }
        let n = s.next_seq;
        s.next_seq += 1;
        n
    };
    let e = NormalizedEntry {
        seq,
        ts: Utc::now(),
        parent_tool_use_id: None,
        kind: EntryKind::UserMessage {
            text: crate::api::with_image_note(text, images.len()),
        },
    };
    st.db
        .append_event(sid, ws, &e)
        .await
        .map_err(|e| e.to_string())?;
    st.emit(ServerEvent::Entry {
        workspace_id: ws,
        session_id: sid,
        entry: Box::new(e),
    });
    Ok(seq)
}

pub async fn deliver_approval(
    st: &Shared,
    a: &blazar_db::PendingApproval,
    allow: bool,
    message: &str,
    answers: Option<&serde_json::Value>,
) -> Result<(), String> {
    deliver_approval_as(st, a, allow, message, answers, None).await
}

pub async fn deliver_approval_as(
    st: &Shared,
    a: &blazar_db::PendingApproval,
    allow: bool,
    message: &str,
    answers: Option<&serde_json::Value>,
    auto_rule: Option<String>,
) -> Result<(), String> {
    if let Some(saved) = &a.response
        && (a.decision.as_deref() != Some(if allow { "allow" } else { "deny" })
            || saved.message != message
            || saved.answers.as_ref() != answers
            || saved.auto_rule != auto_rule)
    {
        return Err(
            "Delivery doesn't match the saved approval decision; retry with the original decision"
                .into(),
        );
    }
    deliver_saved_approval(st, a).await
}

pub(crate) async fn deliver_saved_approval(
    st: &Shared,
    a: &blazar_db::PendingApproval,
) -> Result<(), String> {
    let response = a
        .response
        .as_ref()
        .ok_or("This approval has no saved reply to recover, so it can't be retried safely. Interrupt the session and make the request again")?;
    let allow = match a.decision.as_deref() {
        Some("allow") => true,
        Some("deny") => false,
        _ => return Err("No decision has been saved for this approval".into()),
    };
    let (live, ws) = {
        let r = st.running.read().await;
        let run = r
            .get(&a.session_id)
            .ok_or("Session is no longer running; this approval is void")?;
        (
            run.live
                .clone()
                .ok_or("Session doesn't support approvals")?,
            run.workspace_id,
        )
    };
    let off = a
        .src_offset
        .ok_or("Missing the location of the original request")?;
    let raw = live
        .run
        .line_at(off)
        .await
        .map_err(|e| format!("Failed to read the original request: {e}"))?
        .ok_or("The original request is no longer on the node")?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let msg = if v.get("request_id").is_none() && v.get("method").is_some() && v.get("id").is_some()
    {
        let rid = blazar_runtime::codex_app::request_id_text(&v)
            .ok_or("The original request has no id")?;
        if a.provider_request_id.as_deref() != Some(rid.as_str()) {
            return Err("The original approval request has changed; interrupt the session and make the request again".into());
        }
        blazar_runtime::codex_app::approval_reply(&raw, allow)
            .ok_or("This is not a Codex approval request")?
    } else {
        let rid = v["request_id"]
            .as_str()
            .ok_or("The original request has no request_id")?;
        if a.provider_request_id.as_deref() != Some(rid) {
            return Err("The original approval request has changed; interrupt the session and make the request again".into());
        }
        let mut input = v["request"]["input"].clone();
        if let (true, Some(ans)) = (allow, response.answers.as_ref()) {
            let obj = input
                .as_object_mut()
                .ok_or("The original approval input is incomplete; can't write the reply")?;
            obj.insert("answers".into(), ans.clone());
        }
        approval_response_json(rid, allow, Some(&input), &response.message)
    };
    let mut state = live.state.lock().await;
    live.run
        .append(&format!("a-{}", a.id), &msg)
        .await
        .map_err(|e| format!("Failed to write agent input: {e}"))?;
    let e = NormalizedEntry {
        seq: state.next_seq,
        ts: Utc::now(),
        parent_tool_use_id: None,
        kind: EntryKind::ApprovalResolved {
            id: a.id,
            decision: if let (true, Some(rule)) = (allow, response.auto_rule.as_ref()) {
                ApprovalDecision::AutoAllowed { rule: rule.clone() }
            } else if allow {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Deny {
                    message: response.message.clone(),
                }
            },
        },
    };
    let fresh = st
        .db
        .acknowledge_approval(a.id, a.session_id, ws, &e)
        .await
        .map_err(|e| {
            format!("Input was written, but confirming delivery failed; safe to retry: {e}")
        })?;
    if fresh {
        state.next_seq += 1;
    }
    drop(state);
    if fresh {
        st.emit(ServerEvent::Entry {
            workspace_id: ws,
            session_id: a.session_id,
            entry: Box::new(e),
        });
        st.emit(ServerEvent::WorkspacesChanged);
    }
    crate::inbox::resolve_ref(st, &a.id.to_string()).await;
    Ok(())
}

async fn retry_approvals(ctx: Ctx) {
    loop {
        match ctx.st.db.undelivered_approvals(ctx.sid).await {
            Ok(list) => {
                for approval in list {
                    if let Err(error) = deliver_saved_approval(&ctx.st, &approval).await {
                        tracing::warn!(target: "blazar::run", "审批 {} 暂未送达: {error}", approval.id);
                        let updated = sqlx::query(
                            "UPDATE inbox SET body = ?2 WHERE ref_id = ?1 AND archived_at IS NULL AND body <> ?2",
                        ).bind(approval.id.to_string()).bind(&error).execute(ctx.st.db.pool()).await;
                        if updated.is_ok_and(|result| result.rows_affected() > 0) {
                            ctx.st.emit(ServerEvent::InboxChanged);
                        }
                    }
                }
            }
            Err(error) => tracing::warn!(target: "blazar::run", "读取待投递审批失败: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn triage_approvals(ctx: &Ctx, approvals: &[NewApproval]) {
    for ap in approvals {
        let ws = ctx.ws.to_string();
        if let Some(rule) = crate::rules::find(&ctx.st, &ws, &ap.request).await {
            let label = if rule.pattern.is_empty() {
                rule.tool.clone()
            } else {
                format!("{} {}", rule.tool, rule.pattern)
            };
            let by = format!("rule:{}", rule.id);
            let response = blazar_db::ApprovalResponse {
                auto_rule: Some(label.clone()),
                ..Default::default()
            };
            if let Ok(Some(a)) = ctx
                .st
                .db
                .decide_approval_with_response(ap.id, "allow", Some(&by), &response)
                .await
            {
                match deliver_saved_approval(&ctx.st, &a).await {
                    Ok(()) => continue,
                    Err(e) => tracing::warn!(target: "blazar::run", "自动批准没送到: {e}"),
                }
            }
        }
        let tool = ap.request["tool_name"].as_str().unwrap_or_default();
        let short = tool.rsplit("__").next().unwrap_or(tool);
        let question = short == "AskUserQuestion";
        let input = &ap.request["input"];
        let what = if question {
            input["questions"][0]["question"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        } else {
            input["command"]
                .as_str()
                .or_else(|| input["file_path"].as_str())
                .or_else(|| input["path"].as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let thread: Option<String> =
            sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
                .bind(ctx.sid.to_string())
                .fetch_optional(ctx.st.db.pool())
                .await
                .ok()
                .flatten();
        crate::inbox::push(
            &ctx.st,
            crate::inbox::Item {
                kind: if question { "question" } else { "approval" },
                title: if question {
                    format!("在问你：{what}")
                } else {
                    format!("等你批准：{}", approval_line(short, input))
                },
                body: what,
                workspace_id: Some(ws),
                thread_id: thread,
                session_id: Some(ctx.sid.to_string()),
                ref_id: Some(ap.id.to_string()),
            },
        )
        .await;
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

pub(crate) fn approval_line(short: &str, input: &serde_json::Value) -> String {
    let field = |k: &str| {
        input[k]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.lines().next().unwrap_or_default().to_owned())
    };
    if blazar_core_types::denial::is_shell(short) {
        return match (field("description"), field("command")) {
            (Some(d), _) => d,
            (None, Some(c)) => format!("运行 {}", c.chars().take(60).collect::<String>()),
            (None, None) => "运行一条命令".to_owned(),
        };
    }
    let file = field("file_path")
        .or_else(|| field("notebook_path"))
        .or_else(|| field("path"));
    match short {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => match file {
            Some(f) => format!("改 {}", basename(&f)),
            None => "改一个文件".to_owned(),
        },
        "Read" => match file {
            Some(f) => format!("读 {}", basename(&f)),
            None => "读一个文件".to_owned(),
        },
        "WebFetch" | "WebSearch" => "访问网页".to_owned(),
        "send_prompt" | "start_task" => "给工作区派活".to_owned(),
        "job_spawn" | "job_register" => "启动后台任务".to_owned(),
        "create_workspace" => "新建工作区".to_owned(),
        other => format!("使用 {other}"),
    }
}

pub async fn control(
    st: &Shared,
    sid: SessionId,
    request: serde_json::Value,
) -> Result<(), String> {
    let live = {
        let r = st.running.read().await;
        let run = r.get(&sid).ok_or("Session is not running")?;
        run.live.clone().ok_or("This session is not persistent")?
    };
    if !live.interactive {
        return Err("This agent doesn't support switching mid-turn".into());
    }
    let s = live.state.lock().await;
    if s.eof_sent {
        return Err("This turn has already finished".into());
    }
    if s.app.enabled {
        return Err(
            "Codex can't switch mid-turn; the next message will use the new settings".into(),
        );
    }
    let id = format!("c-{}", uuid::Uuid::now_v7());
    live.run
        .append(&id, &control_json(&id, request))
        .await
        .map(|_| ())
        .map_err(|e| format!("Write failed: {e}"))
}

pub async fn interrupt(st: &Shared, live: Arc<Live>, sid: SessionId) {
    let soft = {
        let mut s = live.state.lock().await;
        s.interrupt_requested = true;
        s.interrupted_at
            .get_or_insert_with(|| Utc::now().to_rfc3339());
        if live.interactive && !s.eof_sent {
            let id = format!("i-{}", uuid::Uuid::now_v7());
            if s.app.enabled {
                match (s.app.thread.clone(), s.app.turn.clone()) {
                    (Some(thread), Some(turn)) => live
                        .run
                        .append(
                            &id,
                            &blazar_runtime::codex_app::interrupt(&thread, &turn, &id),
                        )
                        .await
                        .is_ok(),
                    _ => false,
                }
            } else {
                live.run.append(&id, &interrupt_json(&id)).await.is_ok()
            }
        } else {
            false
        }
    };
    if soft {
        let st = st.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if st.running.read().await.contains_key(&sid) {
                let _ = live.run.hard_kill().await;
            }
        });
    } else {
        let _ = live.run.hard_kill().await;
    }
}

pub async fn reattach_all(st: Shared) {
    let rows: Vec<(String, String, String, String, i64, String)> = match sqlx::query_as(
        "SELECT s.id, s.workspace_id, s.node, s.run_dir, s.out_offset, s.runtime_kind
         FROM sessions s WHERE s.status = 'running' AND s.run_dir IS NOT NULL",
    )
    .fetch_all(st.db.pool())
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(target: "blazar::run", "读取待重连会话失败: {e}");
            return;
        }
    };
    for (sid, ws, node, dir, off, agent) in rows {
        let (Ok(sid_u), Ok(ws_u)) = (sid.parse(), ws.parse()) else {
            continue;
        };
        let (sid, ws) = (SessionId(sid_u), WorkspaceId(ws_u));
        let t = st.transport(&node);
        let Some(runtime) = CliRuntime::by_id(t.clone(), &agent) else {
            continue;
        };
        let run = DetachedRun::attach(t, dir, sid.to_string());
        let next_seq = st.db.max_seq(sid).await.unwrap_or(0) + 1;
        let mut recovered = AppTurn {
            enabled: runtime.app_server(),
            ..AppTurn::default()
        };
        if recovered.enabled {
            for line in run.drain(0).await.unwrap_or_default().lines() {
                apply_ids(&mut recovered, &blazar_runtime::codex_app::ids(line));
            }
        }

        let count = |pat: &'static str| {
            let st = st.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND payload LIKE ?2",
                )
                .bind(sid.to_string())
                .bind(pat)
                .fetch_one(st.db.pool())
                .await
                .unwrap_or(0)
            }
        };
        let users = count(r#"{"type":"user_message"%"#).await;

        let slash = count(r#"{"type":"user_message","text":"/%"#).await;
        let consumed = count(r#"{"type":"input_consumed"%"#).await;
        let mut bg = std::collections::HashSet::new();
        for p in sqlx::query_scalar::<_, String>(
            "SELECT payload FROM events WHERE session_id = ?1
               AND payload LIKE '{\"type\":\"background_task\"%' ORDER BY seq",
        )
        .bind(sid.to_string())
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&p) else {
                continue;
            };
            if let (Some(id), Some(status)) = (v["task_id"].as_str(), v["status"].as_str()) {
                if bg_alive(status) {
                    bg.insert(id.to_owned());
                } else {
                    bg.remove(id);
                }
            }
        }
        let live = Arc::new(Live {
            interactive: runtime.interactive(),
            run,
            state: Mutex::new(LiveState {
                next_seq,
                pending_user: u32::try_from((users - slash - consumed).max(0)).unwrap_or(0),
                user_no: u32::try_from(users).unwrap_or(0),
                eof_sent: false,
                interrupt_requested: false,
                interrupted_at: None,
                bg,
                idle: false,
                app: recovered,
            }),
        });
        let hook = blazar_hooks::HookContext {
            workspace: ws.to_string(),
            node: node.clone(),
            cwd: String::new(),
            session_id: sid.to_string(),
            tool_name: None,
            extra: Default::default(),
        };
        let ctx = Ctx {
            st: st.clone(),
            sid,
            ws,
            node,
            live: live.clone(),
            runtime: Arc::new(runtime),
            hook,
        };
        register(ctx, u64::try_from(off).unwrap_or(0), None).await;

        let last_finished = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM events WHERE session_id = ?1 ORDER BY seq DESC LIMIT 1",
        )
        .bind(sid.to_string())
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .is_some_and(|p| p.starts_with(r#"{"type":"finished""#));
        if last_finished && live.interactive {
            let mut s = live.state.lock().await;
            if s.pending_user == 0 && !s.eof_sent {
                if !s.bg.is_empty() {
                    s.idle = true;
                } else if live.run.eof().await.is_ok() {
                    s.eof_sent = true;
                }
            }
        }
        tracing::info!(target: "blazar::run", "已重新附着会话 {sid}（偏移 {off}）");
    }
}

#[must_use]
pub fn new_provider_session_id() -> ProviderSessionId {
    ProviderSessionId(uuid::Uuid::now_v7().to_string())
}

pub fn parse_approval_id(s: &str) -> Option<ApprovalId> {
    s.parse().ok().map(ApprovalId)
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
