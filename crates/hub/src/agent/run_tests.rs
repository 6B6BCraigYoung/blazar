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
    append_failed: AtomicBool,
    inputs: std::sync::Mutex<Vec<(String, String)>>,
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
            } else if script.contains("in.inode") && script.contains(">> in.jsonl") {
                if self.append_failed.load(Ordering::SeqCst) {
                    return Err(TransportError::Command {
                        code: 1,
                        stderr: "input unavailable".into(),
                    });
                }
                let input = String::from_utf8(spec.stdin.unwrap()).unwrap();
                let (id, payload) = input.trim_end_matches('\n').split_once('\t').unwrap();
                let mut inputs = self.inputs.lock().unwrap();
                if inputs.iter().any(|(existing, _)| existing == id) {
                    "dup".into()
                } else {
                    inputs.push((id.into(), payload.into()));
                    "ok".into()
                }
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
        append_failed: AtomicBool::new(false),
        inputs: std::sync::Mutex::new(Vec::new()),
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

async fn approval_fixture() -> (tempfile::TempDir, Ctx, Arc<FakeTransport>, ApprovalId) {
    let request = serde_json::json!({"tool_name":"AskUserQuestion","input":{"questions":[{"question":"Choice?"}]}});
    let raw =
        serde_json::json!({"type":"control_request","request_id":"request","request":request});
    let (dir, ctx, fake) = fixture(format!("{raw}\n")).await;
    let id = ApprovalId::from_provider("request");
    ctx.st
        .db
        .record_line(
            ctx.sid,
            ctx.ws,
            &[],
            &[NewApproval {
                id,
                provider_request_id: "request".into(),
                src_offset: 0,
                request,
            }],
            raw.to_string().len() as u64 + 1,
        )
        .await
        .unwrap();
    ctx.st.running.write().await.insert(
        ctx.sid,
        RunningSession {
            workspace_id: ctx.ws,
            task: tokio::spawn(std::future::pending()),
            killer: None,
            live: Some(ctx.live.clone()),
        },
    );
    (dir, ctx, fake, id)
}

async fn decide(
    ctx: &Ctx,
    id: ApprovalId,
    allow: bool,
    message: &str,
    answers: Option<serde_json::Value>,
) -> serde_json::Value {
    let response = crate::api::decide_approval(
        axum::extract::State(ctx.st.clone()),
        axum::extract::Path(id.to_string()),
        axum::Json(crate::api::DecideRequest {
            allow,
            message: message.into(),
            answers: answers.and_then(|value| value.as_object().cloned()),
        }),
    )
    .await
    .unwrap_or_else(|error| panic!("{}", error.message()));
    response.0
}

#[tokio::test]
async fn approval_answers_survive_failed_delivery_and_replay() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    fake.append_failed.store(true, Ordering::SeqCst);
    let answers = serde_json::json!({"Choice?":"A"});
    assert_eq!(
        decide(&ctx, id, true, "", Some(answers.clone())).await["delivered"],
        false
    );
    let pending = ctx.st.db.undelivered_approvals(ctx.sid).await.unwrap();
    fake.append_failed.store(false, Ordering::SeqCst);
    deliver_saved_approval(&ctx.st, &pending[0]).await.unwrap();
    let inputs = fake.inputs.lock().unwrap();
    let response: serde_json::Value = serde_json::from_str(&inputs[0].1).unwrap();
    assert_eq!(
        response["response"]["response"]["updatedInput"]["answers"],
        answers
    );
}

#[tokio::test]
async fn approval_denial_message_survives_failed_delivery_and_replay() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    fake.append_failed.store(true, Ordering::SeqCst);
    assert_eq!(
        decide(&ctx, id, false, "请先备份", None).await["delivered"],
        false
    );
    let pending = ctx.st.db.undelivered_approvals(ctx.sid).await.unwrap();
    fake.append_failed.store(false, Ordering::SeqCst);
    deliver_saved_approval(&ctx.st, &pending[0]).await.unwrap();
    let inputs = fake.inputs.lock().unwrap();
    let response: serde_json::Value = serde_json::from_str(&inputs[0].1).unwrap();
    assert_eq!(response["response"]["response"]["message"], "请先备份");
}

#[tokio::test]
async fn approval_delivery_is_not_acknowledged_until_its_event_is_persisted() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    reject_events(&ctx).await;
    assert_eq!(
        decide(&ctx, id, true, "", Some(serde_json::json!({"Choice?":"A"}))).await["delivered"],
        false
    );
    assert_eq!(
        ctx.st
            .db
            .undelivered_approvals(ctx.sid)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(ctx.live.state.lock().await.next_seq, 1);
    sqlx::query("DROP TRIGGER reject_event")
        .execute(ctx.st.db.pool())
        .await
        .unwrap();
    assert_eq!(
        decide(&ctx, id, true, "", Some(serde_json::json!({"Choice?":"A"}))).await["delivered"],
        true
    );
    assert!(
        ctx.st
            .db
            .undelivered_approvals(ctx.sid)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 1);
    assert_eq!(fake.inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn approval_waiting_for_delivery_remains_visible() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    fake.append_failed.store(true, Ordering::SeqCst);
    let _ = decide(&ctx, id, true, "", Some(serde_json::json!({"Choice?":"A"}))).await;
    assert_eq!(
        ctx.st
            .db
            .pending_approvals(Some(ctx.ws))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn approval_legacy_response_is_never_guessed() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    let approval = ctx
        .st
        .db
        .decide_approval(id, "allow", None)
        .await
        .unwrap()
        .unwrap();
    let result = deliver_saved_approval(&ctx.st, &approval).await;
    assert!(result.is_err());
    assert!(fake.inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn approval_retry_cannot_replace_the_original_decision_or_answers() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    fake.append_failed.store(true, Ordering::SeqCst);
    let original = serde_json::json!({"Choice?":"A"});
    let _ = decide(&ctx, id, true, "", Some(original.clone())).await;
    fake.append_failed.store(false, Ordering::SeqCst);
    assert_eq!(
        decide(&ctx, id, false, "changed", None).await["delivered"],
        false
    );
    assert_eq!(
        decide(&ctx, id, true, "", Some(serde_json::json!({"Choice?":"B"}))).await["delivered"],
        false
    );
    assert!(fake.inputs.lock().unwrap().is_empty());
    assert_eq!(
        decide(&ctx, id, true, "", Some(original.clone())).await["delivered"],
        true
    );
    assert_eq!(
        decide(&ctx, id, true, "", Some(original)).await["delivered"],
        true
    );
    {
        let inputs = fake.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1);
        assert!(!inputs[0].1.contains("_blazar_response"));
    }
    assert_eq!(ctx.st.db.max_seq(ctx.sid).await.unwrap(), 1);
}

#[tokio::test]
async fn approval_compatibility_api_rejects_different_saved_response() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    let answers = serde_json::json!({"Choice?":"A"});
    fake.append_failed.store(true, Ordering::SeqCst);
    let _ = decide(&ctx, id, true, "", Some(answers.clone())).await;
    let saved = ctx.st.db.approval(id).await.unwrap().unwrap();
    fake.append_failed.store(false, Ordering::SeqCst);
    assert!(
        deliver_approval(&ctx.st, &saved, false, "changed", None)
            .await
            .is_err()
    );
    assert!(fake.inputs.lock().unwrap().is_empty());
    deliver_approval(&ctx.st, &saved, true, "", Some(&answers))
        .await
        .unwrap();
    assert_eq!(fake.inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn approval_retry_worker_delivers_saved_decision() {
    let (_dir, ctx, fake, id) = approval_fixture().await;
    let answers = serde_json::json!({"Choice?":"A"});
    fake.append_failed.store(true, Ordering::SeqCst);
    let _ = decide(&ctx, id, true, "", Some(answers)).await;
    fake.append_failed.store(false, Ordering::SeqCst);
    let worker = tokio::spawn(retry_approvals(ctx.clone()));
    tokio::time::timeout(Duration::from_secs(3), async {
        while !ctx
            .st
            .db
            .undelivered_approvals(ctx.sid)
            .await
            .unwrap()
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    worker.abort();
    assert_eq!(fake.inputs.lock().unwrap().len(), 1);
}
