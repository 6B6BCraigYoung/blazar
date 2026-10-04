use super::*;
use blazar_transport::{ExecOutput, ExecSpec, LineStream, NodeTransport, TransportKind};
use std::sync::atomic::{AtomicBool, Ordering};

struct FakeRun {
    fail: bool,
    killed: tokio::sync::Notify,
    observed_gate: AtomicBool,
    terminated: AtomicBool,
}

impl NodeTransport for FakeRun {
    fn kind(&self) -> TransportKind {
        TransportKind::Ssh
    }

    fn target(&self) -> &str {
        "local"
    }

    fn exec<'life0, 'async_trait>(
        &'life0 self,
        spec: ExecSpec,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = blazar_transport::Result<ExecOutput>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let killing = spec.args.iter().any(|a| a.contains("kill -TERM"));
            if killing && self.fail {
                return Err(blazar_transport::TransportError::Command {
                    code: 19,
                    stderr: "fixture stop refused".into(),
                });
            }
            if killing {
                self.terminated.store(true, Ordering::SeqCst);
                self.killed.notify_one();
            }
            Ok(ExecOutput {
                code: 0,
                stdout: if spec.args.iter().any(|arg| arg.contains("TABLE=$(ps")) {
                    if self.terminated.load(Ordering::SeqCst) {
                        "stopped"
                    } else {
                        "alive=12345"
                    }
                } else {
                    "size=0\nexited=137\n"
                }
                .into(),
                stderr: String::new(),
            })
        })
    }

    fn spawn_lines<'life0, 'async_trait>(
        &'life0 self,
        _spec: ExecSpec,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = blazar_transport::Result<LineStream>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { panic!("shutdown must not start a process") })
    }
}

struct Fixture {
    st: Shared,
    ws: WorkspaceId,
    sid: SessionId,
    root: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
            crate::services::Services::Isolated,
        );
        let ws = WorkspaceId::new();
        let sid = SessionId::new();
        sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
            .execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, 'node', 'fixture', ?2, '0')")
            .bind(ws.to_string()).bind(root.path().display().to_string())
            .execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, created_at) VALUES (?1, ?2, 'codex', '0')")
            .bind(sid.to_string()).bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
        std::fs::write(root.path().join("keep.txt"), "fixture").unwrap();
        Self { st, ws, sid, root }
    }

    async fn attach(&self, fail: bool, finish: bool) -> Arc<FakeRun> {
        let fake = Arc::new(FakeRun {
            fail,
            killed: tokio::sync::Notify::new(),
            observed_gate: AtomicBool::new(false),
            terminated: AtomicBool::new(false),
        });
        let live = Arc::new(crate::run::Live {
            run: blazar_transport::detached::DetachedRun::attach(
                fake.clone(),
                self.root.path().display().to_string(),
                self.sid.to_string(),
            ),
            interactive: false,
            state: tokio::sync::Mutex::new(crate::run::LiveState {
                next_seq: 1,
                pending_user: 0,
                user_no: 0,
                eof_sent: false,
                interrupt_requested: false,
                bg: Default::default(),
                idle: false,
            }),
        });
        let (st, ws, sid, transport) = (self.st.clone(), self.ws, self.sid, fake.clone());
        let task = tokio::spawn(async move {
            transport.killed.notified().await;
            if !finish {
                std::future::pending::<()>().await;
            }
            transport
                .observed_gate
                .store(st.quiesce(ws).is_err(), Ordering::SeqCst);
            sqlx::query("UPDATE sessions SET status = 'interrupted' WHERE id = ?1")
                .bind(sid.to_string())
                .execute(st.db.pool())
                .await
                .unwrap();
            st.running.write().await.remove(&sid);
        });
        self.st.running.write().await.insert(
            self.sid,
            crate::state::RunningSession {
                workspace_id: self.ws,
                task,
                killer: None,
                live: Some(live),
            },
        );
        fake
    }

    async fn assert_preserved(&self) {
        let workspaces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE id = ?1")
            .bind(self.ws.to_string())
            .fetch_one(self.st.db.pool())
            .await
            .unwrap();
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id = ?1")
            .bind(self.sid.to_string())
            .fetch_one(self.st.db.pool())
            .await
            .unwrap();
        assert_eq!((workspaces, sessions), (1, 1));
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("keep.txt")).unwrap(),
            "fixture"
        );
    }

    async fn abort_fake(&self) {
        for (_, run) in self.st.running.write().await.drain() {
            run.task.abort();
        }
    }
}

#[tokio::test]
async fn workspace_shutdown_delete_keeps_unattached_running_records() {
    let f = Fixture::new().await;
    let result = delete_workspace(State(f.st.clone()), Path(f.ws.to_string())).await;
    assert!(
        result.is_err(),
        "unconfirmed persisted runs must block cascading deletion"
    );
    f.assert_preserved().await;
}

#[tokio::test]
async fn workspace_shutdown_delete_keeps_records_when_kill_fails() {
    let f = Fixture::new().await;
    f.attach(true, false).await;
    let result = delete_workspace(State(f.st.clone()), Path(f.ws.to_string())).await;
    f.abort_fake().await;
    assert!(result.is_err(), "failed stop must block cascading deletion");
    f.assert_preserved().await;
}

#[tokio::test]
async fn workspace_shutdown_destroy_keeps_directory_and_records_when_kill_fails() {
    let f = Fixture::new().await;
    f.attach(true, false).await;
    let result = destroy_workspace(State(f.st.clone()), Path(f.ws.to_string())).await;
    f.abort_fake().await;
    assert!(result.is_err(), "failed stop must block destruction");
    f.assert_preserved().await;
}

#[tokio::test]
async fn workspace_shutdown_pause_reports_stop_failure_before_worktree_operations() {
    let f = Fixture::new().await;
    f.attach(true, false).await;
    let result = pause_workspace(State(f.st.clone()), Path(f.ws.to_string())).await;
    f.abort_fake().await;
    assert!(
        result
            .err()
            .unwrap()
            .message()
            .contains("fixture stop refused")
    );
    f.assert_preserved().await;
}

#[tokio::test]
async fn workspace_shutdown_destroy_retains_invalid_isolated_metadata() {
    let f = Fixture::new().await;
    sqlx::query("UPDATE sessions SET status = 'done'")
        .execute(f.st.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE workspaces SET branch = 'blazar/fixture', base_commit = 'fixture'")
        .execute(f.st.db.pool())
        .await
        .unwrap();
    let result = destroy_workspace(State(f.st.clone()), Path(f.ws.to_string())).await;
    assert!(
        result.is_err(),
        "missing repository metadata must not be silently discarded"
    );
    f.assert_preserved().await;
}

#[tokio::test]
async fn workspace_shutdown_delete_waits_for_finish_before_cascade() {
    let f = Fixture::new().await;
    let fake = f.attach(false, true).await;
    let result = delete_workspace(State(f.st.clone()), Path(f.ws.to_string())).await;
    f.abort_fake().await;
    assert!(result.is_ok());
    assert!(
        fake.observed_gate.load(Ordering::SeqCst),
        "stop must run inside the workspace admission gate"
    );
    assert!(f.st.running.read().await.is_empty());
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(f.st.db.pool())
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn workspace_shutdown_timeout_retains_running_handles_and_records() {
    let f = Fixture::new().await;
    f.attach(false, false).await;
    let _guard = f.st.quiesce(f.ws).unwrap();
    let result = crate::api::websocket::stop_sessions_with_timeout(
        &f.st,
        f.ws,
        std::time::Duration::from_millis(10),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("超时"));
    assert!(f.st.running.read().await.contains_key(&f.sid));
    f.assert_preserved().await;
    f.abort_fake().await;
}

#[tokio::test]
async fn workspace_shutdown_cannot_race_an_existing_start_or_mutation() {
    let f = Fixture::new().await;
    let _guard = f.st.quiesce(f.ws).unwrap();
    let result = delete_workspace(State(f.st.clone()), Path(f.ws.to_string())).await;
    assert!(result.err().unwrap().message().contains("操作正在进行"));
    f.assert_preserved().await;
}
