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
    stdin: std::sync::Mutex<Vec<String>>,
}

impl NodeTransport for FakeTransport {
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
            let script = spec.args.last().unwrap();
            let (code, stdout) = if script == "deny-start" {
                (2, "test hook denied startup".into())
            } else if script.contains("__BLAZAR_LAUNCHED__") {
                self.launched.store(true, Ordering::SeqCst);
                if let Some(payload) = &spec.stdin {
                    self.stdin
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(payload).into_owned());
                }
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
            } else if script.contains("TABLE=$(ps -eo pid=,pgid=,stat=)") {
                (
                    0,
                    if self.killed.load(Ordering::SeqCst) {
                        "stopped\n"
                    } else {
                        "alive=12345\n"
                    }
                    .into(),
                )
            } else if script.contains("builtin kill -TERM") || script.contains("builtin kill -KILL")
            {
                if self.fail_kill.load(Ordering::SeqCst) {
                    return Err(TransportError::Command {
                        code: 1,
                        stderr: "fake stop unavailable".into(),
                    });
                }
                self.killed.store(true, Ordering::SeqCst);
                (0, String::new())
            } else if script.contains("echo 137 > exit.code.tmp") {
                assert!(self.killed.load(Ordering::SeqCst));
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
        stdin: std::sync::Mutex::new(Vec::new()),
    });
    (dir, st, ws, fake)
}

fn launched_command(fake: &FakeTransport) -> String {
    use base64::prelude::*;
    let payload = fake
        .stdin
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap_or_default();
    let cmd = payload.lines().nth(1).unwrap_or_default();
    String::from_utf8(BASE64_STANDARD.decode(cmd).unwrap()).unwrap()
}

async fn resumable_prior(st: &Shared, ws: WorkspaceId) {
    let prior = SessionId::new();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, node, provider_session_id, last_uuid)
         VALUES (?1, ?2, 'claude', 'done', '1', 'local', 'p1', '0b9d2c5e-1a2b-4c3d-9e8f-001122334455')",
    )
    .bind(prior.to_string()).bind(ws.to_string()).execute(st.db.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, 1, '1', '{\"type\":\"session_started\",\"provider_session_id\":\"p1\"}')",
    )
    .bind(prior.to_string()).execute(st.db.pool()).await.unwrap();
}

#[tokio::test]
async fn resuming_at_the_last_message_is_opt_in() {
    for anchored in [true, false] {
        let (_dir, st, ws, fake) = fixture().await;
        resumable_prior(&st, ws).await;
        let req = serde_json::from_value(serde_json::json!({
            "text": "继续", "agent": "claude", "resume": true, "resume_at_last": anchored,
        }))
        .unwrap();
        let transport = fake.clone();
        let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
            .await
            .unwrap_or_else(|error| panic!("{}", error.message()));
        let cmd = launched_command(&fake).replace('\'', "");
        let exec = cmd
            .lines()
            .find(|l| l.starts_with("exec "))
            .unwrap_or_default();
        assert!(exec.contains("--resume p1"), "{exec}");
        assert_eq!(
            exec.contains("--resume-session-at 0b9d2c5e-1a2b-4c3d-9e8f-001122334455"),
            anchored,
            "{exec}"
        );
    }
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
async fn assistant_sessions_always_carry_the_fleet_tools_and_briefing() {
    let (_dir, st, ws, fake) = fixture().await;
    let _ = crate::office::HUB_URL.set("http://127.0.0.1:7777".into());
    let _ = prompt_fake(st.clone(), ws, fake.clone(), "claude")
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let plain = launched_command(&fake);
    assert!(!plain.contains("blazar-fleet"), "{plain}");
    for k in INHERITED_API_ENV {
        assert!(plain.contains(&format!("unset {k}\n")), "{plain}");
    }
    let assistant = crate::api::assistant::ensure(&st)
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let req = serde_json::from_value(serde_json::json!({"text":"看看", "agent":"claude"})).unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), assistant, req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let cmd = launched_command(&fake);
    assert!(cmd.contains("blazar-fleet"), "{cmd}");
    assert!(cmd.contains("你是用户的助手"), "{cmd}");
    assert!(cmd.contains("repo"), "{cmd}");
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
    assert!(fake.killed.load(Ordering::SeqCst));
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
    assert!(!fake.killed.load(Ordering::SeqCst));
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

#[tokio::test]
async fn a_new_turn_leaves_the_conversations_cards_open() {
    let (_dir, st, ws, fake) = fixture().await;
    let prior = SessionId::new();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, node)
         VALUES (?1, ?2, 'claude', 'done', '1', 'local')",
    )
    .bind(prior.to_string())
    .bind(ws.to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    let ask = blazar_core_types::api::Ask {
        question: "推不推？".into(),
        options: vec!["推".into(), "不推".into()],
    };
    let card = super::asks::post(&st, &prior.to_string(), &ask, "")
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let req = serde_json::from_value(serde_json::json!({
        "text": "先说说别的", "agent": "claude", "resume": true, "resume_session": prior.to_string(),
    }))
    .unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    assert!(fake.launched.load(Ordering::SeqCst));
    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inbox WHERE id = ?1 AND archived_at IS NULL AND resolved_at IS NULL",
    )
    .bind(&card)
    .fetch_one(st.db.pool())
    .await
    .unwrap();
    assert_eq!(open, 1);
}

#[tokio::test]
async fn turns_take_the_workspace_default_and_conversations_keep_what_they_ran_with() {
    let (_dir, st, ws, fake) = fixture().await;
    let pick = |m: &str, e: Option<&str>| super::model_choice::Choice {
        model: Some(m.into()),
        effort: e.map(str::to_owned),
    };
    super::model_choice::put(
        &st,
        super::model_choice::GLOBAL,
        "claude",
        &pick("global-model", None),
    )
    .await
    .unwrap();
    super::model_choice::put(
        &st,
        &super::model_choice::ws_scope(&ws.to_string()),
        "claude",
        &pick("ws-model", Some("high")),
    )
    .await
    .unwrap();
    let out = prompt_fake(st.clone(), ws, fake.clone(), "claude")
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()))
        .0;
    let exec = launched_command(&fake).replace('\'', "");
    assert!(exec.contains("--model ws-model"), "{exec}");
    assert!(exec.contains("--effort high"), "{exec}");
    let thread = out["thread_id"].as_str().unwrap().to_owned();
    assert_eq!(
        super::model_choice::thread_layer(&st, &thread).await["claude"],
        pick("ws-model", Some("high")),
        "the turn leaves what it ran with on its conversation"
    );
    super::model_choice::put(
        &st,
        &super::model_choice::ws_scope(&ws.to_string()),
        "claude",
        &pick("newer-default", None),
    )
    .await
    .unwrap();
    let chosen = super::model_choice::for_turn(
        &st,
        &ws.to_string(),
        Some(&thread),
        "claude",
        Default::default(),
        Default::default(),
    )
    .await;
    assert_eq!(
        chosen,
        pick("ws-model", Some("high")),
        "the conversation keeps its model"
    );
}

async fn endpoint_account(st: &Shared, dir: &std::path::Path) -> (String, std::path::PathBuf) {
    let home = dir.join("accounts/claude/ds");
    std::fs::create_dir_all(&home).unwrap();
    let key = home.join(crate::accounts::KEY_FILE);
    std::fs::write(&key, "sk-endpoint-secret-0001").unwrap();
    let endpoint = serde_json::json!({
        "base_url": "https://api.deepseek.com/anthropic",
        "models": ["deepseek-v4-pro", "deepseek-flash"],
        "default_model": "deepseek-v4-pro",
        "small_model": "deepseek-flash",
        "preset": "deepseek",
    });
    sqlx::query(
        "INSERT INTO accounts (id, provider, display_name, auth_mode, status, config_dir, created_at, endpoint)
         VALUES ('ds', 'claude', 'DeepSeek', 'endpoint', 'ok', ?1, '0', ?2)",
    )
    .bind(home.display().to_string())
    .bind(endpoint.to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    ("ds".into(), key)
}

fn exec_line(fake: &FakeTransport) -> String {
    launched_command(fake)
        .replace('\'', "")
        .lines()
        .find(|l| l.starts_with("exec "))
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn an_endpoint_turn_runs_its_own_model_with_the_key_read_from_disk() {
    let (dir, st, ws, fake) = fixture().await;
    let (id, key) = endpoint_account(&st, dir.path()).await;
    let req = serde_json::from_value(serde_json::json!({
        "text": "hi", "agent": "claude", "account": id, "model": "opus", "effort": "high",
    }))
    .unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let cmd = launched_command(&fake);
    assert!(
        cmd.contains("export ANTHROPIC_BASE_URL='https://api.deepseek.com/anthropic'"),
        "{cmd}"
    );
    assert!(
        cmd.contains(&format!(
            "export ANTHROPIC_AUTH_TOKEN=\"$(cat -- '{}')\"",
            key.display()
        )),
        "{cmd}"
    );
    assert!(cmd.contains("export ANTHROPIC_DEFAULT_HAIKU_MODEL='deepseek-flash'"));
    assert!(cmd.contains("unset CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(
        !cmd.contains("sk-endpoint-secret"),
        "密钥原文不能进启动脚本"
    );
    let exec = exec_line(&fake);
    assert!(exec.contains("--model deepseek-v4-pro"), "{exec}");
    assert!(!exec.contains("--effort"), "{exec}");
    let model_arg: Option<String> = sqlx::query_scalar("SELECT model_arg FROM sessions")
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert_eq!(model_arg.as_deref(), Some("deepseek-v4-pro"));
}

#[tokio::test]
async fn moving_a_conversation_between_endpoint_and_claude_hands_it_over() {
    let (dir, st, ws, fake) = fixture().await;
    let (id, _) = endpoint_account(&st, dir.path()).await;
    resumable_prior(&st, ws).await;
    sqlx::query("UPDATE sessions SET account_id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await
        .unwrap();
    let req = serde_json::from_value(serde_json::json!({
        "text": "继续", "agent": "claude", "resume": true, "account": "claude-default",
    }))
    .unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let exec = exec_line(&fake);
    assert!(
        !exec.contains("--resume"),
        "DeepSeek 的思考块不能带去 Anthropic：{exec}"
    );

    let (_dir2, st2, ws2, fake2) = fixture().await;
    resumable_prior(&st2, ws2).await;
    let req = serde_json::from_value(serde_json::json!({
        "text": "继续", "agent": "claude", "resume": true,
    }))
    .unwrap();
    let transport = fake2.clone();
    let _ = prompt_using(st2.clone(), ws2.to_string(), req, move |_| {
        transport.clone()
    })
    .await
    .unwrap_or_else(|error| panic!("{}", error.message()));
    assert!(
        exec_line(&fake2).contains("--resume p1"),
        "同一类账号照常续接"
    );
}

#[tokio::test]
async fn an_endpoint_model_is_not_sent_to_a_claude_account() {
    let (dir, st, ws, fake) = fixture().await;
    endpoint_account(&st, dir.path()).await;
    let req = serde_json::from_value(serde_json::json!({
        "text": "hi", "agent": "claude", "account": "claude-default", "model": "deepseek-flash",
    }))
    .unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let exec = exec_line(&fake);
    assert!(!exec.contains("deepseek-flash"), "{exec}");
    assert!(!launched_command(&fake).contains("api.deepseek.com"));
}

#[tokio::test]
async fn an_auto_turn_runs_where_routing_points_and_says_why_under_the_message() {
    let (_dir, st, ws, fake) = fixture().await;
    crate::office::kv_put(
        &st,
        "routing",
        &serde_json::json!({ "channels": ["codex", "claude"], "first": "codex" }),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rate_limit_snapshots (account_id, window_name, utilization, resets_at, observed_at)
         VALUES ('codex-default', 'five_hour', 0.9, ?1, ?2)",
    )
    .bind((Utc::now() + chrono::Duration::hours(2)).to_rfc3339())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await
    .unwrap();
    let req = serde_json::from_value(serde_json::json!({ "text": "hi", "agent": "auto" })).unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    assert!(fake.launched.load(Ordering::SeqCst));
    let (runtime, route, request): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT runtime_kind, route, request FROM sessions")
            .fetch_one(st.db.pool())
            .await
            .unwrap();
    assert_eq!(runtime, "claude");
    let route: serde_json::Value = serde_json::from_str(&route.unwrap()).unwrap();
    assert_eq!(route["channel"], "claude");
    assert_eq!(route["skipped"][0]["why"], "over_line");
    let request: serde_json::Value = serde_json::from_str(&request.unwrap()).unwrap();
    assert_eq!(request["agent"], "claude");
    let events: Vec<(i64, String)> = sqlx::query_as("SELECT seq, payload FROM events ORDER BY seq")
        .fetch_all(st.db.pool())
        .await
        .unwrap();
    assert!(events[0].1.contains("user_message"), "{events:?}");
    assert_eq!(events[1].0, 2);
    assert!(
        events[1]
            .1
            .contains("Auto → Claude Code · Codex is over its usage line"),
        "{events:?}"
    );
}

async fn auto_prior(st: &Shared, ws: WorkspaceId) -> String {
    let prior = SessionId::new().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, node, provider_session_id, account_id, route)
         VALUES (?1, ?2, 'claude', 'done', '1', 'local', 'p1', 'claude-default', ?3)",
    )
    .bind(&prior)
    .bind(ws.to_string())
    .bind(r#"{"channel":"claude","runtime":"claude","account":"claude-default","model":null,"label":"Claude Code"}"#)
    .execute(st.db.pool())
    .await
    .unwrap();
    for (seq, payload) in [
        (
            1,
            r#"{"type":"user_message","text":"what does this repo do?"}"#,
        ),
        (
            2,
            r#"{"type":"session_started","provider_session_id":"p1","model":"claude-opus-5-5"}"#,
        ),
    ] {
        sqlx::query("INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, ?2, '1', ?3)")
            .bind(&prior)
            .bind(seq)
            .bind(payload)
            .execute(st.db.pool())
            .await
            .unwrap();
    }
    prior
}

async fn picking_jev(wanted: &'static str) -> String {
    let app = axum::Router::new().route(
        "/v1/systemone",
        axum::routing::post(move |body: axum::Json<serde_json::Value>| async move {
            let criteria = body.0["questions"]["model"]["criteria"].clone();
            let keys: Vec<String> = criteria
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let pick = keys
                .iter()
                .find(|k| {
                    criteria[k.as_str()]
                        .as_str()
                        .is_some_and(|a| a.contains(wanted))
                })
                .cloned()
                .unwrap_or_default();
            let probs: serde_json::Map<String, serde_json::Value> = keys
                .iter()
                .map(|k| {
                    (
                        k.clone(),
                        serde_json::json!(if *k == pick { 0.8 } else { 0.01 }),
                    )
                })
                .collect();
            axum::Json(serde_json::json!({
                "answers": {
                    "difficulty": { "score": 0.2, "confidence": 0.9 },
                    "kind": { "choice": "question", "confidence": 0.9 },
                    "model": { "choice": pick, "confidence": 0.8, "probabilities": probs }
                }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1/systemone")
}

async fn latest_turn(st: &Shared) -> (String, Option<String>, Vec<String>) {
    let (id, route): (String, Option<String>) =
        sqlx::query_as("SELECT id, route FROM sessions ORDER BY created_at DESC LIMIT 1")
            .fetch_one(st.db.pool())
            .await
            .unwrap();
    let events: Vec<String> =
        sqlx::query_scalar("SELECT payload FROM events WHERE session_id = ?1 ORDER BY seq")
            .bind(&id)
            .fetch_all(st.db.pool())
            .await
            .unwrap();
    (id, route, events)
}

#[tokio::test]
async fn an_auto_followup_stays_put_without_a_clear_reason_to_move() {
    let (_dir, st, ws, fake) = fixture().await;
    let thread = auto_prior(&st, ws).await;
    let req = serde_json::from_value(serde_json::json!({
        "text": "and the tests?", "agent": "auto", "resume": true, "resume_session": thread,
    }))
    .unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let (_, route, events) = latest_turn(&st).await;
    let route: serde_json::Value = serde_json::from_str(&route.unwrap()).unwrap();
    assert_eq!(route["kept"], true);
    assert!(events.iter().all(|e| !e.contains("Auto →")), "{events:?}");
    assert!(
        !exec_line(&fake).contains("--model"),
        "{}",
        exec_line(&fake)
    );
}

#[tokio::test]
async fn an_auto_followup_moves_to_a_lighter_model_when_jev_is_sure() {
    let (_dir, st, ws, fake) = fixture().await;
    let thread = auto_prior(&st, ws).await;
    crate::routing::jev::save_key(&st, "ts-key-123456").unwrap();
    let url = picking_jev("with Haiku").await;
    crate::office::kv_put(&st, "routing", &serde_json::json!({ "jev_url": url }))
        .await
        .unwrap();
    let req = serde_json::from_value(serde_json::json!({
        "text": "what is 2 + 2?", "agent": "auto", "resume": true, "resume_session": thread,
    }))
    .unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let (_, route, events) = latest_turn(&st).await;
    let route: serde_json::Value = serde_json::from_str(&route.unwrap()).unwrap();
    assert_eq!(route["model"], "haiku");
    assert!(
        events
            .iter()
            .any(|e| e.contains("Auto → Claude Code · Haiku · trivial question")),
        "{events:?}"
    );
    assert!(
        exec_line(&fake).contains("--model haiku"),
        "{}",
        exec_line(&fake)
    );
}

#[tokio::test]
async fn naming_a_model_in_an_auto_conversation_takes_it_off_auto() {
    let (_dir, st, ws, fake) = fixture().await;
    let thread = auto_prior(&st, ws).await;
    let req = serde_json::from_value(serde_json::json!({
        "text": "redo it", "agent": "claude", "model": "sonnet", "resume": true, "resume_session": thread,
    }))
    .unwrap();
    let transport = fake.clone();
    let _ = prompt_using(st.clone(), ws.to_string(), req, move |_| transport.clone())
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    let (_, route, _) = latest_turn(&st).await;
    assert_eq!(route, None);
    assert!(
        exec_line(&fake).contains("--model sonnet"),
        "{}",
        exec_line(&fake)
    );
}

#[tokio::test]
async fn back_on_auto_after_a_manual_switch_starts_from_the_manual_pick() {
    let (_dir, st, ws, _fake) = fixture().await;
    let thread = auto_prior(&st, ws).await;
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, node, thread_id, request)
         VALUES (?1, ?2, 'codex', 'done', '2', 'local', ?3, ?4)",
    )
    .bind(SessionId::new().to_string())
    .bind(ws.to_string())
    .bind(&thread)
    .bind(r#"{"text":"redo","agent":"codex","model":"gpt-5.5-codex"}"#)
    .execute(st.db.pool())
    .await
    .unwrap();
    let on_codex = crate::routing::current(&st, &thread, "codex", None).await;
    assert_eq!(on_codex.model.as_deref(), Some("gpt-5.5-codex"));
    assert_eq!(on_codex.channel, "codex");
    let stale = crate::routing::current(&st, &thread, "claude", None).await;
    assert_eq!(stale.model, None);
    assert_eq!(stale.opening, "what does this repo do?");
}
