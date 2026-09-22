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

    note_chain_uuid(ctx, t).await;
    let parsed = ctx.runtime.parse_line(line);
    let mut entries = Vec::with_capacity(parsed.len());
    let mut approvals = Vec::new();
    let interrupting = ctx.live.state.lock().await.interrupt_requested;
    for (mut kind, parent) in parsed {
        if interrupting
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
            EntryKind::InputConsumed { .. } => {
                let mut s = ctx.live.state.lock().await;
                s.pending_user = s.pending_user.saturating_sub(1);
            }
            EntryKind::SessionStarted {
                provider_session_id,
                ..
            } => {
                let _ = sqlx::query("UPDATE sessions SET provider_session_id = ?1 WHERE id = ?2")
                    .bind(&provider_session_id.0)
                    .bind(ctx.sid.to_string())
                    .execute(ctx.st.db.pool())
                    .await;
            }
            EntryKind::Finished(_) => {
                p.saw_finished = true;
                if ctx.live.state.lock().await.interrupt_requested {
                    kind = EntryKind::Finished(Outcome::Interrupted);
                }
            }
            _ => {}
        }
        entries.push(NormalizedEntry {
            seq: ctx.live.take_seq().await,
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

    triage_approvals(ctx, &approvals).await;
    for e in &entries {
        match &e.kind {
            EntryKind::ApprovalResolved { id, .. } => {
                crate::inbox::resolve_ref(&ctx.st, &id.to_string()).await;
            }
            EntryKind::RateLimit(rl) => crate::inbox::note_rate_limit(&ctx.st, rl).await,
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
                match ctx.live.run.eof().await {
                    Ok(_) => s.eof_sent = true,
                    Err(e) => tracing::warn!(target: "blazar::run", "发 EOF 失败: {e}"),
                }
            }
        }
        let (st, node, hook) = (ctx.st.clone(), ctx.node.clone(), ctx.hook.clone());
        tokio::spawn(async move {
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

        match ctx.live.run.probe().await {
            Ok(RunState::Alive { .. }) => {
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
            }
            Ok(RunState::Exited { code, .. }) => {
                if let Ok(rest) = ctx.live.run.drain(offset).await {
                    for chunk in rest.split_inclusive('\n') {
                        if !chunk.ends_with('\n') {
                            break;
                        }
                        let line = &chunk[..chunk.len() - 1];
                        let at = offset;
                        if matches!(process_line(&ctx, line, at, &mut p).await, Line::Partial) {
                            break;
                        }
                        offset = at + line.len() as u64 + 1;
                    }
                }
                finalize(&ctx, Some(code), &p).await;
                return;
            }
            Ok(RunState::Lost { .. } | RunState::Missing) => {
                finalize(&ctx, None, &p).await;
                return;
            }
            Err(e) => {
                tracing::info!(target: "blazar::run", "{} 暂时连不上: {e}", ctx.node);
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
            }
        }
    }
}

async fn finalize(ctx: &Ctx, code: Option<i32>, p: &Progress) {
    let interrupted = ctx.live.state.lock().await.interrupt_requested;

    if !p.saw_finished {
        let outcome = if interrupted {
            Outcome::Interrupted
        } else {
            let tail = ctx.live.run.stderr_tail().await.unwrap_or_default();
            let tail = tail.trim();
            Outcome::Failed {
                message: match code {
                    Some(c) => format!(
                        "agent 异常退出（退出码 {c}）{}",
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!("。stderr: {tail}")
                        }
                    ),
                    None => "agent 进程已经不在了（机器重启、被整组杀掉或目录被删），\
                             可以续接上一段对话重来"
                        .to_owned(),
                },
            }
        };
        let e = NormalizedEntry {
            seq: ctx.live.take_seq().await,
            ts: Utc::now(),
            parent_tool_use_id: None,
            kind: EntryKind::Finished(outcome),
        };
        let _ = ctx.st.db.append_event(ctx.sid, ctx.ws, &e).await;
        ctx.st.emit(ServerEvent::Entry {
            workspace_id: ctx.ws,
            session_id: ctx.sid,
            entry: Box::new(e),
        });
    }
    let _ = ctx.st.db.abort_open_approvals(ctx.sid).await;
    crate::inbox::resolve_session(&ctx.st, ctx.sid).await;

    tokio::spawn(crate::titles::maybe_title(ctx.st.clone(), ctx.sid, ctx.ws));
    let status = if interrupted {
        "interrupted"
    } else if code == Some(0) {
        "done"
    } else {
        "failed"
    };
    let _ = sqlx::query("UPDATE sessions SET status = ?1, exit_code = ?2 WHERE id = ?3")
        .bind(status)
        .bind(code)
        .bind(ctx.sid.to_string())
        .execute(ctx.st.db.pool())
        .await;
    ctx.st.running.write().await.remove(&ctx.sid);

    tokio::spawn(crate::tasks::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));
    tokio::spawn(crate::inbox::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));
    tokio::spawn(crate::scripts::on_run_finished(
        ctx.st.clone(),
        ctx.ws,
        status,
    ));
    tokio::spawn(crate::autopilot::on_run_finished(
        ctx.st.clone(),
        ctx.sid,
        status,
    ));

    tokio::spawn(crate::chat::on_run_finished(
        ctx.st.clone(),
        ctx.ws,
        ctx.sid,
        status,
    ));

    if code.is_some() {
        let _ = ctx.live.run.remove().await;
    }
    ctx.st.emit(ServerEvent::WorkspacesChanged);
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
        let live = Arc::new(Live {
            interactive: runtime.interactive(),
            run,
            state: Mutex::new(LiveState {
                next_seq,
                pending_user: u32::try_from((users - slash - consumed).max(0)).unwrap_or(0),
                user_no: u32::try_from(users).unwrap_or(0),
                eof_sent: false,
                interrupt_requested: false,
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
            if s.pending_user == 0 && !s.eof_sent && live.run.eof().await.is_ok() {
                s.eof_sent = true;
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
