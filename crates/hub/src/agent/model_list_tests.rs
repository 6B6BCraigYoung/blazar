use super::*;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn a_menu_open_rechecks_the_version_and_an_old_list_is_fetched_again() {
    assert_eq!(step(None, false), Step::Fetch);
    assert_eq!(step(None, true), Step::Fetch);
    assert_eq!(step(Some((secs(5), secs(5))), false), Step::Cached);
    assert_eq!(step(Some((secs(5), secs(5))), true), Step::Check);
    assert_eq!(
        step(Some((secs(600), RECHECK_AFTER)), false),
        Step::Check,
        "隔一阵子没核对版本，后台取列表时也核对一次"
    );
    assert_eq!(
        step(Some((REFETCH_AFTER, secs(1))), true),
        Step::Fetch,
        "版本没变但列表太旧，打开菜单时整份重取"
    );
    assert_eq!(step(Some((REFETCH_AFTER, secs(1))), false), Step::Cached);
}

#[test]
fn versions_compare_by_number_not_by_text() {
    assert!(same_version(Some("2.1.321"), Some("2.1.321 (Claude Code)")));
    assert!(!same_version(Some("2.1.302"), Some("2.1.321")));
    assert!(!same_version(None, Some("2.1.321")));
    assert!(!same_version(Some("garbage"), Some("garbage")));
}

const INIT: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"blazar-models","response":{"models":[{"value":"default","displayName":"Default (recommended)","description":"Use the default model (currently Opus 5.5) · $4/$20 per Mtok","supportedEffortLevels":["low","high"]},{"value":"fable","displayName":"Fable","description":"Fable 5.2 · Most capable for your hardest and longest-running tasks · $8/$40 per Mtok"}],"account":{"email":"someone@example.com","organization":"Org"},"pid":42}}}"#;

#[test]
fn the_answer_keeps_only_the_models_and_the_cli_version() {
    let other = r#"{"type":"control_response","response":{"request_id":"someone-else","response":{"models":[{"value":"x"}]}}}"#;
    let out = format!("noise\n{VERSION_MARK} 1794000000 2.1.321 (Claude Code)\n{other}\n{INIT}\n");
    let probe = parse(&out);
    assert_eq!(probe.version.as_deref(), Some("2.1.321"));
    assert_eq!(
        probe.installed,
        Some(1_794_000_000),
        "顺带读出这个版本装上的时间"
    );
    assert_eq!(
        parse(&format!("{VERSION_MARK} 0 ")).version,
        None,
        "没装命令行时版本为空"
    );
    let list = rows(&probe.models.unwrap());
    assert_eq!(list.len(), 2, "别人的应答不算");
    assert_eq!(list[0]["id"], "", "default 记成空，表示交给命令行的默认");
    assert_eq!(list[0]["efforts"], json!(["low", "high"]));
    assert_eq!(list[1]["id"], "fable");
    assert_eq!(list[1]["label"], "Fable");
    assert!(list[1]["desc"].as_str().unwrap().starts_with("Fable 5.2"));
    let text = serde_json::to_string(&list).unwrap();
    assert!(
        !text.contains("someone@example.com") && !text.contains("Org"),
        "账号信息不能经 API 流出: {text}"
    );
}

#[test]
fn secrets_travel_on_stdin_and_never_in_the_command() {
    let mut env = RunEnv::default();
    env.vars
        .insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "sekret-proxy-key".into());
    env.vars
        .insert("CLAUDE_CONFIG_DIR".into(), "/acct dir/it's".into());
    env.vars.insert("BAD KEY".into(), "x".into());
    env.files.insert(
        "TOKEN_FROM_FILE".into(),
        "/acct dir/.blazar-oauth-token".into(),
    );
    env.unset = vec!["ANTHROPIC_API_KEY".into(), "X;rm".into()];
    let block = env_block(&env);
    assert!(block.contains("unset ANTHROPIC_API_KEY\n"));
    assert!(!block.contains("X;rm"), "{block}");
    assert!(!block.contains("BAD KEY"), "{block}");
    assert!(block.contains("export CLAUDE_CONFIG_DIR='/acct dir/it'\\''s'\n"));
    assert!(block.contains(
        "export TOKEN_FROM_FILE=\"$(cat -- '/acct dir/.blazar-oauth-token' 2>/dev/null)\"\n"
    ));
    let cmd = script("/opt/claude bin/claude");
    assert!(!cmd.contains("sekret"), "令牌不能出现在命令行里");
    assert!(cmd.contains("P='/opt/claude bin/claude'"));
    let input = payload(&env);
    assert!(!input.contains("sekret"), "令牌只以编码后的形式走标准输入");
    let mut lines = input.lines();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(lines.next().unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(decoded).unwrap(), block);
    let init: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(init["request"]["subtype"], "initialize");
    assert_eq!(init["request_id"], REQUEST_ID);
}

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new(version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fake = Self { dir };
        fake.set_version(version);
        let body = r#"#!/bin/sh
if [ "$1" = "--version" ]; then cat "$(dirname "$0")/version"; exit 0; fi
echo run >> "$(dirname "$0")/runs"
IFS= read -r line
case "$CLAUDE_CODE_OAUTH_TOKEN" in sekret) tok=yes ;; *) tok=no ;; esac
printf '{"type":"control_response","response":{"request_id":"blazar-models","response":{"models":[{"value":"default","displayName":"Default","description":"dir=%s token=%s"},{"value":"preview","displayName":"Preview","description":"Preview %s"}]}}}\n' "$CLAUDE_CONFIG_DIR" "$tok" "$(cat "$(dirname "$0")/version")"
"#;
        let path = fake.program();
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        fake
    }

    fn program(&self) -> String {
        self.dir.path().join("claude").display().to_string()
    }

    fn set_version(&self, v: &str) {
        std::fs::write(
            self.dir.path().join("version"),
            format!("{v} (Claude Code)\n"),
        )
        .unwrap();
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("runs"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }
}

async fn hub() -> Shared {
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    crate::state::AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-model-list-tests")),
        crate::services::Services::Isolated,
    )
}

#[tokio::test]
async fn the_cli_runs_in_the_account_environment_without_the_token_leaking() {
    let st = hub().await;
    let fake = Fake::new("2.1.321");
    let token = fake.dir.path().join("token");
    std::fs::write(&token, "sekret").unwrap();
    let mut env = RunEnv::default();
    env.vars
        .insert("CLAUDE_CONFIG_DIR".into(), "/accounts/max".into());
    env.files.insert(
        "CLAUDE_CODE_OAUTH_TOKEN".into(),
        token.display().to_string(),
    );
    let (probe, models) = fetch(&st, "local", &fake.program(), &env).await.unwrap();
    assert_eq!(probe.version.as_deref(), Some("2.1.321"));
    assert!(probe.installed.is_some(), "读得到命令行装上的时间");
    let list = rows(&models);
    assert_eq!(
        list[0]["desc"], "dir=/accounts/max token=yes",
        "要用下一轮真正会用的账号环境去问命令行"
    );
    assert!(!serde_json::to_string(&list).unwrap().contains("sekret"));
}

#[tokio::test]
async fn a_missing_cli_is_said_plainly() {
    let st = hub().await;
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-claude-here").display().to_string();
    let err = fetch(&st, "local", &missing, &RunEnv::default())
        .await
        .unwrap_err();
    assert_eq!(err.0, "Claude Code isn't installed on this computer");
}

#[tokio::test]
async fn the_list_is_reused_until_the_cli_version_changes() {
    let st = hub().await;
    let fake = Fake::new("2.1.319");
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n-local', 'local', 'local', 'online', '0')",
    )
    .execute(st.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent_configs (id, agent_id, program_path, created_at, updated_at) VALUES ('c1', 'claude', ?1, '0', '0')",
    )
    .bind(fake.program())
    .execute(st.db.pool())
    .await
    .unwrap();
    let ws = "w1";
    let first = claude_for(&st, ws, "local", None, None, false).await;
    assert_eq!(first["cli_version"], "2.1.319");
    assert_eq!(first["models"][1]["desc"], "Preview 2.1.319 (Claude Code)");
    assert_eq!(first["error"], Value::Null);
    let again = claude_for(&st, ws, "local", None, None, false).await;
    assert_eq!(again["models"], first["models"]);
    let reopened = claude_for(&st, ws, "local", None, None, true).await;
    assert_eq!(reopened["models"], first["models"]);
    assert_eq!(
        fake.runs(),
        1,
        "版本没变，打开菜单只核对版本，不重新起命令行"
    );
    fake.set_version("2.1.321");
    let updated = claude_for(&st, ws, "local", None, None, true).await;
    assert_eq!(updated["cli_version"], "2.1.321");
    assert_eq!(
        updated["models"][1]["desc"],
        "Preview 2.1.321 (Claude Code)"
    );
    assert_eq!(fake.runs(), 2, "版本一变就作废缓存，重取列表");
    std::fs::remove_file(fake.program()).unwrap();
    let broken = claude_for(&st, ws, "local", None, None, true).await;
    assert_eq!(
        broken["models"], updated["models"],
        "取不到时退回上一次的结果"
    );
    assert_eq!(broken["stale"], true);
    assert!(
        broken["error"].as_str().unwrap().contains("this computer"),
        "{broken}"
    );
}

#[test]
fn model_ids_read_as_names() {
    assert_eq!(
        pretty_model("claude-fable-5-2").as_deref(),
        Some("Fable 5.2")
    );
    assert_eq!(
        pretty_model("claude-fable-5-1[1m]").as_deref(),
        Some("Fable 5.1 [1m]")
    );
    assert_eq!(pretty_model("claude-opus-5-5").as_deref(), Some("Opus 5.5"));
    assert_eq!(pretty_model("opus"), None);
    assert_eq!(pretty_model("claude-mythos-5-2"), None);
    assert_eq!(pretty_model("claude-fable-"), None);
}

#[test]
fn family_rows_use_the_alias_and_say_what_it_last_ran() {
    let rows = vec![
        json!({"id": "", "label": "Default (recommended)", "desc": "Sonnet 5.5 · Efficient for routine tasks", "efforts": []}),
        json!({"id": "claude-fable-5-1[1m]", "label": "Fable", "desc": "Fable 5.1 · Most capable for your hardest and longest-running tasks", "efforts": ["low"]}),
        json!({"id": "opus", "label": "Opus 5.5", "desc": "For complex work and everyday tasks", "efforts": []}),
        json!({"id": "claude-fable-5", "label": "Fable 5", "desc": "Older Fable", "efforts": []}),
    ];
    let last = HashMap::from([("fable".to_owned(), "claude-fable-5-2".to_owned())]);
    let out = alias_rows(rows, &last);
    assert_eq!(out[0]["id"], "", "Default 不动");
    assert_eq!(out[0]["desc"], "Sonnet 5.5 · Efficient for routine tasks");
    assert_eq!(
        out[1]["id"], "fable",
        "点下去存别名，由那台机器的命令行解析"
    );
    assert_eq!(out[1]["label"], "Fable");
    assert_eq!(
        out[1]["desc"],
        "Fable 5.2 · Last run on this machine · Most capable for your hardest and longest-running tasks"
    );
    assert_eq!(out[1]["efforts"], json!(["low"]));
    assert_eq!(out[2]["id"], "opus");
    assert_eq!(out[2]["label"], "Opus");
    assert_eq!(
        out[2]["desc"],
        "Newest Opus on this machine · For complex work and everyday tasks"
    );
    assert_eq!(
        out[3]["id"], "claude-fable-5",
        "同系列后面写死版本的行原样保留"
    );
}

#[tokio::test]
async fn the_last_run_of_each_alias_is_read_per_machine() {
    let st = hub().await;
    for q in [
        "INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n1', 'gpu', 'ssh', 'online', '0')",
        "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('w1', 'n1', 'w', '/tmp/w', '0')",
    ] {
        sqlx::query(q).execute(st.db.pool()).await.unwrap();
    }
    let now = chrono::Utc::now();
    let ago = |mins: i64| (now - chrono::Duration::minutes(mins)).to_rfc3339();
    let runs = [
        ("s1", "gpu", Some("fable"), None, 180, "claude-fable-5-1"),
        ("s2", "gpu", Some("fable"), None, 60, "claude-fable-5-2"),
        (
            "s3",
            "gpu",
            None,
            Some(r#"{"model":"opus"}"#),
            120,
            "claude-opus-5-5",
        ),
        ("s4", "local", Some("fable"), None, 10, "claude-fable-5-1"),
        (
            "s5",
            "gpu",
            Some("claude-fable-5-1[1m]"),
            None,
            5,
            "claude-fable-5-1",
        ),
        (
            "s6",
            "gpu",
            Some("sonnet"),
            None,
            90 * 24 * 60,
            "claude-sonnet-5",
        ),
    ];
    for (sid, node, arg, request, mins, model) in runs {
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, node, model_arg, request)
             VALUES (?1, 'w1', 'claude', 'done', ?2, ?3, ?4, ?5)",
        )
        .bind(sid)
        .bind(ago(mins))
        .bind(node)
        .bind(arg)
        .bind(request)
        .execute(st.db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, 1, ?2, ?3)")
            .bind(sid)
            .bind(ago(mins))
            .bind(format!(
                r#"{{"type":"session_started","provider_session_id":"p-{sid}","model":"{model}","cwd":"/tmp/w","permission_mode":null}}"#
            ))
            .execute(st.db.pool())
            .await
            .unwrap();
    }
    let installed = (now - chrono::Duration::minutes(90)).timestamp();
    assert_eq!(
        last_runs(&st, "gpu", Some(installed)).await,
        HashMap::from([("fable".to_owned(), "claude-fable-5-2".to_owned())]),
        "当前这个命令行版本装上之前的运行不算，旧版本跑出的型号说明不了现在"
    );
    let last = last_runs(&st, "gpu", None).await;
    assert_eq!(
        last,
        HashMap::from([
            ("fable".to_owned(), "claude-fable-5-2".to_owned()),
            ("opus".to_owned(), "claude-opus-5-5".to_owned()),
        ]),
        "只看这台机器、按别名发起、60 天内最近的一次；旧会话没有 model_arg 时读当时的请求"
    );
}

async fn endpoint_account(st: &Shared, dir: &std::path::Path) -> String {
    let id = "endpoint-ds".to_owned();
    std::fs::write(
        dir.join(crate::accounts::KEY_FILE),
        "sk-sekret-endpoint-key",
    )
    .unwrap();
    let endpoint = serde_json::json!({
        "base_url": "https://api.deepseek.com/anthropic",
        "models": ["deepseek-v4-pro", "deepseek-flash"],
        "default_model": "deepseek-v4-pro",
        "small_model": "deepseek-flash",
        "preset": "deepseek",
    });
    sqlx::query(
        "INSERT INTO accounts (id, provider, display_name, auth_mode, status, config_dir, created_at, endpoint)
         VALUES (?1, 'claude', 'DeepSeek', 'endpoint', 'ok', ?2, '0', ?3)",
    )
    .bind(&id)
    .bind(dir.display().to_string())
    .bind(endpoint.to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn an_endpoint_account_lists_its_own_models_without_asking_the_cli() {
    let st = hub().await;
    let dir = tempfile::tempdir().unwrap();
    let id = endpoint_account(&st, dir.path()).await;
    let v = claude_for(&st, "ws", "local", None, Some(&id), false).await;
    assert_eq!(v["source"], "endpoint");
    assert_eq!(v["endpoint"], "api.deepseek.com");
    assert_eq!(v["account"], id.as_str());
    let ids: Vec<&str> = v["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["", "deepseek-v4-pro", "deepseek-flash"]);
    assert!(!v.to_string().contains("sk-sekret"), "密钥不能出现在回复里");

    let remote = claude_for(&st, "ws", "gpu-box", None, Some(&id), false).await;
    assert_eq!(remote["source"], "endpoint", "{remote}");
    assert_eq!(remote["node"], "gpu-box");
    assert_eq!(
        remote["models"], v["models"],
        "远端也只列接口的型号，不为此开隧道"
    );
}

#[tokio::test]
async fn an_endpoint_account_is_never_picked_by_auto_balance() {
    let st = hub().await;
    let dir = tempfile::tempdir().unwrap();
    let id = endpoint_account(&st, dir.path()).await;
    let auto = crate::accounts::resolve(&st, "claude", "local", Some("auto"), None, None, &[])
        .await
        .unwrap();
    assert_ne!(auto.map(|c| c.id), Some(id.clone()));
    let picked = crate::accounts::resolve(&st, "claude", "local", Some(&id), None, None, &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        picked.token_file.as_ref().map(|(k, _)| k.as_str()),
        Some("ANTHROPIC_AUTH_TOKEN")
    );
    assert_eq!(
        picked.token_file.map(|(_, p)| p),
        Some(dir.path().join(crate::accounts::KEY_FILE))
    );
    assert!(picked.endpoint.is_some());
    assert!(!picked.proxy);
    let remote = crate::accounts::resolve(&st, "claude", "gpu-box", Some(&id), None, None, &[])
        .await
        .unwrap()
        .unwrap();
    assert!(remote.proxy, "远端经凭据代理，真密钥留在本机");
    assert!(remote.token_file.is_none() && remote.env.is_none());
    assert!(remote.endpoint.is_some());
}
