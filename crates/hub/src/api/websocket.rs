use super::*;

pub async fn ws_handler(ws: WebSocketUpgrade, State(st): State<Shared>) -> Response {
    ws.on_upgrade(move |socket| async move {
        let (mut tx, mut rx) = socket.split();
        let mut bus = st.bus.subscribe();

        let send = tokio::spawn(async move {
            loop {
                match bus.recv().await {
                    Ok(ev) => {
                        let Ok(text) = serde_json::to_string(&ev) else {
                            continue;
                        };
                        if tx.send(ws::Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }

                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS 订阅者落后 {n} 条，触发全量刷新");
                        let msg = serde_json::json!({ "kind": "workspaces_changed" });
                        if tx
                            .send(ws::Message::Text(msg.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        while let Some(Ok(msg)) = rx.next().await {
            if matches!(msg, ws::Message::Close(_)) {
                break;
            }
        }
        send.abort();
    })
}

use tokio::sync::broadcast;

pub(crate) async fn stop_sessions_of(st: &Shared, workspace_id: WorkspaceId) -> usize {
    let victims: Vec<SessionId> = {
        let running = st.running.read().await;
        running
            .iter()
            .filter(|(_, r)| r.workspace_id == workspace_id)
            .map(|(id, _)| *id)
            .collect()
    };
    let mut n = 0;
    for sid in victims {
        let live = st
            .running
            .read()
            .await
            .get(&sid)
            .and_then(|r| r.live.clone());
        if let Some(live) = live {
            live.state.lock().await.interrupt_requested = true;
            let _ = live.run.hard_kill().await;
            n += 1;
            continue;
        }
        if let Some(run) = st.running.write().await.remove(&sid) {
            if let Some(k) = &run.killer {
                k.kill();
            }
            run.task.abort();
            let _ = sqlx::query("UPDATE sessions SET status = 'interrupted' WHERE id = ?1")
                .bind(sid.to_string())
                .execute(st.db.pool())
                .await;
            n += 1;
        }
    }
    n
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
            serde_json::json!({ "interrupted": false, "reason": "会话未在运行" }),
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
        .ok_or_else(|| ApiError(anyhow::anyhow!("审批 id 不合法")))?;
    let decision = if req.allow { "allow" } else { "deny" };

    let Some(a) = st.db.decide_approval(aid, decision, None).await? else {
        return Ok(Json(serde_json::json!({
            "delivered": false, "reason": "这条审批已经被处理过了",
        })));
    };
    let message = if req.allow || !req.message.trim().is_empty() {
        req.message.clone()
    } else {
        "用户在 Blazar 界面上拒绝了这次操作".to_owned()
    };
    let answers = req.answers.map(serde_json::Value::Object);
    match crate::run::deliver_approval(&st, &a, req.allow, &message, answers.as_ref()).await {
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
        format!("{text}\n［附图 {n} 张］")
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
            return Err("模型名不合法".into());
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
            return Err("未知权限模式".into());
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
            return Err("输出风格名不合法".into());
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
            return Err("MCP 服务器名不合法".into());
        }
        out.push((
            "mcp_reconnect",
            json!({ "subtype": "mcp_reconnect", "serverName": n }),
        ));
    }
    if let Some(t) = req.mcp_toggle {
        if !valid_mcp_name(&t.name) {
            return Err("MCP 服务器名不合法".into());
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
