use super::*;

const RESPONSE_KEY: &str = "_blazar_response";

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ApprovalResponse {
    pub message: String,
    pub answers: Option<serde_json::Value>,
    pub auto_rule: Option<String>,
}

#[derive(serde::Deserialize)]
struct RecordedResponse {
    version: u8,
    response: ApprovalResponse,
}

pub(super) fn strip_response(request: &mut serde_json::Value) {
    if let Some(object) = request.as_object_mut() {
        object.remove(RESPONSE_KEY);
    }
}

pub(super) fn take_response(request: &mut serde_json::Value) -> Option<ApprovalResponse> {
    request
        .as_object_mut()
        .and_then(|object| object.remove(RESPONSE_KEY))
        .and_then(|value| serde_json::from_value::<RecordedResponse>(value).ok())
        .filter(|recorded| recorded.version == 1)
        .map(|recorded| recorded.response)
}

impl Db {
    pub async fn decide_approval_with_response(
        &self,
        id: ApprovalId,
        decision: &str,
        decided_by: Option<&str>,
        response: &ApprovalResponse,
    ) -> Result<Option<PendingApproval>> {
        let mut tx = self.begin_write().await?;
        let Some(mut approval) = fetch_approval(&mut tx, id).await? else {
            return Ok(None);
        };
        if approval.decision.is_some() {
            return Ok(None);
        }
        let object = approval
            .request
            .as_object_mut()
            .ok_or_else(|| Error::InvalidApproval("请求必须是 JSON 对象".into()))?;
        object.insert(
            RESPONSE_KEY.into(),
            serde_json::json!({"version":1,"response":response}),
        );
        sqlx::query(
            "UPDATE approvals SET decision = ?1, decided_at = ?2, decided_by = ?3, request = ?5
             WHERE id = ?4 AND decision IS NULL",
        )
        .bind(decision)
        .bind(Utc::now().to_rfc3339())
        .bind(decided_by)
        .bind(id.to_string())
        .bind(serde_json::to_string(&approval.request)?)
        .execute(&mut *tx)
        .await?;
        let approval = fetch_approval(&mut tx, id).await?;
        tx.commit().await?;
        Ok(approval)
    }

    pub async fn approval(&self, id: ApprovalId) -> Result<Option<PendingApproval>> {
        let mut tx = self.pool.begin().await?;
        fetch_approval(&mut tx, id).await
    }

    pub async fn acknowledge_approval(
        &self,
        id: ApprovalId,
        session: SessionId,
        workspace: WorkspaceId,
        event: &NormalizedEntry,
    ) -> Result<bool> {
        let mut tx = self.begin_write().await?;
        let updated = sqlx::query(
            "UPDATE approvals SET delivered_at = ?1 WHERE id = ?2 AND session_id = ?3
             AND decision IN ('allow','deny') AND delivered_at IS NULL",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .bind(session.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Ok(false);
        }
        write_event(&mut tx, session, workspace, event).await?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture(db: &Db) -> (WorkspaceId, SessionId, ApprovalId) {
        let ws = WorkspaceId::new();
        let sid = SessionId::new();
        let aid = ApprovalId::from_provider("request");
        sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, 'node', 'repo', '/home/me/repo', '0')")
            .bind(ws.to_string()).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, created_at) VALUES (?1, ?2, 'claude', '0')")
            .bind(sid.to_string()).bind(ws.to_string()).execute(db.pool()).await.unwrap();
        db.record_line(sid, ws, &[], &[NewApproval {
            id: aid, provider_request_id: "request".into(), src_offset: 0,
            request: serde_json::json!({"tool_name":"AskUserQuestion","input":{"questions":[]},"provider_extension":"kept"}),
        }], 1).await.unwrap();
        (ws, sid, aid)
    }

    #[tokio::test]
    async fn saved_response_survives_reopen_without_leaking_through_public_dto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub.db");
        let db = Db::open(&path).await.unwrap();
        let (_ws, sid, aid) = fixture(&db).await;
        let response = ApprovalResponse {
            message: "private-response-reason".into(),
            answers: Some(serde_json::json!({"选择":"选项甲-private-answer"})),
            auto_rule: None,
        };
        db.decide_approval_with_response(aid, "allow", None, &response)
            .await
            .unwrap()
            .unwrap();
        db.pool().close().await;
        let db = Db::open(&path).await.unwrap();
        let saved = db.undelivered_approvals(sid).await.unwrap().remove(0);
        assert_eq!(saved.response, Some(response.clone()));
        assert_eq!(saved.request["provider_extension"], "kept");
        assert!(saved.request.get(RESPONSE_KEY).is_none());
        let public = serde_json::to_string(&saved).unwrap();
        for hidden in [RESPONSE_KEY, "private-answer", "private-response-reason"] {
            assert!(!public.contains(hidden));
        }
        assert!(
            db.decide_approval_with_response(aid, "deny", None, &ApprovalResponse::default())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            db.undelivered_approvals(sid).await.unwrap()[0].response,
            Some(response)
        );
    }

    #[tokio::test]
    async fn ended_session_undelivered_approval_does_not_block_new_session_activity() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, old, aid) = fixture(&db).await;
        db.decide_approval_with_response(aid, "allow", None, &ApprovalResponse::default())
            .await
            .unwrap();
        let event = NormalizedEntry {
            seq: 1,
            ts: Utc::now(),
            parent_tool_use_id: None,
            kind: EntryKind::AssistantMessage {
                text: "working".into(),
            },
        };
        db.append_event(old, ws, &event).await.unwrap();
        assert_eq!(
            db.workspace_activity(ws).await.unwrap(),
            ActivityState::AwaitingApproval
        );
        sqlx::query("UPDATE sessions SET status = 'done' WHERE id = ?1")
            .bind(old.to_string())
            .execute(db.pool())
            .await
            .unwrap();
        let current = SessionId::new();
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, created_at) VALUES (?1, ?2, 'claude', '1')")
            .bind(current.to_string()).bind(ws.to_string()).execute(db.pool()).await.unwrap();
        db.append_event(current, ws, &event).await.unwrap();
        assert!(db.pending_approvals(Some(ws)).await.unwrap().is_empty());
        assert_eq!(
            db.workspace_activity(ws).await.unwrap(),
            ActivityState::Running
        );
        assert_eq!(db.undelivered_approvals(old).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn decision_and_response_roll_back_together() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, _sid, aid) = fixture(&db).await;
        sqlx::query("CREATE TRIGGER reject_response BEFORE UPDATE OF request ON approvals BEGIN SELECT RAISE(FAIL, 'disk full'); END")
            .execute(db.pool()).await.unwrap();
        assert!(
            db.decide_approval_with_response(aid, "allow", None, &ApprovalResponse::default())
                .await
                .is_err()
        );
        let saved = db.pending_approvals(Some(ws)).await.unwrap().remove(0);
        assert!(saved.decision.is_none());
        assert!(saved.response.is_none());
    }

    #[test]
    fn legacy_and_invalid_response_metadata_stay_readable_without_guessing() {
        let mut legacy = serde_json::json!({"input":{"questions":[]}});
        assert!(take_response(&mut legacy).is_none());
        let mut invalid = serde_json::json!({"input":{"questions":[]},"_blazar_response":{"unknown":"provider-field"}});
        assert!(take_response(&mut invalid).is_none());
        assert_eq!(invalid, legacy);
    }
}
