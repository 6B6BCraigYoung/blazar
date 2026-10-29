use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DenialDecision {
    pub id: String,
    pub session_id: SessionId,
    pub workspace_id: WorkspaceId,
    pub tool_use_id: String,
    pub tool_name: String,
    pub what: String,
    pub reason: Option<String>,
    pub allow: bool,
    pub decided_by: String,
    pub decided_at: String,
}

impl Db {
    pub async fn record_denial_decision(&self, d: &DenialDecision) -> Result<bool> {
        let done = sqlx::query(
            "INSERT INTO denial_decisions (id, session_id, workspace_id, tool_use_id, tool_name, what, reason, allow, decided_by, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (session_id, tool_use_id) DO NOTHING",
        )
        .bind(&d.id)
        .bind(d.session_id.to_string())
        .bind(d.workspace_id.to_string())
        .bind(&d.tool_use_id)
        .bind(&d.tool_name)
        .bind(&d.what)
        .bind(&d.reason)
        .bind(i64::from(d.allow))
        .bind(&d.decided_by)
        .bind(&d.decided_at)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    pub async fn denial_decision(
        &self,
        session_id: SessionId,
        tool_use_id: &str,
    ) -> Result<Option<DenialDecision>> {
        let row = sqlx::query(
            "SELECT id, session_id, workspace_id, tool_use_id, tool_name, what, reason, allow, decided_by, decided_at
             FROM denial_decisions WHERE session_id = ?1 AND tool_use_id = ?2",
        )
        .bind(session_id.to_string())
        .bind(tool_use_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(DenialDecision {
                id: row.try_get("id")?,
                session_id: SessionId(row.try_get::<String, _>("session_id")?.parse()?),
                workspace_id: WorkspaceId(row.try_get::<String, _>("workspace_id")?.parse()?),
                tool_use_id: row.try_get("tool_use_id")?,
                tool_name: row.try_get("tool_name")?,
                what: row.try_get("what")?,
                reason: row.try_get("reason")?,
                allow: row.try_get::<i64, _>("allow")? != 0,
                decided_by: row.try_get("decided_by")?,
                decided_at: row.try_get("decided_at")?,
            })
        })
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_denial_is_decided_once_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("t.db")).await.unwrap();
        let (sid, ws) = (SessionId::new(), WorkspaceId::new());
        sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('n', 'local', 'local', '0')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, 'n', 'w', '/w', '0')")
            .bind(ws.to_string())
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES (?1, ?2, 'claude', 'done', '0')",
        )
        .bind(sid.to_string())
        .bind(ws.to_string())
        .execute(db.pool())
        .await
        .unwrap();
        let d = DenialDecision {
            id: "d1".into(),
            session_id: sid,
            workspace_id: ws,
            tool_use_id: "t1".into(),
            tool_name: "Bash".into(),
            what: "ssh edge.invalid 'systemctl restart x'".into(),
            reason: Some("Remote Shell Writes".into()),
            allow: true,
            decided_by: "me".into(),
            decided_at: "2026-10-26T08:00:00Z".into(),
        };
        assert!(db.record_denial_decision(&d).await.unwrap());
        assert!(!db.record_denial_decision(&d).await.unwrap());
        assert_eq!(db.denial_decision(sid, "t1").await.unwrap(), Some(d));
        assert_eq!(db.denial_decision(sid, "t2").await.unwrap(), None);
    }
}
