use std::path::Path;
use std::str::FromStr;

use blazar_core_types::{
    ActivityState, ApprovalId, EntryKind, NormalizedEntry, SessionId, WorkspaceId,
};
use chrono::Utc;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("数据库错误: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("迁移失败: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("事件负载序列化失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("库里的标识符不是合法 UUID: {0}")]
    BadId(#[from] sqlx::types::uuid::Error),
}

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(10))
            .foreign_keys(true);
        Self::connect(options).await
    }

    pub async fn open_in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .expect("内存 DSN 应始终有效")
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        let migrator = sqlx::migrate!("./migrations");
        self.repair_checksums(&migrator).await?;
        migrator.run(&self.pool).await?;
        Ok(())
    }

    async fn repair_checksums(&self, migrator: &sqlx::migrate::Migrator) -> Result<()> {
        let table: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_optional(&self.pool)
        .await?;
        if table.is_none() {
            return Ok(());
        }
        for m in migrator.iter() {
            sqlx::query(
                "UPDATE _sqlx_migrations SET checksum = ?1 WHERE version = ?2 AND checksum != ?1",
            )
            .bind(m.checksum.as_ref())
            .bind(m.version)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn append_event(
        &self,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        entry: &NormalizedEntry,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        write_event(&mut tx, session_id, workspace_id, entry).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn begin_write(&self) -> Result<sqlx::Transaction<'_, sqlx::Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }

    pub async fn record_line(
        &self,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        entries: &[NormalizedEntry],
        approvals: &[NewApproval],
        new_offset: u64,
    ) -> Result<()> {
        let mut tx = self.begin_write().await?;
        let now = Utc::now().to_rfc3339();
        for a in approvals {
            let mut req = a.request.clone();

            if let serde_json::Value::Object(_) = req {
                let mut k = EntryKind::Approval {
                    id: a.id,
                    request: req,
                };
                blazar_core_types::sanitize::sanitize(&mut k);
                let EntryKind::Approval { request, .. } = k else {
                    unreachable!()
                };
                req = request;
            }
            sqlx::query(
                "INSERT INTO approvals (id, session_id, workspace_id, request, provider_request_id,
                                        src_offset, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT DO NOTHING",
            )
            .bind(a.id.to_string())
            .bind(session_id.to_string())
            .bind(workspace_id.to_string())
            .bind(serde_json::to_string(&req)?)
            .bind(&a.provider_request_id)
            .bind(i64::try_from(a.src_offset).unwrap_or(i64::MAX))
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        for e in entries {
            if let EntryKind::ApprovalResolved {
                id,
                decision: blazar_core_types::ApprovalDecision::Cancelled,
            } = &e.kind
            {
                sqlx::query(
                    "UPDATE approvals SET decision = 'cancelled', decided_at = ?1
                     WHERE id = ?2 AND decision IS NULL",
                )
                .bind(&now)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
            }
            write_event(&mut tx, session_id, workspace_id, e).await?;
        }
        sqlx::query("UPDATE sessions SET out_offset = ?1 WHERE id = ?2")
            .bind(i64::try_from(new_offset).unwrap_or(i64::MAX))
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn decide_approval(
        &self,
        id: ApprovalId,
        decision: &str,
        decided_by: Option<&str>,
    ) -> Result<Option<PendingApproval>> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query(
            "UPDATE approvals SET decision = ?1, decided_at = ?2, decided_by = ?3
             WHERE id = ?4 AND decision IS NULL",
        )
        .bind(decision)
        .bind(Utc::now().to_rfc3339())
        .bind(decided_by)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let row = if n == 1 {
            fetch_approval(&mut tx, id).await?
        } else {
            None
        };
        tx.commit().await?;
        Ok(row)
    }

    pub async fn mark_approval_delivered(&self, id: ApprovalId) -> Result<()> {
        sqlx::query("UPDATE approvals SET delivered_at = ?1 WHERE id = ?2")
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn undelivered_approvals(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<PendingApproval>> {
        let mut tx = self.pool.begin().await?;
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM approvals WHERE session_id = ?1
               AND decision IN ('allow', 'deny') AND delivered_at IS NULL",
        )
        .bind(session_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(a) = fetch_approval(&mut tx, ApprovalId(id.parse()?)).await? {
                out.push(a);
            }
        }
        Ok(out)
    }

    pub async fn pending_approvals(
        &self,
        workspace_id: Option<WorkspaceId>,
    ) -> Result<Vec<PendingApproval>> {
        let mut tx = self.pool.begin().await?;
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM approvals WHERE decision IS NULL
               AND (?1 IS NULL OR workspace_id = ?1) ORDER BY created_at",
        )
        .bind(workspace_id.map(|w| w.to_string()))
        .fetch_all(&mut *tx)
        .await?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(a) = fetch_approval(&mut tx, ApprovalId(id.parse()?)).await? {
                out.push(a);
            }
        }
        Ok(out)
    }

    pub async fn abort_open_approvals(&self, session_id: SessionId) -> Result<u64> {
        Ok(sqlx::query(
            "UPDATE approvals SET decision = 'aborted', decided_at = ?1
             WHERE session_id = ?2 AND decision IS NULL",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(session_id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    pub async fn max_seq(&self, session_id: SessionId) -> Result<u64> {
        let n: Option<i64> =
            sqlx::query_scalar("SELECT MAX(seq) FROM events WHERE session_id = ?1")
                .bind(session_id.to_string())
                .fetch_one(&self.pool)
                .await?;
        Ok(u64::try_from(n.unwrap_or(0)).unwrap_or(0))
    }

    pub async fn events_since(
        &self,
        session_id: SessionId,
        after_seq: u64,
    ) -> Result<Vec<NormalizedEntry>> {
        let rows = sqlx::query(
            "SELECT seq, ts, payload FROM events
             WHERE session_id = ?1 AND seq > ?2
             ORDER BY seq",
        )
        .bind(session_id.to_string())
        .bind(i64::try_from(after_seq).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let payload: String = row.try_get("payload")?;
                let ts: String = row.try_get("ts")?;
                let seq: i64 = row.try_get("seq")?;
                Ok(NormalizedEntry {
                    seq: u64::try_from(seq).unwrap_or(0),
                    ts: chrono::DateTime::parse_from_rfc3339(&ts)
                        .map(|t| t.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    parent_tool_use_id: None,
                    kind: serde_json::from_str::<EntryKind>(&payload)?,
                })
            })
            .collect()
    }

    pub async fn workspace_activity(&self, workspace_id: WorkspaceId) -> Result<ActivityState> {
        let row = sqlx::query("SELECT activity FROM workspaces WHERE id = ?1")
            .bind(workspace_id.to_string())
            .fetch_one(&self.pool)
            .await?;
        let value: String = row.try_get("activity")?;
        Ok(parse_activity(&value))
    }
}

fn activity_str(state: ActivityState) -> &'static str {
    match state {
        ActivityState::AwaitingApproval => "awaiting_approval",
        ActivityState::Errored => "errored",
        ActivityState::Completed => "completed",
        ActivityState::Running => "running",
        ActivityState::Idle => "idle",
    }
}

fn parse_activity(value: &str) -> ActivityState {
    match value {
        "awaiting_approval" => ActivityState::AwaitingApproval,
        "errored" => ActivityState::Errored,
        "completed" => ActivityState::Completed,
        "running" => ActivityState::Running,
        _ => ActivityState::Idle,
    }
}

#[derive(Debug, Clone)]
pub struct NewApproval {
    pub id: ApprovalId,
    pub provider_request_id: String,

    pub src_offset: u64,
    pub request: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingApproval {
    pub id: ApprovalId,
    pub session_id: SessionId,
    pub workspace_id: Option<String>,
    pub provider_request_id: Option<String>,
    pub src_offset: Option<u64>,

    pub request: serde_json::Value,
    pub decision: Option<String>,
    pub created_at: String,
}

async fn fetch_approval(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: ApprovalId,
) -> Result<Option<PendingApproval>> {
    let Some(r) = sqlx::query(
        "SELECT id, session_id, workspace_id, provider_request_id, src_offset, request,
                decision, created_at FROM approvals WHERE id = ?1",
    )
    .bind(id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let sid: String = r.try_get("session_id")?;
    Ok(Some(PendingApproval {
        id,
        session_id: SessionId(sid.parse()?),
        workspace_id: r.try_get("workspace_id")?,
        provider_request_id: r.try_get("provider_request_id")?,
        src_offset: r
            .try_get::<Option<i64>, _>("src_offset")?
            .and_then(|v| u64::try_from(v).ok()),
        request: serde_json::from_str(&r.try_get::<String, _>("request")?)?,
        decision: r.try_get("decision")?,
        created_at: r.try_get("created_at")?,
    }))
}

async fn write_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: SessionId,
    workspace_id: WorkspaceId,
    entry: &NormalizedEntry,
) -> Result<()> {
    let mut kind = entry.kind.clone();
    blazar_core_types::sanitize::sanitize(&mut kind);
    let payload = serde_json::to_string(&kind)?;
    sqlx::query(
        "INSERT INTO events (session_id, seq, ts, payload, parent_tool_id) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (session_id, seq) DO NOTHING",
    )
    .bind(session_id.to_string())
    .bind(i64::try_from(entry.seq).unwrap_or(i64::MAX))
    .bind(entry.ts.to_rfc3339())
    .bind(&payload)
    .bind(entry.parent_tool_use_id.as_ref().map(|p| p.0.clone()))
    .execute(&mut **tx)
    .await?;

    if let Some(mut activity) = ActivityState::from_entry(&kind) {
        if activity == ActivityState::Running {
            let still: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM approvals WHERE workspace_id = ?1 AND decision IS NULL",
            )
            .bind(workspace_id.to_string())
            .fetch_one(&mut **tx)
            .await?;
            if still > 0 {
                activity = ActivityState::AwaitingApproval;
            }
        }
        sqlx::query("UPDATE workspaces SET activity = ?1, last_active_at = ?2 WHERE id = ?3")
            .bind(activity_str(activity))
            .bind(Utc::now().to_rfc3339())
            .bind(workspace_id.to_string())
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use blazar_core_types::{NodeId, Outcome, ToolId};

    async fn fixture(db: &Db) -> (WorkspaceId, SessionId) {
        let now = Utc::now().to_rfc3339();
        let node = NodeId::new();
        let workspace = WorkspaceId::new();
        let session = SessionId::new();

        sqlx::query(
            "INSERT INTO nodes (id, name, transport, created_at) VALUES (?1,'local','local',?2)",
        )
        .bind(node.to_string())
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO workspaces (id, node_id, name, path, created_at)
             VALUES (?1, ?2, 'w', '/tmp/w', ?3)",
        )
        .bind(workspace.to_string())
        .bind(node.to_string())
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, created_at)
             VALUES (?1, ?2, 'claude_cli', ?3)",
        )
        .bind(session.to_string())
        .bind(workspace.to_string())
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();

        (workspace, session)
    }

    fn entry(seq: u64, kind: EntryKind) -> NormalizedEntry {
        NormalizedEntry {
            seq,
            ts: Utc::now(),
            parent_tool_use_id: None,
            kind,
        }
    }

    #[tokio::test]
    async fn migrations_apply() {
        Db::open_in_memory().await.expect("迁移应成功");
    }

    #[tokio::test]
    async fn events_replay_from_seq() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, session) = fixture(&db).await;

        for i in 1..=5 {
            let e = entry(
                i,
                EntryKind::AssistantMessage {
                    text: format!("m{i}"),
                },
            );
            db.append_event(session, ws, &e).await.unwrap();
        }

        let replay = db.events_since(session, 2).await.unwrap();
        assert_eq!(replay.len(), 3);
        assert_eq!(replay[0].seq, 3);
    }

    #[tokio::test]
    async fn seq_zero_replays_everything() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, session) = fixture(&db).await;

        for i in 1..=3 {
            db.append_event(
                session,
                ws,
                &entry(
                    i,
                    EntryKind::AssistantMessage {
                        text: format!("m{i}"),
                    },
                ),
            )
            .await
            .unwrap();
        }

        let all = db.events_since(session, 0).await.unwrap();
        assert_eq!(all.len(), 3, "after_seq=0 必须返回全部，含第一条");
        assert_eq!(all[0].seq, 1);
    }

    #[tokio::test]
    async fn append_is_idempotent_on_replay() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, session) = fixture(&db).await;
        let e = entry(
            1,
            EntryKind::AssistantMessage {
                text: "once".into(),
            },
        );

        db.append_event(session, ws, &e).await.unwrap();
        db.append_event(session, ws, &e).await.unwrap();

        assert_eq!(db.events_since(session, 0).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn approval_drives_workspace_badge() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, session) = fixture(&db).await;

        db.append_event(
            session,
            ws,
            &entry(
                1,
                EntryKind::ToolUse {
                    id: ToolId("t1".into()),
                    name: "Bash".into(),
                    input: serde_json::Value::Null,
                },
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            db.workspace_activity(ws).await.unwrap(),
            ActivityState::Running
        );

        db.append_event(
            session,
            ws,
            &entry(
                2,
                EntryKind::Approval {
                    id: blazar_core_types::ApprovalId::new(),
                    request: serde_json::json!({ "command": "rm -rf /" }),
                },
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            db.workspace_activity(ws).await.unwrap(),
            ActivityState::AwaitingApproval
        );
    }

    #[tokio::test]
    async fn token_usage_does_not_disturb_badge() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, session) = fixture(&db).await;

        db.append_event(
            session,
            ws,
            &entry(
                1,
                EntryKind::Finished(Outcome::Success {
                    text: Some("done".into()),
                    usage: None,
                    denied: Vec::new(),
                }),
            ),
        )
        .await
        .unwrap();
        db.append_event(
            session,
            ws,
            &entry(2, EntryKind::TokenUsage(Default::default())),
        )
        .await
        .unwrap();

        assert_eq!(
            db.workspace_activity(ws).await.unwrap(),
            ActivityState::Completed
        );
    }

    #[tokio::test]
    async fn concurrent_writers_on_a_real_wal_file_never_lose_events() {
        let path = std::env::temp_dir().join(format!("blazar-wal-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path).await.unwrap();
        let (ws, s1) = fixture(&db).await;
        let s2 = SessionId::new();
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, created_at) VALUES (?1, ?2, 'claude_cli', 't')")
            .bind(s2.to_string()).bind(ws.to_string()).execute(db.pool()).await.unwrap();

        let mut tasks = Vec::new();
        for sid in [s1, s2] {
            for chunk in 0..4u64 {
                let db = db.clone();
                tasks.push(tokio::spawn(async move {
                    for i in 0..50u64 {
                        let seq = chunk * 50 + i + 1;
                        db.record_line(
                            sid,
                            ws,
                            &[entry(
                                seq,
                                EntryKind::AssistantMessage {
                                    text: format!("m{seq}"),
                                },
                            )],
                            &[],
                            seq * 10,
                        )
                        .await
                        .expect("并发写不能报 database is locked");
                    }
                }));
            }
        }
        for t in tasks {
            t.await.unwrap();
        }
        for sid in [s1, s2] {
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE session_id = ?1")
                .bind(sid.to_string())
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(n, 200, "一条都不能丢");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn an_approval_is_decided_exactly_once() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, sid) = fixture(&db).await;
        let id = ApprovalId::from_provider("req-1");
        db.record_line(
            sid,
            ws,
            &[entry(1, EntryKind::Approval { id, request: serde_json::json!({"tool_name":"Bash"}) })],
            &[NewApproval {
                id,
                provider_request_id: "req-1".into(),
                src_offset: 42,
                request: serde_json::json!({"tool_name":"Bash","input":{"command":"export GITHUB_TOKEN=ghp_abcdefghijklmnopqrst"}}),
            }],
            100,
        )
        .await
        .unwrap();

        let pending = db.pending_approvals(Some(ws)).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].src_offset, Some(42));
        let shown = pending[0].request.to_string();
        assert!(
            !shown.contains("ghp_abcdefghij"),
            "审批里的输入同样要脱敏再进库: {shown}"
        );

        assert!(
            db.decide_approval(id, "allow", Some("me"))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            db.decide_approval(id, "deny", Some("别人"))
                .await
                .unwrap()
                .is_none(),
            "第二次裁决不能覆盖第一次"
        );
        assert_eq!(
            db.undelivered_approvals(sid).await.unwrap().len(),
            1,
            "裁决了还没送达"
        );
        db.mark_approval_delivered(id).await.unwrap();
        assert!(db.undelivered_approvals(sid).await.unwrap().is_empty());
        let off: i64 = sqlx::query_scalar("SELECT out_offset FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(off, 100, "偏移要和事件一起前进");
    }

    #[tokio::test]
    async fn one_resolved_approval_does_not_hide_another_still_pending() {
        let db = Db::open_in_memory().await.unwrap();
        let (ws, sid) = fixture(&db).await;
        let (a, b) = (
            ApprovalId::from_provider("a"),
            ApprovalId::from_provider("b"),
        );
        let na = |id, rid: &str| NewApproval {
            id,
            provider_request_id: rid.into(),
            src_offset: 0,
            request: serde_json::json!({}),
        };
        db.record_line(
            sid,
            ws,
            &[
                entry(
                    1,
                    EntryKind::Approval {
                        id: a,
                        request: serde_json::json!({}),
                    },
                ),
                entry(
                    2,
                    EntryKind::Approval {
                        id: b,
                        request: serde_json::json!({}),
                    },
                ),
            ],
            &[na(a, "a"), na(b, "b")],
            10,
        )
        .await
        .unwrap();
        db.decide_approval(a, "allow", None).await.unwrap();
        db.append_event(
            sid,
            ws,
            &entry(
                3,
                EntryKind::ApprovalResolved {
                    id: a,
                    decision: blazar_core_types::ApprovalDecision::Allow,
                },
            ),
        )
        .await
        .unwrap();
        let act: String = sqlx::query_scalar("SELECT activity FROM workspaces WHERE id = ?1")
            .bind(ws.to_string())
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(act, "awaiting_approval", "还挂着一条，徽标不能熄");

        db.record_line(
            sid,
            ws,
            &[entry(
                4,
                EntryKind::ApprovalResolved {
                    id: b,
                    decision: blazar_core_types::ApprovalDecision::Cancelled,
                },
            )],
            &[],
            20,
        )
        .await
        .unwrap();
        let act: String = sqlx::query_scalar("SELECT activity FROM workspaces WHERE id = ?1")
            .bind(ws.to_string())
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(act, "running");
        assert!(db.pending_approvals(Some(ws)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn edited_migrations_are_accepted_by_existing_databases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let db = Db::open(&path).await.unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1")
            .execute(db.pool())
            .await
            .unwrap();
        drop(db);
        let db = Db::open(&path).await.unwrap();
        let stale: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE checksum = X'00'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(stale, 0);
    }
}
