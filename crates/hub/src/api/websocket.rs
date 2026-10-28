use super::*;

mod resync;

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub since: Option<i64>,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(st): State<Shared>,
    Query(q): Query<WsQuery>,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        let _page = st.ui_driver.page_connected();
        let (tx, mut rx) = socket.split();
        let bus = st.bus.subscribe();
        let send = tokio::spawn(forward(st.clone(), q.since, bus, tx));

        while let Some(Ok(msg)) = rx.next().await {
            if matches!(msg, ws::Message::Close(_)) {
                break;
            }
        }
        send.abort();
    })
}

pub(crate) async fn forward<S>(
    st: Shared,
    since: Option<i64>,
    mut bus: broadcast::Receiver<crate::state::EventFrame>,
    mut tx: S,
) where
    S: futures::Sink<ws::Message> + Unpin,
{
    let mut replayed = 0;
    if let Some(since) = since {
        let frames = match crate::events::replay(&st.db, since, st.events.boot()).await {
            Ok(crate::events::Replay::Events(frames)) => {
                replayed = since;
                frames
            }
            Ok(crate::events::Replay::Gap) => {
                vec![crate::state::EventFrame::live(resync::lag_notification())]
            }
            Err(error) => {
                tracing::warn!("WS 补发查询失败，改为全量刷新: {error:#}");
                vec![crate::state::EventFrame::live(resync::lag_notification())]
            }
        };
        for frame in frames {
            replayed = frame.event_id.unwrap_or(replayed);
            if !send_frame(&mut tx, &frame).await {
                return;
            }
        }
    }
    loop {
        match bus.recv().await {
            Ok(frame) => {
                if frame.event_id.is_some_and(|id| id <= replayed) {
                    continue;
                }
                if !send_frame(&mut tx, &frame).await {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("WS 订阅者落后 {n} 条，触发全量刷新");
                for event in [resync::lag_notification(), ServerEvent::WorkspacesChanged] {
                    if !send_frame(&mut tx, &crate::state::EventFrame::live(event)).await {
                        return;
                    }
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn send_frame<S>(tx: &mut S, frame: &crate::state::EventFrame) -> bool
where
    S: futures::Sink<ws::Message> + Unpin,
{
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    tx.send(ws::Message::Text(text.into())).await.is_ok()
}

pub async fn event_head(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let id = crate::events::head(&st.db).await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

use tokio::sync::broadcast;

pub(crate) async fn stop_sessions_of(
    st: &Shared,
    workspace_id: WorkspaceId,
) -> anyhow::Result<usize> {
    stop_sessions_with_timeout(st, workspace_id, std::time::Duration::from_secs(30)).await
}

pub(crate) async fn stop_sessions_with_timeout(
    st: &Shared,
    workspace_id: WorkspaceId,
    timeout: std::time::Duration,
) -> anyhow::Result<usize> {
    tokio::time::timeout(timeout, async {
        let victims: Vec<_> = st
            .running
            .read()
            .await
            .iter()
            .filter(|(_, run)| run.workspace_id == workspace_id)
            .map(|(sid, run)| (*sid, run.live.clone(), run.killer.clone()))
            .collect();
        let persisted: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE workspace_id = ?1 AND status = 'running'",
        )
        .bind(workspace_id.to_string())
        .fetch_all(st.db.pool())
        .await?;
        for sid in persisted {
            anyhow::ensure!(
                victims.iter().any(|(id, _, _)| id.to_string() == sid),
                "会话 {sid} 尚未重新连接，无法确认已经停止；请恢复节点连接并重试，工作区记录与目录已保留"
            );
        }
        for (sid, live, killer) in &victims {
            if let Some(live) = live {
                live.state.lock().await.interrupt_requested = true;
                live.run.hard_kill().await.map_err(|error| {
                    anyhow::anyhow!("无法停止会话 {sid}: {error}；请检查节点连接或进程权限后重试，工作区记录与目录已保留")
                })?;
                anyhow::ensure!(
                    matches!(live.run.probe().await?, blazar_transport::detached::RunState::Exited { .. }),
                    "会话 {sid} 未确认退出；请恢复节点连接并重试，工作区记录与目录已保留"
                );
            } else if let Some(killer) = killer {
                killer.kill();
            } else {
                anyhow::bail!("会话 {sid} 没有可用的停止句柄；请重新连接会话后重试，工作区记录与目录已保留");
            }
        }
        while st.running.read().await.values().any(|run| run.workspace_id == workspace_id) {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE workspace_id = ?1 AND status = 'running'",
        )
        .bind(workspace_id.to_string())
        .fetch_one(st.db.pool())
        .await?;
        anyhow::ensure!(
            remaining == 0,
            "会话结束状态尚未保存；请恢复数据库与节点连接后重试，工作区记录与目录已保留"
        );
        Ok(victims.len())
    })
    .await
    .map_err(|_| anyhow::anyhow!("等待会话停止超时；请检查节点连接与进程状态后重试，工作区记录与目录已保留"))?
}

pub async fn interrupt(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = SessionId(id.parse()?);

    let live = st
        .running
        .read()
        .await
        .get(&sid)
        .and_then(|r| r.live.clone());
    if let Some(live) = live {
        crate::run::interrupt(&st, live, sid).await;
        return Ok(Json(serde_json::json!({ "interrupted": true })));
    }
    let removed = st.running.write().await.remove(&sid);
    let Some(run) = removed else {
        return Ok(Json(
            serde_json::json!({ "interrupted": false, "reason": "Session is not running" }),
        ));
    };

    if let Some(k) = &run.killer {
        k.kill();
    }
    run.task.abort();

    let entry = NormalizedEntry {
        seq: next_seq(&st, sid).await,
        ts: Utc::now(),
        parent_tool_use_id: None,
        kind: blazar_core_types::EntryKind::Finished(blazar_core_types::Outcome::Interrupted),
    };
    st.db.append_event(sid, run.workspace_id, &entry).await?;
    st.emit(ServerEvent::Entry {
        workspace_id: run.workspace_id,
        session_id: sid,
        entry: Box::new(entry),
    });
    st.emit(ServerEvent::WorkspacesChanged);

    let _ = sqlx::query("UPDATE sessions SET status = 'interrupted' WHERE id = ?1")
        .bind(sid.to_string())
        .execute(st.db.pool())
        .await;
    Ok(Json(serde_json::json!({ "interrupted": true })))
}

#[derive(Debug, Deserialize)]
pub struct ApprovalsQuery {
    pub workspace: Option<String>,
}

pub async fn list_approvals(
    State(st): State<Shared>,
    Query(q): Query<ApprovalsQuery>,
) -> ApiResult<Json<Vec<blazar_db::PendingApproval>>> {
    let ws = match q.workspace {
        Some(w) => Some(WorkspaceId(w.parse()?)),
        None => None,
    };
    Ok(Json(st.db.pending_approvals(ws).await?))
}

#[derive(Debug, Deserialize)]
pub struct DecideRequest {
    pub allow: bool,

    #[serde(default)]
    pub message: String,

    #[serde(default)]
    pub answers: Option<serde_json::Map<String, serde_json::Value>>,
}

pub async fn decide_approval(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<DecideRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let aid = crate::run::parse_approval_id(&id)
        .ok_or_else(|| ApiError(anyhow::anyhow!("Invalid approval id")))?;
    let decision = if req.allow { "allow" } else { "deny" };

    let message = if req.allow || !req.message.trim().is_empty() {
        req.message.clone()
    } else {
        "The user denied this action in Blazar".to_owned()
    };
    let answers = req.answers.map(serde_json::Value::Object);
    let response = blazar_db::ApprovalResponse {
        message,
        answers,
        auto_rule: None,
    };
    let a = match st
        .db
        .decide_approval_with_response(aid, decision, None, &response)
        .await?
    {
        Some(a) => a,
        None => {
            let Some(a) = st.db.approval(aid).await? else {
                return Ok(Json(serde_json::json!({
                    "delivered": false, "reason": "这条审批已经被处理过了",
                    "reason_en": "This approval has already been handled",
                })));
            };
            if let Some(saved) = &a.response
                && (a.decision.as_deref() != Some(decision)
                    || saved.message != response.message
                    || saved.answers != response.answers)
            {
                return Ok(Json(serde_json::json!({
                    "delivered": false,
                    "reason": "已有待送达的审批决定，请按原选择重试；要更改决定，请先中断当前会话",
                    "reason_en": "A decision for this approval is already pending delivery; retry with the same choice, or interrupt the session to change it",
                })));
            }
            if a.delivered && a.response.is_some() {
                return Ok(Json(serde_json::json!({"delivered": true})));
            }
            a
        }
    };
    match crate::run::deliver_saved_approval(&st, &a).await {
        Ok(()) => Ok(Json(serde_json::json!({ "delivered": true }))),

        Err(reason) => Ok(Json(
            serde_json::json!({ "delivered": false, "reason": reason }),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct InputRequest {
    pub text: String,
    #[serde(default)]
    pub images: Vec<blazar_runtime::ImageInput>,

    #[serde(default)]
    pub thinking: Option<bool>,

    #[serde(default)]
    pub context_file: Option<String>,
}

pub(crate) fn dsh_entry() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let p =
        std::path::Path::new(&home).join(".dsh/profiles/node_modules/@deepseek-ai/dsh/lib/bin.js");
    p.is_file().then(|| p.display().to_string())
}

pub(crate) fn is_slash_command(text: &str) -> bool {
    text.trim_start().starts_with('/')
}

pub(crate) fn with_image_note(text: &str, n: usize) -> String {
    if n == 0 {
        text.to_owned()
    } else {
        format!(
            "{text}\n[{n} {} attached]",
            if n == 1 { "image" } else { "images" }
        )
    }
}

#[derive(Debug, Deserialize)]
pub struct McpToggle {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct ControlRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub output_style: Option<String>,
    #[serde(default)]
    pub fast_mode: Option<bool>,
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub mcp_reconnect: Option<String>,
    #[serde(default)]
    pub mcp_toggle: Option<McpToggle>,
}

pub(crate) fn valid_mcp_name(n: &str) -> bool {
    !n.is_empty() && n.chars().count() <= 120 && !n.chars().any(char::is_control)
}

pub(crate) fn control_requests(
    req: ControlRequest,
) -> Result<Vec<(&'static str, serde_json::Value)>, String> {
    use serde_json::{Value, json};
    let mut out = Vec::new();
    if let Some(m) = req.model {
        let ok = m.len() <= 80
            && m.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.:[]/".contains(c));
        if !ok {
            return Err("Invalid model name".into());
        }

        let model = if m.is_empty() {
            Value::Null
        } else {
            Value::String(m)
        };
        out.push(("model", json!({ "subtype": "set_model", "model": model })));
    }
    if let Some(mode) = req.permission_mode.filter(|m| !m.is_empty()) {
        if ![
            "acceptEdits",
            "default",
            "auto",
            "plan",
            "bypassPermissions",
        ]
        .contains(&mode.as_str())
        {
            return Err("Unknown permission mode".into());
        }
        out.push((
            "permission_mode",
            json!({ "subtype": "set_permission_mode", "mode": mode }),
        ));
    }
    let mut flags = serde_json::Map::new();
    if let Some(style) = req.output_style {
        let style = style.trim();
        if !blazar_runtime::valid_style(style) {
            return Err("Invalid output style name".into());
        }
        flags.insert("outputStyle".into(), style.into());
    }
    if let Some(f) = req.fast_mode {
        flags.insert("fastMode".into(), f.into());
    }
    if req.thinking == Some(true) {
        flags.insert("alwaysThinkingEnabled".into(), true.into());
    }
    if !flags.is_empty() {
        out.push((
            "settings",
            json!({ "subtype": "apply_flag_settings", "settings": flags }),
        ));
    }
    if let Some(t) = req.thinking {
        let n = if t { Value::Null } else { json!(0) };
        out.push((
            "thinking",
            json!({ "subtype": "set_max_thinking_tokens", "max_thinking_tokens": n }),
        ));
    }
    if let Some(n) = req.mcp_reconnect {
        if !valid_mcp_name(&n) {
            return Err("Invalid MCP server name".into());
        }
        out.push((
            "mcp_reconnect",
            json!({ "subtype": "mcp_reconnect", "serverName": n }),
        ));
    }
    if let Some(t) = req.mcp_toggle {
        if !valid_mcp_name(&t.name) {
            return Err("Invalid MCP server name".into());
        }
        out.push((
            "mcp_toggle",
            json!({ "subtype": "mcp_toggle", "serverName": t.name, "enabled": t.enabled }),
        ));
    }
    Ok(out)
}

pub async fn session_control(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<ControlRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = SessionId(id.parse()?);
    let reqs = match control_requests(req) {
        Ok(r) => r,
        Err(r) => return Ok(Json(serde_json::json!({ "accepted": false, "reason": r }))),
    };
    let mut done = Vec::new();
    for (what, req) in reqs {
        if let Err(r) = crate::run::control(&st, sid, req).await {
            return Ok(Json(serde_json::json!({ "accepted": false, "reason": r })));
        }
        done.push(what);
    }
    Ok(Json(
        serde_json::json!({ "accepted": true, "applied": done }),
    ))
}

pub async fn session_input(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<InputRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = SessionId(id.parse()?);
    if let Err(e) = check_images(&req.images) {
        return Ok(Json(serde_json::json!({ "accepted": false, "reason": e })));
    }
    let located: Option<(String, String, String)> = sqlx::query_as(
        "SELECT w.id, n.name, w.path FROM sessions s JOIN workspaces w ON w.id = s.workspace_id
         JOIN nodes n ON n.id = w.node_id WHERE s.id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let snap = match &located {
        Some((_, node, path)) => crate::checkpoint::snapshot(&st, node, path).await,
        None => None,
    };
    if let Some(t) = req.thinking
        && let Ok(reqs) = control_requests(ControlRequest {
            thinking: Some(t),
            ..Default::default()
        })
    {
        for (_, r) in reqs {
            let _ = crate::run::control(&st, sid, r).await;
        }
    }
    let sent = with_editor_context(&req.text, req.context_file.as_deref());
    match crate::run::interject(&st, sid, &req.text, &sent, &req.images).await {
        Ok(seq) => {
            if let (Some(snap), Some((ws, _, _))) = (snap, located) {
                crate::checkpoint::record(&st, &ws, snap, Some(&sid.to_string()), Some(seq), "")
                    .await;
            }
            Ok(Json(serde_json::json!({ "accepted": true, "seq": seq })))
        }
        Err(reason) => Ok(Json(
            serde_json::json!({ "accepted": false, "reason": reason }),
        )),
    }
}

pub(crate) async fn next_seq(st: &AppState, sid: SessionId) -> u64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?1",
    )
    .bind(sid.to_string())
    .fetch_one(st.db.pool())
    .await
    .map(|v| v as u64)
    .unwrap_or(1)
}
