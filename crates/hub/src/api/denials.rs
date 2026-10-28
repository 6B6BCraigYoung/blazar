use super::*;

use blazar_core_types::{Denial, EntryKind, Outcome, denial};

#[derive(Debug, Deserialize)]
pub struct DenialRequest {
    pub tool_use_id: String,
    pub allow: bool,
}

fn decided_by() -> String {
    std::env::var("USER")
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "user".into())
}

fn find_denial(events: &[NormalizedEntry], tool_use_id: &str) -> Option<Denial> {
    events.iter().rev().find_map(|e| match &e.kind {
        EntryKind::Finished(Outcome::Success { denials, .. }) => denials
            .iter()
            .find(|d| d.tool_use_id == tool_use_id)
            .cloned(),
        _ => None,
    })
}

fn find_reason(events: &[NormalizedEntry], tool_use_id: &str) -> Option<String> {
    events.iter().find_map(|e| match &e.kind {
        EntryKind::ToolResult { id, content, .. } if id.0 == tool_use_id => {
            denial::reason_of(content)
        }
        _ => None,
    })
}

pub async fn decide_denial(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<DenialRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = SessionId(id.parse()?);
    let ws: Option<String> = sqlx::query_scalar("SELECT workspace_id FROM sessions WHERE id = ?1")
        .bind(sid.to_string())
        .fetch_optional(st.db.pool())
        .await?;
    let Some(ws) = ws else {
        return Err(ApiError(anyhow::Error::new(NotFound(
            "Session not found".into(),
        ))));
    };
    let ws = WorkspaceId(ws.parse()?);
    let events = st.db.events_since(sid, 0).await?;
    let Some(denial) = find_denial(&events, &req.tool_use_id) else {
        return Err(ApiError::bad_request(
            "This action isn't in the blocked list",
        ));
    };
    let what = denial.what();
    let text = denial::retry_message(&denial.tool_name, &what);
    if let Some(done) = st.db.denial_decision(sid, &req.tool_use_id).await? {
        return Ok(Json(serde_json::json!({
            "recorded": false, "allow": done.allow, "by": done.decided_by, "what": what, "text": text,
        })));
    }
    if st.running.read().await.contains_key(&sid) {
        return Ok(Json(serde_json::json!({
            "recorded": false, "reason": "这一轮还没结束，等它停下再决定",
            "reason_en": "This turn hasn't finished yet; decide once it stops",
        })));
    }
    let by = decided_by();
    let now = Utc::now();
    let decision = blazar_db::DenialDecision {
        id: uuid::Uuid::now_v7().to_string(),
        session_id: sid,
        workspace_id: ws,
        tool_use_id: req.tool_use_id.clone(),
        tool_name: denial.tool_name.clone(),
        what: what.clone(),
        reason: find_reason(&events, &req.tool_use_id),
        allow: req.allow,
        decided_by: by.clone(),
        decided_at: now.to_rfc3339(),
    };
    if !st.db.record_denial_decision(&decision).await? {
        return Ok(Json(serde_json::json!({
            "recorded": false, "reason": "这条操作已经决定过了",
            "reason_en": "This action has already been decided",
        })));
    }
    let entry = NormalizedEntry {
        seq: next_seq(&st, sid).await,
        ts: now,
        parent_tool_use_id: None,
        kind: EntryKind::DenialDecided {
            tool_use_id: req.tool_use_id,
            allow: req.allow,
            by,
            what: what.clone(),
        },
    };
    st.db.append_event(sid, ws, &entry).await?;
    st.emit(ServerEvent::Entry {
        workspace_id: ws,
        session_id: sid,
        entry: Box::new(entry),
    });
    crate::inbox::resolve_ref(&st, &decision.tool_use_id).await;
    Ok(Json(serde_json::json!({
        "recorded": true, "allow": req.allow, "what": what, "text": text, "reason": decision.reason,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    async fn fixture() -> (tempfile::TempDir, Shared, WorkspaceId, SessionId) {
        let dir = tempfile::tempdir().unwrap();
        blazar_db::ensure_private_dir(dir.path()).unwrap();
        let db = blazar_db::Db::open(dir.path().join("hub.db"))
            .await
            .unwrap();
        let st = AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().join("mesh")),
            crate::services::Services::Isolated,
        );
        let ws = WorkspaceId::new();
        let sid = SessionId::new();
        sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
            .execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, 'node', 'repo', '/repo', '0')")
            .bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES (?1, ?2, 'claude', 'done', '0')")
            .bind(sid.to_string()).bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        let denied = "Permission for this action was denied by the Claude Code auto mode classifier. Reason: [Remote Shell Writes]. If you have other tasks";
        for (seq, payload) in [
            (1, r#"{"type":"user_message","text":"重启 edge.invalid 上的 demo-agent"}"#.to_owned()),
            (2, r#"{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ssh edge.invalid 'systemctl restart demo-agent'"}}"#.to_owned()),
            (3, format!(r#"{{"type":"tool_result","id":"t1","ok":false,"content":"{denied}"}}"#)),
            (4, r#"{"type":"finished","status":"success","denied":["Bash: ssh edge.invalid 'systemctl restart demo-agent'"],"denials":[{"tool_name":"Bash","tool_use_id":"t1","input":{"command":"ssh edge.invalid 'systemctl restart demo-agent'"}}]}"#.to_owned()),
        ] {
            sqlx::query("INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, ?2, '2026-10-26T08:00:00Z', ?3)")
                .bind(sid.to_string()).bind(seq).bind(payload).execute(st.db.pool()).await.unwrap();
        }
        (dir, st, ws, sid)
    }

    async fn call(
        st: &Shared,
        sid: SessionId,
        tool_use_id: &str,
        allow: bool,
    ) -> serde_json::Value {
        let req = DenialRequest {
            tool_use_id: tool_use_id.into(),
            allow,
        };
        match decide_denial(State(st.clone()), Path(sid.to_string()), Json(req)).await {
            Ok(Json(v)) => v,
            Err(e) => {
                let status = e.into_response().status();
                serde_json::json!({ "status": status.as_u16() })
            }
        }
    }

    #[tokio::test]
    async fn allowing_a_denied_call_records_who_and_appends_the_decision() {
        let (_dir, st, ws, sid) = fixture().await;
        let r = call(&st, sid, "t1", true).await;
        assert_eq!(r["recorded"], true, "{r}");
        assert_eq!(r["what"], "ssh edge.invalid 'systemctl restart demo-agent'");
        assert_eq!(r["reason"], "Remote Shell Writes");
        assert_eq!(
            r["text"],
            "I approve the command that was just blocked. Please retry it once, exactly as \
            written:\n```\nssh edge.invalid 'systemctl restart demo-agent'\n```"
        );
        let saved = st.db.denial_decision(sid, "t1").await.unwrap().unwrap();
        assert_eq!(
            (saved.workspace_id, saved.allow, saved.reason.as_deref()),
            (ws, true, Some("Remote Shell Writes"))
        );
        assert!(!saved.decided_by.is_empty());
        let events = st.db.events_since(sid, 4).await.unwrap();
        assert!(
            matches!(&events[..], [NormalizedEntry { seq: 5, kind: EntryKind::DenialDecided { tool_use_id, allow: true, by, what }, .. }]
                if tool_use_id == "t1" && by == &saved.decided_by && what == "ssh edge.invalid 'systemctl restart demo-agent'"),
            "{events:?}"
        );
        let again = call(&st, sid, "t1", false).await;
        assert_eq!(again["recorded"], false);
        assert_eq!(again["allow"], true, "第二次回答沿用第一次的决定");
        assert_eq!(st.db.events_since(sid, 5).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn only_calls_the_turn_reported_as_denied_can_be_decided() {
        let (_dir, st, _ws, sid) = fixture().await;
        let r = call(&st, sid, "t9", true).await;
        assert_eq!(r["status"], 400, "{r}");
        let missing = call(&st, SessionId::new(), "t1", true).await;
        assert_eq!(missing["status"], 404, "{missing}");
        assert!(st.db.denial_decision(sid, "t9").await.unwrap().is_none());
    }
}
