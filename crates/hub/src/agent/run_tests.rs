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
    probes: std::sync::Mutex<std::collections::VecDeque<&'static str>>,
    group_cleared: AtomicBool,
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
                let next = self
                    .probes
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or("exited=0");
                format!("size={}\n{next}\n", self.log.len())
            } else if script.contains("TABLE=$(ps") {
                self.group_cleared.store(true, Ordering::SeqCst);
                "stopped".into()
            } else if script.contains("137 > exit.code.tmp") {
                String::new()
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
        probes: std::sync::Mutex::new(std::collections::VecDeque::new()),
        group_cleared: AtomicBool::new(false),
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
            unreplayed: Vec::new(),
            user_no: 1,
            eof_sent: false,
            interrupt_requested: false,
            interrupted_at: None,
            bg: Default::default(),
            idle: false,
            app: Default::default(),
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
async fn a_failed_last_turn_marks_the_run_failed_even_when_the_cli_exits_cleanly() {
    let failed = r#"{"type":"result","subtype":"success","is_error":true,"terminal_reason":"api_error","result":"API Error: Connection error."}"#;
    let (_dir, ctx, _transport) = fixture(format!("{failed}\n")).await;
    tokio::time::timeout(Duration::from_secs(3), supervise(ctx.clone(), 0, None))
        .await
        .unwrap();
    assert_eq!(session_state(&ctx).await.0, "failed");
}

#[tokio::test]
async fn one_unrecognised_probe_does_not_end_a_run_that_is_still_going() {
    let (_dir, ctx, transport) = fixture(format!("{FINISHED}\n")).await;
    transport
        .probes
        .lock()
        .unwrap()
        .extend(["lost", "alive", "exited=0"]);
    tokio::time::timeout(Duration::from_secs(10), supervise(ctx.clone(), 0, None))
        .await
        .unwrap();
    assert_eq!(session_state(&ctx).await.0, "done");
    assert!(!transport.group_cleared.load(Ordering::SeqCst));
    assert!(transport.removed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_run_lost_twice_in_a_row_is_given_up_after_its_group_is_cleared() {
    let (_dir, ctx, transport) = fixture(String::new()).await;
    transport.probes.lock().unwrap().extend(["lost", "lost"]);
    tokio::time::timeout(Duration::from_secs(10), supervise(ctx.clone(), 0, None))
        .await
        .unwrap();
    assert_eq!(session_state(&ctx).await.0, "failed");
    assert!(transport.group_cleared.load(Ordering::SeqCst));
    assert!(
        crate::failures::last_message(&ctx.st, &ctx.sid.to_string())
            .await
            .starts_with("agent process is gone")
    );
    assert!(
        !transport.removed.load(Ordering::SeqCst),
        "放弃的运行留着目录以便排查"
    );
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

#[test]
fn a_replay_that_merges_queued_messages_settles_all_of_them() {
    let mut unreplayed = vec![
        "notice one".to_owned(),
        "notice two".to_owned(),
        "later".to_owned(),
    ];
    assert_eq!(take_replayed(&mut unreplayed, "notice one\nnotice two"), 2);
    assert_eq!(unreplayed, ["later"]);
    assert_eq!(take_replayed(&mut unreplayed, "the first prompt"), 1);
    assert_eq!(unreplayed, ["later"]);
    let mut twice = vec!["go on".to_owned(), "go on".to_owned()];
    assert_eq!(take_replayed(&mut twice, "go on"), 1);
    assert_eq!(twice, ["go on"]);
}

#[test]
fn only_a_leading_command_name_is_a_slash_command() {
    assert!(crate::api::is_slash_command("/compact"));
    assert!(crate::api::is_slash_command("  /goal clear"));
    assert!(!crate::api::is_slash_command(
        "/dev/mapper/ubuntu--vg-ubuntu--lv is full"
    ));
    assert!(!crate::api::is_slash_command("/"));
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

#[tokio::test]
async fn a_prompt_with_interject_joins_the_running_turn() {
    let (dir, ctx, fake) = fixture(String::new()).await;
    let live = Arc::new(Live {
        run: DetachedRun::attach(
            fake.clone(),
            dir.path().join("run").display().to_string(),
            ctx.sid.to_string(),
        ),
        interactive: true,
        state: Mutex::new(LiveState {
            next_seq: 1,
            pending_user: 1,
            unreplayed: Vec::new(),
            user_no: 1,
            eof_sent: false,
            interrupt_requested: false,
            interrupted_at: None,
            bg: Default::default(),
            idle: false,
            app: Default::default(),
        }),
    });
    ctx.st.running.write().await.insert(
        ctx.sid,
        RunningSession {
            workspace_id: ctx.ws,
            task: tokio::spawn(std::future::pending()),
            killer: None,
            live: Some(live),
        },
    );
    let send = |interject: bool| {
        let st = ctx.st.clone();
        let ws = ctx.ws.to_string();
        async move {
            crate::api::prompt(
                axum::extract::State(st),
                axum::extract::Path(ws),
                axum::Json(
                    serde_json::from_value(serde_json::json!({
                        "text": "顺手把检查点存一下",
                        "interject": interject,
                    }))
                    .unwrap(),
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("{}", error.message()))
            .0
        }
    };
    let refused = send(false).await;
    assert_eq!(refused["admitted"], false);
    assert!(fake.inputs.lock().unwrap().is_empty());
    let joined = send(true).await;
    assert_eq!(joined["interjected"], true);
    assert_eq!(joined["session_id"], ctx.sid.to_string());
    let inputs = fake.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].1.contains("顺手把检查点存一下"));
}

#[test]
fn chain_uuid_skips_messages_the_cli_never_persists() {
    let said = r#"{"type":"assistant","message":{"id":"msg_1","model":"claude-opus-5-5","content":[{"type":"text","text":"好"}]},"parent_tool_use_id":null,"uuid":"0b9d2c5e-1a2b-4c3d-9e8f-001122334455","session_id":"s"}"#;
    assert_eq!(
        chain_uuid(said).as_deref(),
        Some("0b9d2c5e-1a2b-4c3d-9e8f-001122334455")
    );
    let typed = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"继续"}]},"parent_tool_use_id":null,"uuid":"1b9d2c5e-1a2b-4c3d-9e8f-001122334455","session_id":"s"}"#;
    assert_eq!(
        chain_uuid(typed).as_deref(),
        Some("1b9d2c5e-1a2b-4c3d-9e8f-001122334455")
    );
    let api_error = r#"{"type":"assistant","message":{"id":"err","model":"<synthetic>","content":[{"type":"text","text":"API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)"}]},"parent_tool_use_id":null,"uuid":"9b915e37-5c1e-4a7e-9d3b-2f1c0a8e7b6d","session_id":"s"}"#;
    assert_eq!(chain_uuid(api_error), None, "合成的错误消息不进会话链");
    let flagged = r#"{"type":"assistant","error":"authentication_failed","message":{"content":[{"type":"text","text":"Not logged in · Please run /login"}]},"parent_tool_use_id":null,"uuid":"2b9d2c5e-1a2b-4c3d-9e8f-001122334455"}"#;
    assert_eq!(chain_uuid(flagged), None);
    let sub = r#"{"type":"assistant","message":{"content":[]},"parent_tool_use_id":"toolu_1","uuid":"3b9d2c5e-1a2b-4c3d-9e8f-001122334455"}"#;
    assert_eq!(chain_uuid(sub), None, "子智能体的消息在另一条链上");
    assert_eq!(chain_uuid(r#"{"type":"result","uuid":"x"}"#), None);
    assert_eq!(chain_uuid("not json \"uuid\""), None);
}

#[test]
fn a_vanished_agent_interrupts_its_approval_while_a_stop_or_an_exit_just_ends_it() {
    assert_eq!(voided_decision(false, None), ApprovalDecision::Interrupted);
    assert_eq!(voided_decision(true, None), ApprovalDecision::Aborted);
    assert_eq!(voided_decision(false, Some(1)), ApprovalDecision::Aborted);
    assert_eq!(voided_decision(false, Some(0)), ApprovalDecision::Aborted);
}

#[tokio::test]
async fn approvals_voided_by_an_older_build_get_their_resolution_once() {
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    let st = AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-void-tests")),
        crate::services::Services::Isolated,
    );
    let pool = st.db.pool();
    let ws = "0a0b0c0d-0000-4000-8000-000000000001";
    sqlx::query("INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n1', 'gpu-x', 'ssh', 'online', '0')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO workspaces (id, node_id, name, path, activity, created_at) VALUES (?1, 'n1', 'repo', '/home/me/repo', 'idle', '0')")
        .bind(ws).execute(pool).await.unwrap();
    let cases = [
        (
            "0a0b0c0d-0000-4000-8000-0000000000a1",
            "0a0b0c0d-0000-4000-8000-0000000000b1",
            "failed",
            None,
            false,
        ),
        (
            "0a0b0c0d-0000-4000-8000-0000000000a2",
            "0a0b0c0d-0000-4000-8000-0000000000b2",
            "interrupted",
            None,
            false,
        ),
        (
            "0a0b0c0d-0000-4000-8000-0000000000a3",
            "0a0b0c0d-0000-4000-8000-0000000000b3",
            "failed",
            Some(1),
            true,
        ),
        (
            "0a0b0c0d-0000-4000-8000-0000000000a4",
            "0a0b0c0d-0000-4000-8000-0000000000b4",
            "running",
            None,
            false,
        ),
    ];
    for (sid, aid, status, code, resolved) in cases {
        sqlx::query("INSERT INTO sessions (id, workspace_id, runtime_kind, status, exit_code, created_at) VALUES (?1, ?2, 'claude', ?3, ?4, '1')")
            .bind(sid).bind(ws).bind(status).bind(code).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO approvals (id, session_id, request, decision, decided_at, created_at) VALUES (?1, ?2, '{}', 'aborted', '2', '1')")
            .bind(aid).bind(sid).execute(pool).await.unwrap();
        let mut payloads = vec![format!(
            r#"{{"type":"approval","id":"{aid}","request":{{}}}}"#
        )];
        if resolved {
            payloads.push(format!(
                r#"{{"type":"approval_resolved","id":"{aid}","decision":{{"kind":"aborted"}}}}"#
            ));
        }
        for (i, p) in payloads.iter().enumerate() {
            sqlx::query(
                "INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, ?2, '1', ?3)",
            )
            .bind(sid)
            .bind(i64::try_from(i).unwrap() + 1)
            .bind(p)
            .execute(pool)
            .await
            .unwrap();
        }
    }
    let resolutions = |sid: &'static str| {
        let st = st.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT payload FROM events WHERE session_id = ?1 AND payload LIKE '{\"type\":\"approval_resolved\"%' ORDER BY seq",
            )
            .bind(sid)
            .fetch_all(st.db.pool())
            .await
            .unwrap()
        }
    };

    settle_voided_approvals(&st).await;
    settle_voided_approvals(&st).await;

    let vanished = resolutions("0a0b0c0d-0000-4000-8000-0000000000a1").await;
    assert_eq!(vanished.len(), 1, "只补一次");
    assert!(
        vanished[0].contains(r#""decision":{"kind":"interrupted"}"#),
        "{vanished:?}"
    );
    let stopped = resolutions("0a0b0c0d-0000-4000-8000-0000000000a2").await;
    assert_eq!(stopped.len(), 1);
    assert!(
        stopped[0].contains(r#""decision":{"kind":"aborted"}"#),
        "用户停下的不算重启打断"
    );
    assert_eq!(
        resolutions("0a0b0c0d-0000-4000-8000-0000000000a3")
            .await
            .len(),
        1,
        "已有结果的不再补"
    );
    assert!(
        resolutions("0a0b0c0d-0000-4000-8000-0000000000a4")
            .await
            .is_empty(),
        "还在跑的会话不动"
    );
}

const PRICED: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","result":"done","total_cost_usd":0.1839295,"usage":{"input_tokens":4,"output_tokens":140}}"#;

#[tokio::test]
async fn endpoint_turns_keep_their_tokens_but_not_the_cli_price() {
    for endpoint in [false, true] {
        let (_dir, ctx, _) = fixture(String::new()).await;
        if endpoint {
            sqlx::query(
                "INSERT INTO accounts (id, provider, display_name, auth_mode, status, config_dir, created_at, endpoint)
                 VALUES ('ds', 'claude', 'DeepSeek', 'endpoint', 'ok', '/tmp/none', '0', '{}')",
            )
            .execute(ctx.st.db.pool())
            .await
            .unwrap();
            sqlx::query("UPDATE sessions SET account_id = 'ds' WHERE id = ?1")
                .bind(ctx.sid.to_string())
                .execute(ctx.st.db.pool())
                .await
                .unwrap();
        }
        let mut p = progress();
        process_line(&ctx, PRICED, 0, &mut p).await;
        let all = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM events WHERE session_id = ?1 ORDER BY seq",
        )
        .bind(ctx.sid.to_string())
        .fetch_all(ctx.st.db.pool())
        .await
        .unwrap()
        .join("\n");
        assert!(all.contains("\"output\":140"), "用量照记：{all}");
        assert_eq!(
            all.contains("cost_usd"),
            !endpoint,
            "命令行按 Claude 价格估的钱不能算到自定义接口头上：{all}"
        );
    }
}
