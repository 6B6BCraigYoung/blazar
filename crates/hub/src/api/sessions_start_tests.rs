use super::*;
use blazar_transport::{
    ExecOutput, ExecSpec, LineStream, NodeTransport, TransportError, TransportKind,
};
use std::sync::atomic::{AtomicBool, Ordering};

struct FakeTransport {
    root: std::path::PathBuf,
    launched: AtomicBool,
    killed: AtomicBool,
    fail_launch: AtomicBool,
    fail_kill: AtomicBool,
    offline: AtomicBool,
    block_launch: AtomicBool,
    launch_entered: tokio::sync::Notify,
    release_launch: tokio::sync::Notify,
}

impl NodeTransport for FakeTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Local
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
            let script = spec.args.last().unwrap();
            let (code, stdout) = if script == "deny-start" {
                (2, "test hook denied startup".into())
            } else if script.contains("__BLAZAR_LAUNCHED__") {
                self.launched.store(true, Ordering::SeqCst);
                if self.block_launch.load(Ordering::SeqCst) {
                    self.launch_entered.notify_one();
                    self.release_launch.notified().await;
                }
                if self.fail_launch.load(Ordering::SeqCst) {
                    (98, "test launch failed".into())
                } else {
                    (
                        0,
                        format!("__BLAZAR_LAUNCHED__ {}/run\n", self.root.display()),
                    )
                }
            } else if script.contains("kill -TERM") {
                self.killed.store(true, Ordering::SeqCst);
                if self.fail_kill.load(Ordering::SeqCst) {
                    return Err(TransportError::Command {
                        code: 1,
                        stderr: "fake stop unavailable".into(),
                    });
                }
                (0, String::new())
            } else if script.contains("printf") && script.contains(".blazar/runs") {
                (0, format!("__BLAZAR_RUNS_ROOT__ {}\n", self.root.display()))
            } else if script.contains("echo \"size=") {
                if self.offline.load(Ordering::SeqCst) {
                    return Err(TransportError::Command {
                        code: 1,
                        stderr: "fake node unavailable".into(),
                    });
                }
                (0, "size=0\nexited=98\n".into())
            } else if script.contains("err.txt") || script.contains("rm -rf") {
                (0, String::new())
            } else {
                panic!("unexpected fake command: {script}")
            };
            Ok(ExecOutput {
                code,
                stdout,
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
        Box::pin(async {
            Err(TransportError::Command {
                code: 1,
                stderr: "fake follow unavailable".into(),
            })
        })
    }
}

async fn fixture() -> (tempfile::TempDir, Shared, WorkspaceId, Arc<FakeTransport>) {
    let dir = tempfile::tempdir().unwrap();
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
    sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
        .execute(st.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, 'node', 'repo', ?2, '0')")
        .bind(ws.to_string()).bind(dir.path().display().to_string()).execute(st.db.pool()).await.unwrap();
    let fake = Arc::new(FakeTransport {
        root: dir.path().join("runs"),
        launched: AtomicBool::new(false),
        killed: AtomicBool::new(false),
        fail_launch: AtomicBool::new(false),
        fail_kill: AtomicBool::new(false),
        offline: AtomicBool::new(false),
        block_launch: AtomicBool::new(false),
        launch_entered: tokio::sync::Notify::new(),
        release_launch: tokio::sync::Notify::new(),
    });
    (dir, st, ws, fake)
}

async fn prompt_fake(
    st: Shared,
    ws: WorkspaceId,
    fake: Arc<FakeTransport>,
    agent: &str,
) -> ApiResult<Json<serde_json::Value>> {
    let req = serde_json::from_value(serde_json::json!({"text":"test", "agent":agent})).unwrap();
    prompt_using(st, ws.to_string(), req, move |_| fake.clone()).await
}

async fn running_rows(st: &Shared) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE status = 'running'")
        .fetch_one(st.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn unknown_runtime_does_not_leave_a_running_session() {
    let (_dir, st, ws, fake) = fixture().await;
    assert!(
        prompt_fake(st.clone(), ws, fake.clone(), "missing-runtime")
            .await
            .is_err()
    );
    assert_eq!(running_rows(&st).await, 0);
    assert!(!fake.launched.load(Ordering::SeqCst));
}

#[tokio::test]
async fn blocking_hook_finishes_the_session_without_starting_an_agent() {
    let (_dir, st, ws, fake) = fixture().await;
    sqlx::query("INSERT INTO hooks (id, event, command, target, blocking, created_at) VALUES ('hook', 'turn_start', 'deny-start', 'node', 1, '0')")
        .execute(st.db.pool()).await.unwrap();
    let result = prompt_fake(st.clone(), ws, fake.clone(), "claude")
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    assert!(result.0["blocked_by_hook"].is_string());
    assert_eq!(running_rows(&st).await, 0);
    assert!(!fake.launched.load(Ordering::SeqCst));
    let status: String = sqlx::query_scalar("SELECT status FROM sessions")
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert_eq!(status, "failed");
}

#[tokio::test]
async fn failed_launch_is_compensated_before_releasing_admission() {
    let (_dir, st, ws, fake) = fixture().await;
    fake.fail_launch.store(true, Ordering::SeqCst);
    assert!(
        prompt_fake(st.clone(), ws, fake.clone(), "claude")
            .await
            .is_err()
    );
    assert!(fake.launched.load(Ordering::SeqCst));
    assert_eq!(running_rows(&st).await, 0);
    assert!(st.admit(ws).await.is_ok());
}

#[tokio::test]
async fn failed_run_metadata_write_never_launches_a_process() {
    let (_dir, st, ws, fake) = fixture().await;
    sqlx::query("CREATE TRIGGER reject_run_dir BEFORE UPDATE OF run_dir ON sessions BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(st.db.pool()).await.unwrap();
    assert!(
        prompt_fake(st.clone(), ws, fake.clone(), "claude")
            .await
            .is_err()
    );
    assert!(!fake.launched.load(Ordering::SeqCst));
}

#[tokio::test]
async fn failed_initial_event_write_never_launches_a_process() {
    let (_dir, st, ws, fake) = fixture().await;
    sqlx::query("CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(st.db.pool()).await.unwrap();
    assert!(
        prompt_fake(st.clone(), ws, fake.clone(), "claude")
            .await
            .is_err()
    );
    assert!(!fake.launched.load(Ordering::SeqCst));
}

#[tokio::test]
async fn admission_rejects_persisted_running_session_before_reattach() {
    let (_dir, st, ws, _) = fixture().await;
    sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, run_dir, created_at) VALUES (?1, ?2, 'claude', '/home/me/run', '0')")
        .bind(SessionId::new().to_string()).bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
    assert!(st.running.read().await.is_empty());
    assert!(st.admit(ws).await.is_err());
}

#[tokio::test]
async fn admission_fails_closed_when_session_lookup_fails() {
    let (_dir, st, ws, _) = fixture().await;
    st.db.pool().close().await;
    assert!(st.admit(ws).await.is_err());
}

#[tokio::test]
async fn disconnected_caller_does_not_cancel_launch_compensation() {
    let (_dir, st, ws, fake) = fixture().await;
    fake.fail_launch.store(true, Ordering::SeqCst);
    fake.block_launch.store(true, Ordering::SeqCst);
    let task = tokio::spawn(prompt_fake(st.clone(), ws, fake.clone(), "claude"));
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        fake.launch_entered.notified(),
    )
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    fake.release_launch.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while running_rows(&st).await != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(fake.killed.load(Ordering::SeqCst));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while st.admit(ws).await.is_err() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn concurrent_admission_only_reserves_one_startup() {
    let (_dir, st, ws, _) = fixture().await;
    let admitted = futures::future::join_all((0..16).map(|_| st.admit(ws))).await;
    assert_eq!(admitted.iter().filter(|result| result.is_ok()).count(), 1);
    drop(admitted);
    assert!(st.admit(ws).await.is_ok());
}

#[tokio::test]
async fn workspace_mutation_excludes_startup_and_deleted_workspaces_stay_deleted() {
    let (_dir, st, ws, _) = fixture().await;
    let guard = st.quiesce(ws).unwrap();
    assert!(st.admit(ws).await.is_err());
    assert!(st.quiesce(ws).is_err());
    sqlx::query("DELETE FROM workspaces WHERE id = ?1")
        .bind(ws.to_string())
        .execute(st.db.pool())
        .await
        .unwrap();
    drop(guard);
    assert!(st.admit(ws).await.is_err());
}

#[tokio::test]
async fn uncertain_launch_keeps_admission_closed_and_attaches_a_supervisor() {
    let (_dir, st, ws, fake) = fixture().await;
    fake.fail_launch.store(true, Ordering::SeqCst);
    fake.fail_kill.store(true, Ordering::SeqCst);
    fake.offline.store(true, Ordering::SeqCst);
    let result = prompt_fake(st.clone(), ws, fake.clone(), "claude").await;
    assert!(result.is_err());
    assert_eq!(running_rows(&st).await, 1);
    assert_eq!(st.running.read().await.len(), 1);
    assert!(st.admit(ws).await.is_err());
    let dir: String = sqlx::query_scalar("SELECT run_dir FROM sessions")
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert!(std::path::Path::new(&dir).starts_with(&fake.root));
    fake.offline.store(false, Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !st.running.read().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(running_rows(&st).await, 0);
}
