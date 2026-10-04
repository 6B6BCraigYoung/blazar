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

    pub bg: std::collections::HashSet<String>,
    pub idle: bool,
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

pub async fn send_to_idle(
    st: &Shared,
    ws: WorkspaceId,
    thread: Option<&str>,
    text: &str,
    sent: &str,
    images: &[ImageInput],
) -> Option<(SessionId, String)> {
    let (sid, live) = {
        let r = st.running.read().await;
        let (sid, run) = r.iter().find(|(_, x)| x.workspace_id == ws)?;
        (*sid, run.live.clone()?)
    };
    {
        let s = live.state.lock().await;
        if !live.interactive || !s.idle || s.eof_sent {
            return None;
        }
    }
    let its_thread: String =
        sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()?;
    if thread.is_some_and(|t| t != its_thread) {
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

async fn note_chain_uuid(ctx: &Ctx, line: &str) {
    if !line.contains(r#""uuid""#) {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    if !matches!(v["type"].as_str(), Some("assistant" | "user"))
        || !v["parent_tool_use_id"].is_null()
    {
        return;
    }
    if let Some(uuid) = v["uuid"].as_str() {
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
                crate::inbox::note_rate_limit(&ctx.st, rl).await;
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
    for e in entries {
        ctx.st.emit(ServerEvent::Entry {
            workspace_id: ctx.ws,
            session_id: ctx.sid,
            entry: Box::new(e),
        });
    }
    ctx.st.emit(ServerEvent::WorkspacesChanged);

    if finished {
        if ctx.live.interactive {
            let mut s = ctx.live.state.lock().await;
            if s.pending_user == 0 && !s.eof_sent {
                if s.bg.is_empty() {
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
    let mut backoff = 1u64;
    loop {
        match ctx.live.run.follow(offset).await {
            Ok(mut stream) => {
                while let Some(line) = stream.stdout.next().await {
                    let at = offset;
                    match process_line(&ctx, &line, at, &mut p).await {
                        Line::Done => {
                            offset = at + line.len() as u64 + 1;
                            backoff = 1;
                        }
                        Line::Exit => offset = at + line.len() as u64 + 1,
                        Line::Partial => break,
                    }
                }
            }
            Err(e) => tracing::warn!(target: "blazar::run", "跟读 {} 失败: {e}", ctx.sid),
        }

        let terminal = match ctx.live.run.probe().await {
            Ok(RunState::Alive { .. }) => None,
            Ok(RunState::Exited { code, size }) => Some((Some(code), size)),
            Ok(RunState::Lost { size }) => Some((None, size)),
            Ok(RunState::Missing) => Some((None, offset)),
            Err(e) => {
                tracing::info!(target: "blazar::run", "{} 暂时连不上: {e}", ctx.node);
                None
            }
        };
        if let Some((code, size)) = terminal
            && drain_remaining(&ctx, &mut offset, size, &mut p).await
        {
            match finalize(&ctx, code, &mut p).await {
                Ok(()) => return,
                Err(e) => tracing::error!(target: "blazar::run", "收尾落库失败，保留日志重试: {e}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
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
                        "agent 异常退出（退出码 {c}）{}{}",
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!("。stderr: {tail}")
                        },
                        if ctx.node != "local" && tail.contains("unknown option") {
                            format!(
                                "。{} 上的 CLI 比本机旧，不认这个参数：到「机器 → {}」里把它更新到和本机一致",
                                ctx.node, ctx.node
                            )
                        } else {
                            String::new()
                        }
                    ),
                    None => "agent 进程已经不在了（机器重启、被整组杀掉或目录被删），\
                             可以续接上一段对话重来"
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
        }
        ctx.st.emit(ServerEvent::Entry {
            workspace_id: ctx.ws,
            session_id: ctx.sid,
            entry: Box::new(e),
        });
    }
    let status = if interrupted {
        "interrupted"
    } else if code == Some(0) {
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
    ctx.st.services.spawn(crate::inbox::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
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

    ctx.st.services.spawn(crate::accounts::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));

    ctx.st.services.spawn(crate::chat::on_run_finished(
        ctx.st.clone(),
        ctx.ws,
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
        let run = r.get(&sid).ok_or("会话不在运行")?;
        (
            run.live.clone().ok_or("这个会话不是常驻形态，不能插话")?,
            run.workspace_id,
        )
    };
    if !live.interactive {
        return Err("这个 agent 不支持运行中插话".into());
    }
    let seq = {
        let mut s = live.state.lock().await;

        if s.eof_sent {
            return Err("这一轮已经收尾，请开一轮新的".into());
        }
        s.user_no += 1;
        let id = format!("u-{}", s.user_no);

        let line = user_input_line_with_images(&id, sent, images);
        let json = line.split_once('\t').map_or("", |x| x.1);
        live.run
            .append(&id, json)
            .await
            .map_err(|e| format!("写入失败: {e}"))?;
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
    let (live, ws) = {
        let r = st.running.read().await;
        let run = r
            .get(&a.session_id)
            .ok_or("会话已经不在运行，这条审批作废了")?;
        (run.live.clone().ok_or("会话不支持审批")?, run.workspace_id)
    };
    let off = a.src_offset.ok_or("缺少原始请求的位置")?;
    let raw = live
        .run
        .line_at(off)
        .await
        .map_err(|e| format!("取原始请求失败: {e}"))?
        .ok_or("原始请求已经不在节点上了")?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let rid = v["request_id"].as_str().ok_or("原始请求缺 request_id")?;

    let mut input = v["request"]["input"].clone();
    if let (true, Some(ans), Some(obj)) = (allow, answers, input.as_object_mut()) {
        obj.insert("answers".into(), ans.clone());
    }
    let msg = approval_response_json(rid, allow, Some(&input), message);
    {
        let _guard = live.state.lock().await;
        live.run
            .append(&format!("a-{}", a.id), &msg)
            .await
            .map_err(|e| format!("写入 agent 输入失败: {e}"))?;
    }
    let _ = st.db.mark_approval_delivered(a.id).await;
    let e = NormalizedEntry {
        seq: live.take_seq().await,
        ts: Utc::now(),
        parent_tool_use_id: None,
        kind: EntryKind::ApprovalResolved {
            id: a.id,
            decision: if let (true, Some(rule)) = (allow, auto_rule) {
                ApprovalDecision::AutoAllowed { rule }
            } else if allow {
                ApprovalDecision::Allow
            } else {
                ApprovalDecision::Deny {
                    message: message.to_owned(),
                }
            },
        },
    };
    let _ = st.db.append_event(a.session_id, ws, &e).await;
    st.emit(ServerEvent::Entry {
        workspace_id: ws,
        session_id: a.session_id,
        entry: Box::new(e),
    });
    st.emit(ServerEvent::WorkspacesChanged);
    crate::inbox::resolve_ref(st, &a.id.to_string()).await;
    Ok(())
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
            if let Ok(Some(a)) = ctx.st.db.decide_approval(ap.id, "allow", Some(&by)).await {
                match deliver_approval_as(&ctx.st, &a, true, "", None, Some(label)).await {
                    Ok(()) => continue,
                    Err(e) => tracing::warn!(target: "blazar::run", "自动批准没送到: {e}"),
                }
            }
        }
        let tool = ap.request["tool_name"].as_str().unwrap_or_default();
        let short = tool.rsplit("__").next().unwrap_or(tool);
        let question = short == "AskUserQuestion";
        let ws_name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = ?1")
            .bind(&ws)
            .fetch_optional(ctx.st.db.pool())
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
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
                    format!("{ws_name} · agent 在问你")
                } else {
                    format!("{ws_name} · 等你裁决：{short}")
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

pub async fn control(
    st: &Shared,
    sid: SessionId,
    request: serde_json::Value,
) -> Result<(), String> {
    let live = {
        let r = st.running.read().await;
        let run = r.get(&sid).ok_or("会话不在运行")?;
        run.live.clone().ok_or("这个会话不是常驻形态")?
    };
    if !live.interactive {
        return Err("这个 agent 不支持运行中切换".into());
    }
    let s = live.state.lock().await;
    if s.eof_sent {
        return Err("这一轮已经收尾".into());
    }
    let id = format!("c-{}", uuid::Uuid::now_v7());
    live.run
        .append(&id, &control_json(&id, request))
        .await
        .map(|_| ())
        .map_err(|e| format!("写入失败: {e}"))
}

pub async fn interrupt(st: &Shared, live: Arc<Live>, sid: SessionId) {
    let soft = {
        let mut s = live.state.lock().await;
        s.interrupt_requested = true;
        if live.interactive && !s.eof_sent {
            let id = format!("i-{}", uuid::Uuid::now_v7());
            live.run.append(&id, &interrupt_json(&id)).await.is_ok()
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
                bg,
                idle: false,
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
        let task = tokio::spawn(supervise(ctx, u64::try_from(off).unwrap_or(0), None));
        st.running.write().await.insert(
            sid,
            RunningSession {
                workspace_id: ws,
                task,
                killer: None,
                live: Some(live.clone()),
            },
        );

        if let Ok(list) = st.db.undelivered_approvals(sid).await {
            for a in list {
                let allow = a.decision.as_deref() == Some("allow");
                let _ = deliver_approval(&st, &a, allow, "", None).await;
            }
        }

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
