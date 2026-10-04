use super::*;
use blazar_transport::{
    ExecOutput, ExecSpec, LineStream, NodeTransport, TransportError, TransportKind,
};
use std::sync::atomic::{AtomicBool, Ordering};

const FINISHED: &str = r#"{"type":"result","subtype":"success","result":"done"}"#;

struct FakeTransport {
    log: String,
    removed: AtomicBool,
    drain_failed: AtomicBool,
    drain_nonzero: AtomicBool,
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
            let stdout = if script.contains("echo \"size=") {
                format!("size={}\nexited=0\n", self.log.len())
            } else if script.contains("tail -c +") {
                if self.drain_failed.load(Ordering::SeqCst) {
                    return Err(TransportError::Command {
                        code: 1,
                        stderr: "drain failed".into(),
                    });
                }
                let start: usize = script
                    .split("tail -c +")
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                self.log[start - 1..].to_owned()
            } else if script.contains("rm -rf") {
                self.removed.store(true, Ordering::SeqCst);
                String::new()
            } else {
                assert!(
                    script.contains("err.txt"),
                    "unexpected fake command: {script}"
                );
                String::new()
            };
            Ok(ExecOutput {
                code: i32::from(
                    script.contains("tail -c +") && self.drain_nonzero.load(Ordering::SeqCst),
                ),
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
                stderr: "follow unavailable".into(),
            })
        })
    }
}

async fn fixture(log: String) -> (tempfile::TempDir, Ctx, Arc<FakeTransport>) {
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
    let sid = SessionId(uuid::Uuid::now_v7());
    let ws = WorkspaceId(uuid::Uuid::now_v7());
    sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
        .execute(st.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, 'node', 'repo', '/home/me/repo', '0')")
        .bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, run_dir, created_at) VALUES (?1, ?2, 'claude', ?3, '0')")
        .bind(sid.to_string()).bind(ws.to_string()).bind(dir.path().join("run").display().to_string())
        .execute(st.db.pool()).await.unwrap();
    let transport = Arc::new(FakeTransport {
        log,
        removed: AtomicBool::new(false),
        drain_failed: AtomicBool::new(false),
        drain_nonzero: AtomicBool::new(false),
    });
    let live = Arc::new(Live {
        run: DetachedRun::attach(
            transport.clone(),
            dir.path().join("run").display().to_string(),
            sid.to_string(),
        ),
        interactive: false,
        state: Mutex::new(LiveState {
            next_seq: 1,
            pending_user: 1,
            user_no: 1,
            eof_sent: false,
            interrupt_requested: false,
            bg: Default::default(),
            idle: false,
        }),
    });
    let ctx = Ctx {
        st,
        sid,
        ws,
        node: "local".into(),
        live,
        runtime: Arc::new(CliRuntime::by_id(transport.clone(), "claude").unwrap()),
        hook: blazar_hooks::HookContext {
            workspace: ws.to_string(),
            node: "local".into(),
            cwd: "/home/me/repo".into(),
            session_id: sid.to_string(),
            tool_name: None,
            extra: Default::default(),
        },
    };
    (dir, ctx, transport)
}

fn progress() -> Progress {
    Progress {
        started: None,
        saw_finished: false,
        partial_at: None,
    }
}

async fn reject_events(ctx: &Ctx) {
    sqlx::query("CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(ctx.st.db.pool()).await.unwrap();
}

async fn session_state(ctx: &Ctx) -> (String, i64) {
    sqlx::query_as("SELECT status, out_offset FROM sessions WHERE id = ?1")
        .bind(ctx.sid.to_string())
        .fetch_one(ctx.st.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn rejected_line_does_not_advance_live_state_and_retries_without_a_gap() {
    let (_dir, ctx, _) = fixture(String::new()).await;
    reject_events(&ctx).await;
    let mut p = progress();
    assert!(matches!(
        process_line(&ctx, FINISHED, 0, &mut p).await,
        Line::Partial
    ));
    assert!(!p.saw_finished);
    assert_eq!(ctx.live.state.lock().await.next_seq, 1);
    assert_eq!(session_state(&ctx).await.1, 0);
    sqlx::query("DROP TRIGGER reject_event")
        .execute(ctx.st.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        process_line(&ctx, FINISHED, 0, &mut p).await,
        Line::Done
    ));
    assert!(p.saw_finished);
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 1);
    assert_eq!(session_state(&ctx).await.1, (FINISHED.len() + 1) as i64);
}

#[tokio::test]
async fn exited_run_keeps_its_log_until_the_last_line_is_persisted() {
    let (_dir, ctx, transport) = fixture(format!("{FINISHED}\n")).await;
    reject_events(&ctx).await;
    let mut task = tokio::spawn(supervise(ctx.clone(), 0, None));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut task)
            .await
            .is_err()
    );
    assert!(!transport.removed.load(Ordering::SeqCst));
    assert_eq!(session_state(&ctx).await, ("running".into(), 0));
    sqlx::query("DROP TRIGGER reject_event")
        .execute(ctx.st.db.pool())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert!(transport.removed.load(Ordering::SeqCst));
    assert_eq!(
        session_state(&ctx).await,
        ("done".into(), (FINISHED.len() + 1) as i64)
    );
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 1);
}

#[tokio::test]
async fn exited_run_retries_failed_drain_before_cleanup() {
    let (_dir, ctx, transport) = fixture(format!("{FINISHED}\n")).await;
    transport.drain_failed.store(true, Ordering::SeqCst);
    let mut task = tokio::spawn(supervise(ctx.clone(), 0, None));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut task)
            .await
            .is_err()
    );
    assert!(!transport.removed.load(Ordering::SeqCst));
    assert_eq!(session_state(&ctx).await, ("running".into(), 0));
    transport.drain_failed.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert!(transport.removed.load(Ordering::SeqCst));
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 1);
}

#[tokio::test]
async fn failed_finish_write_keeps_the_log_and_running_status() {
    let (_dir, ctx, transport) = fixture(String::new()).await;
    reject_events(&ctx).await;
    let mut p = progress();
    let _ = finalize(&ctx, Some(0), &mut p).await;
    assert!(!transport.removed.load(Ordering::SeqCst));
    assert_eq!(session_state(&ctx).await.0, "running");
    assert_eq!(ctx.live.state.lock().await.next_seq, 1);
}

#[tokio::test]
async fn failed_status_write_keeps_the_log_and_retry_does_not_duplicate_finish() {
    let (_dir, ctx, transport) = fixture(String::new()).await;
    sqlx::query("CREATE TRIGGER reject_status BEFORE UPDATE OF status ON sessions BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(ctx.st.db.pool()).await.unwrap();
    let mut p = progress();
    let _ = finalize(&ctx, Some(0), &mut p).await;
    assert!(!transport.removed.load(Ordering::SeqCst));
    assert_eq!(session_state(&ctx).await.0, "running");
    sqlx::query("DROP TRIGGER reject_status")
        .execute(ctx.st.db.pool())
        .await
        .unwrap();
    let _ = finalize(&ctx, Some(0), &mut p).await;
    assert!(transport.removed.load(Ordering::SeqCst));
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 1);
}

#[test]
fn only_unfinished_background_tasks_keep_the_cli_alive() {
    for st in ["started", "running", "pending"] {
        assert!(bg_alive(st), "{st}");
    }
    for st in ["completed", "killed", "stopped", "failed"] {
        assert!(!bg_alive(st), "{st}");
    }
}

#[tokio::test]
async fn nonzero_drain_preserves_log_and_offset_even_with_valid_stdout() {
    let (_dir, ctx, transport) = fixture(format!("{FINISHED}\n")).await;
    transport.drain_nonzero.store(true, Ordering::SeqCst);
    let mut p = progress();
    let mut offset = 0;
    assert!(!drain_remaining(&ctx, &mut offset, transport.log.len() as u64, &mut p).await);
    assert_eq!(offset, 0);
    assert_eq!(session_state(&ctx).await.1, 0);
    assert!(!transport.removed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn incomplete_final_line_is_not_discarded() {
    let (_dir, ctx, transport) = fixture(FINISHED.to_owned()).await;
    let mut p = progress();
    let mut offset = 0;
    assert!(!drain_remaining(&ctx, &mut offset, transport.log.len() as u64, &mut p).await);
    assert_eq!(offset, 0);
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 0);
    assert!(!transport.removed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn failed_offset_commit_rolls_back_receipt_and_background_state() {
    let (_dir, ctx, _) = fixture(String::new()).await;
    sqlx::query("CREATE TRIGGER reject_offset BEFORE UPDATE OF out_offset ON sessions BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(ctx.st.db.pool()).await.unwrap();
    let mut p = progress();
    for line in [
        r#"{"type":"user","message":{"role":"user","content":"hello"},"isReplay":true}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"background-task","status":"started"}"#,
    ] {
        assert!(matches!(
            process_line(&ctx, line, 0, &mut p).await,
            Line::Partial
        ));
    }
    let state = ctx.live.state.lock().await;
    assert_eq!(state.pending_user, 1);
    assert!(state.bg.is_empty());
    assert_eq!(state.next_seq, 1);
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 0);
    assert_eq!(session_state(&ctx).await.1, 0);
}

#[tokio::test]
async fn fast_completion_cannot_leave_a_stale_registered_run() {
    let (_dir, ctx, transport) = fixture(format!("{FINISHED}\n")).await;
    register(ctx.clone(), 0, None).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !transport.removed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(session_state(&ctx).await.0, "done");
    assert!(!ctx.st.running.read().await.contains_key(&ctx.sid));
}
