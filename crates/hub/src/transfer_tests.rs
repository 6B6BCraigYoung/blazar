use super::*;
use crate::state::AppState;

struct Hub {
    st: Shared,
    root: tempfile::TempDir,
}

async fn hub() -> Hub {
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let st = AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
        crate::services::Services::Isolated,
    );
    sql(
        &st,
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES ('n-local', 'local', 'local', 'online', '0')",
    )
    .await;
    Hub { st, root }
}

impl Hub {
    fn opts(&self, local_node: Option<&str>) -> Options {
        Options {
            local_node: local_node.map(str::to_owned),
            bot_home: Some(self.root.path().join("bots")),
        }
    }

    async fn export(&self) -> ExportFile {
        let mut conn = self.st.db.pool().acquire().await.unwrap();
        export_from(&mut conn).await.unwrap()
    }

    async fn import(&self, file: &ExportFile, local_node: Option<&str>) -> ImportReport {
        import(&self.st, file, &self.opts(local_node), false)
            .await
            .unwrap()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.st.db.pool())
            .await
            .unwrap()
    }
}

async fn sql(st: &Shared, q: &str) {
    sqlx::query(q).execute(st.db.pool()).await.unwrap();
}

async fn seed(h: &Hub) {
    let st = &h.st;
    for q in [
        r#"INSERT INTO kv_settings (key, value, updated_at) VALUES
           ('prefs', '{"git":{"branch_prefix":"team/"}}', '0'),
           ('skillsmp_api_key', '"sk-market-SECRET"', '0'),
           ('office.lark', '{"app_secret":"lark-SECRET"}', '0')"#,
        "INSERT INTO settings (key, value, updated_at) VALUES ('mesh.identity', 'id-SECRET', '0')",
        r#"INSERT INTO nodes (id, name, transport, endpoint, network, labels, capabilities, load,
                              status, os, role, created_at)
           VALUES ('n-gpu', 'gpu-1', 'ssh', '10.0.0.2', 'mesh', '["gpu"]', '{"x":1}', '{"cpu":3}',
                   'online', 'linux', 'dev', '0')"#,
        r#"INSERT INTO nodes (id, name, transport, endpoint, network, labels, status, created_at)
           VALUES ('n-mesh', 'peer-2', 'ssh', '10.126.0.7', 'easytier',
                   '{"cost":"p2p","latency_ms":12.5,"loss_rate":0.01,"nat_type":"Symmetric"}',
                   'online', '0')"#,
        "INSERT INTO projects (id, name, created_at) VALUES ('p1', 'blazar', '0')",
        "INSERT INTO workspaces (id, project_id, node_id, name, path, branch, base_commit,
                                 target_branch, repo_root, created_at)
         VALUES ('w-gpu', 'p1', 'n-gpu', '训练', '/data/repo', 'main', 'abc', 'main', '/data/repo', '0')",
        "INSERT INTO workspaces (id, node_id, name, path, parent_id, created_at)
         VALUES ('w-child', 'n-gpu', 'child', '/data/repo-child', 'w-gpu', '0')",
        "INSERT INTO workspaces (id, node_id, name, path, origin, created_at)
         VALUES ('w-task', 'n-gpu', 'task', '/data/task', 'task', '0')",
        "INSERT INTO skills (id, name, description, created_at, updated_at)
         VALUES ('s1', 'deploy', '发布', '0', '0')",
        "INSERT INTO skill_files (skill_id, path, content) VALUES
         ('s1', 'SKILL.md', '---\ndescription: 发布\n---\n步骤'), ('s1', 'scripts/run.sh', 'echo hi')",
        r#"INSERT INTO mcp_servers (id, name, transport, command, args, env, created_at, updated_at)
           VALUES ('m1', 'github', 'stdio', 'gh-mcp', '["--stdio"]', '{"GITHUB_TOKEN":"ghp-SECRET"}', '0', '0')"#,
        r#"INSERT INTO mcp_servers (id, name, transport, url, headers, created_at, updated_at)
           VALUES ('m2', 'search', 'http', 'https://example.com/mcp', '{"Authorization":"Bearer hdr-SECRET"}', '0', '0')"#,
        r#"INSERT INTO agents (id, name, runtime, description, custom_env, account, starters,
                               custom_args, max_concurrent, created_at, updated_at)
           VALUES ('a1', 'reviewer', 'claude', '审代码', '{"OPENAI_API_KEY":"env-SECRET"}', 'acc-1',
                   '[{"label":"看看","prompt":"看一下 diff"}]', '["--debug"]', 2, '0', '0')"#,
        "INSERT INTO agents (id, name, runtime, archived_at, created_at, updated_at)
         VALUES ('a-old', 'retired', 'claude', '1', '0', '0')",
        "INSERT INTO agent_skills (agent_id, skill_id) VALUES ('a1', 's1')",
        "INSERT INTO agent_mcp (agent_id, server_id) VALUES ('a1', 'm1')",
        "INSERT INTO snippets (id, name, body, created_at, updated_at) VALUES ('sn', '提交', 'commit it', '0', '0')",
        "INSERT INTO hooks (id, event, command, target, enabled, created_at)
         VALUES ('h1', 'turn_start', 'echo start', 'hub', 1, '0')",
        "INSERT INTO approval_rules (id, tool, pattern, enabled, hits, created_at)
         VALUES ('r1', 'Bash', 'cargo test', 1, 9, '0')",
        "INSERT INTO approval_rules (id, tool, pattern, workspace_id, enabled, hits, created_at)
         VALUES ('r2', 'Read', 'src/', 'w-gpu', 0, 3, '0')",
        "INSERT INTO autopilots (id, name, instructions, workspace_id, agent_profile, status, cron,
                                 timezone, webhook_token, fail_streak, next_run_at, last_run_at,
                                 created_at, updated_at)
         VALUES ('ap1', 'nightly', '跑测试', 'w-gpu', 'a1', 'active', '0 3 * * *', 'Asia/Shanghai',
                 'whk-SECRET', 2, 99, '1', '0', '0')",
        "INSERT INTO accounts (id, provider, display_name, auth_mode, status, created_at)
         VALUES ('acc-1', 'claude', 'me', 'cli', 'ok', '0')",
    ] {
        sql(st, q).await;
    }
    let lead = crate::bots::create(
        st,
        crate::bots::NewBot {
            handle: "lead".into(),
            agent_id: "a1".into(),
            role: Some("lead".into()),
            priority: Some("P0".into()),
            runtimes: Some(vec![json!("claude")]),
            home_path: Some(h.root.path().join("home-lead").display().to_string()),
            ..crate::bots::NewBot::default()
        },
    )
    .await
    .unwrap();
    let temp = crate::bots::create(
        st,
        crate::bots::NewBot {
            handle: "tmp-1".into(),
            agent_id: "a1".into(),
            lifetime: Some("temporary".into()),
            home_path: Some(h.root.path().join("home-tmp").display().to_string()),
            ..crate::bots::NewBot::default()
        },
    )
    .await
    .unwrap();
    for (scope, name) in [
        ("shared".to_owned(), "conventions"),
        (format!("bot:{}", lead.id), "lead-notes"),
        (format!("bot:{}", temp.id), "scratch"),
    ] {
        sqlx::query(
            "INSERT INTO memories (id, scope, name, description, type, body, updated_by, updated_at)
             VALUES (?1, ?2, ?3, '说明', 'project', '正文', 'user', '0')",
        )
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(scope)
        .bind(name)
        .execute(st.db.pool())
        .await
        .unwrap();
    }
}

async fn seed_local(h: &Hub) {
    for q in [
        "INSERT INTO workspaces (id, node_id, name, path, created_at)
         VALUES ('w-local', 'n-local', '笔记本项目', '/Users/me/proj', '0')",
        "INSERT INTO approval_rules (id, tool, pattern, workspace_id, created_at)
         VALUES ('r-local', 'Edit', 'docs/', 'w-local', '0')",
        "INSERT INTO autopilots (id, name, instructions, workspace_id, created_at, updated_at)
         VALUES ('ap-local', 'tidy', '整理', 'w-local', '0', '0')",
    ] {
        sql(&h.st, q).await;
    }
}

fn without_time(mut f: ExportFile) -> ExportFile {
    f.exported_at = String::new();
    f
}

#[tokio::test]
async fn export_leaves_out_secrets_and_runtime_state() {
    let a = hub().await;
    seed(&a).await;
    let file = a.export().await;
    let text = serde_json::to_string_pretty(&file).unwrap();
    assert!(!text.contains("SECRET"), "{text}");
    for gone in [
        "acc-1",
        "skillsmp",
        "office",
        "mesh.identity",
        "whk-",
        "retired",
        "repo-child",
        "/data/task",
        "home-lead",
        "\"local\"",
        "scratch",
        "tmp-1",
        "fail_streak",
        "next_run_at",
        "\"id\"",
        "created_at",
        "latency_ms",
        "Symmetric",
    ] {
        assert!(!text.contains(gone), "{gone} leaked:\n{text}");
    }
    assert_eq!(file.format, FORMAT);
    assert_eq!(file.version, VERSION);
    assert_eq!(file.prefs, Some(json!({"git": {"branch_prefix": "team/"}})));
    assert_eq!(
        file.mcp_servers[0].env,
        BTreeMap::from([("GITHUB_TOKEN".into(), String::new())])
    );
    assert_eq!(
        file.mcp_servers[1].headers,
        BTreeMap::from([("Authorization".into(), String::new())])
    );
    let reviewer = &file.agents[0];
    assert_eq!(
        reviewer.custom_env.get("OPENAI_API_KEY"),
        Some(&String::new())
    );
    assert_eq!(reviewer.skills, vec!["deploy"]);
    assert_eq!(reviewer.mcp_servers, vec!["github"]);
    assert_eq!(file.nodes.len(), 2);
    assert_eq!(file.nodes[0].labels, json!(["gpu"]));
    assert_eq!(file.nodes[1].name, "peer-2");
    assert_eq!(file.nodes[1].labels, json!([]));
    assert_eq!(file.nodes[1].network.as_deref(), Some("easytier"));
    assert_eq!(file.workspaces.len(), 1);
    assert_eq!(file.autopilots[0].agent.as_deref(), Some("reviewer"));
    assert_eq!(
        file.autopilots[0].workspace,
        Some(WorkspaceRef {
            node: "gpu-1".into(),
            path: "/data/repo".into()
        })
    );
    assert_eq!(file.bots.len(), 1);
    assert_eq!(file.bots[0].agent, "reviewer");
    let scopes: Vec<_> = file.memories.iter().map(|m| m.scope.as_str()).collect();
    assert_eq!(scopes, vec!["bot:lead", "shared"]);
}

#[tokio::test]
async fn export_import_export_round_trips() {
    let a = hub().await;
    seed(&a).await;
    let first = a.export().await;
    let b = hub().await;
    let report = b.import(&first, None).await;
    assert!(report.applied);
    assert!(
        report.items.iter().all(|i| i.imported),
        "{:#?}",
        report.items
    );
    let second = b.export().await;
    assert_eq!(without_time(first), without_time(second));
}

#[tokio::test]
async fn imported_things_that_run_by_themselves_start_stopped() {
    let a = hub().await;
    seed(&a).await;
    let b = hub().await;
    let report = b.import(&a.export().await, None).await;
    let row = sqlx::query(
        "SELECT status, paused_reason, webhook_token, next_run_at, fail_streak FROM autopilots",
    )
    .fetch_one(b.st.db.pool())
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "paused");
    assert_eq!(row.get::<String, _>("paused_reason"), PAUSED_REASON);
    assert_eq!(row.get::<Option<String>, _>("webhook_token"), None);
    assert_eq!(row.get::<Option<i64>, _>("next_run_at"), None);
    assert_eq!(row.get::<i64, _>("fail_streak"), 0);
    assert_eq!(
        b.count("SELECT COUNT(*) FROM hooks WHERE enabled = 0")
            .await,
        1
    );
    assert_eq!(b.count("SELECT SUM(hits) FROM approval_rules").await, 0);
    assert_eq!(
        b.count("SELECT COUNT(*) FROM agents WHERE account IS NOT NULL")
            .await,
        0
    );
    assert_eq!(
        b.count("SELECT COUNT(*) FROM accounts WHERE id = 'acc-1'")
            .await,
        0
    );
    assert_eq!(
        b.count("SELECT COUNT(*) FROM kv_settings WHERE key <> 'prefs'")
            .await,
        0
    );
    assert_eq!(
        b.count("SELECT COUNT(*) FROM bot_threads t JOIN bots b ON b.id = t.bot_id WHERE b.handle = 'lead' AND t.kind = 'main'")
            .await,
        1
    );
    assert!(b.root.path().join("bots/lead").is_dir());
    assert!(report.notes.iter().any(|n| n.contains("原 hub")));
    let attention: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.action == ImportAction::Attention)
        .map(|i| (i.section.as_str(), i.key.as_str()))
        .collect();
    assert_eq!(
        attention,
        vec![
            ("mcp_servers", "github"),
            ("mcp_servers", "search"),
            ("agents", "reviewer"),
        ]
    );
}

#[tokio::test]
async fn importing_twice_skips_everything_the_second_time() {
    let a = hub().await;
    seed(&a).await;
    seed_local(&a).await;
    let file = a.export().await;
    let b = hub().await;
    sql(
        &b.st,
        "INSERT INTO nodes (id, name, transport, created_at) VALUES ('n-mac', 'mac', 'ssh', '0')",
    )
    .await;
    let first = b.import(&file, Some("mac")).await;
    assert!(first.counts.create > 0);
    let second = b.import(&file, Some("mac")).await;
    assert_eq!(second.counts.create, 0, "{:#?}", second.items);
    assert_eq!(second.counts.attention, 0, "{:#?}", second.items);
    assert_eq!(second.counts.skip, second.items.len());
}

#[tokio::test]
async fn local_workspaces_need_a_node_on_the_new_hub() {
    let a = hub().await;
    seed(&a).await;
    seed_local(&a).await;
    let file = a.export().await;
    assert!(file.workspaces.iter().any(|w| w.node == "local"));

    let b = hub().await;
    let report = b.import(&file, None).await;
    for (section, key) in [
        ("workspaces", "local:/Users/me/proj"),
        ("approval_rules", "Edit docs/ @ local:/Users/me/proj"),
        ("autopilots", "tidy"),
    ] {
        let item = report
            .items
            .iter()
            .find(|i| i.section == section && i.key == key)
            .unwrap_or_else(|| panic!("{section} {key} missing: {:#?}", report.items));
        assert_eq!(item.action, ImportAction::Attention);
        assert!(!item.imported);
    }
    assert_eq!(
        b.count("SELECT COUNT(*) FROM autopilots WHERE name = 'tidy'")
            .await,
        0
    );
    assert_eq!(
        b.count("SELECT COUNT(*) FROM approval_rules WHERE tool = 'Edit'")
            .await,
        0
    );

    let c = hub().await;
    c.import(&file, Some("gpu-1")).await;
    let ws: String = sqlx::query_scalar(
        "SELECT w.id FROM workspaces w JOIN nodes n ON n.id = w.node_id
         WHERE n.name = 'gpu-1' AND w.path = '/Users/me/proj'",
    )
    .fetch_one(c.st.db.pool())
    .await
    .unwrap();
    assert_eq!(
        c.count(&format!(
            "SELECT COUNT(*) FROM autopilots WHERE name = 'tidy' AND workspace_id = '{ws}'"
        ))
        .await,
        1
    );
    assert_eq!(
        c.count(&format!(
            "SELECT COUNT(*) FROM approval_rules WHERE tool = 'Edit' AND workspace_id = '{ws}'"
        ))
        .await,
        1
    );
}

#[tokio::test]
async fn missing_references_need_attention() {
    let a = hub().await;
    seed(&a).await;
    let mut file = a.export().await;
    file.autopilots[0].workspace = Some(WorkspaceRef {
        node: "gpu-1".into(),
        path: "/data/gone".into(),
    });
    file.agents[0].skills.push("ghost".into());
    file.bots[0].agent = "nobody".into();
    let b = hub().await;
    let report = import(&b.st, &file, &b.opts(None), true).await.unwrap();
    let find = |section: &str| {
        report
            .items
            .iter()
            .find(|i| i.section == section)
            .unwrap()
            .clone()
    };
    let ap = find("autopilots");
    assert_eq!((ap.action, ap.imported), (ImportAction::Attention, false));
    assert!(ap.detail.contains("/data/gone"), "{}", ap.detail);
    let agent = find("agents");
    assert_eq!(
        (agent.action, agent.imported),
        (ImportAction::Attention, true)
    );
    assert!(agent.detail.contains("ghost"));
    let bot = find("bots");
    assert_eq!((bot.action, bot.imported), (ImportAction::Attention, false));
    let memory = report
        .items
        .iter()
        .find(|i| i.key == "bot:lead/lead-notes")
        .unwrap();
    assert_eq!(
        (memory.action, memory.imported),
        (ImportAction::Attention, false)
    );
}

#[tokio::test]
async fn dry_run_writes_nothing_and_matches_the_real_import() {
    let a = hub().await;
    seed(&a).await;
    let file = a.export().await;
    let b = hub().await;
    let dry = import(&b.st, &file, &b.opts(None), true).await.unwrap();
    assert!(dry.dry_run && !dry.applied);
    assert_eq!(b.count("SELECT COUNT(*) FROM agents").await, 0);
    assert_eq!(b.count("SELECT COUNT(*) FROM nodes").await, 1);
    assert!(!b.root.path().join("bots").exists());
    let real = b.import(&file, None).await;
    assert_eq!(dry.items, real.items);
    assert_eq!(dry.counts, real.counts);
}

#[tokio::test]
async fn a_failure_halfway_rolls_everything_back() {
    let a = hub().await;
    seed(&a).await;
    let file = a.export().await;
    let b = hub().await;
    sql(
        &b.st,
        "CREATE TRIGGER no_memories BEFORE INSERT ON memories
         BEGIN SELECT RAISE(ABORT, 'disk on fire'); END",
    )
    .await;
    let err = import(&b.st, &file, &b.opts(None), false)
        .await
        .unwrap_err();
    assert!(
        err.contains("disk on fire") && err.contains("回滚"),
        "{err}"
    );
    for table in [
        "agents",
        "skills",
        "mcp_servers",
        "autopilots",
        "approval_rules",
        "bots",
        "workspaces",
        "projects",
        "kv_settings",
    ] {
        assert_eq!(
            b.count(&format!("SELECT COUNT(*) FROM {table}")).await,
            0,
            "{table}"
        );
    }
    assert_eq!(b.count("SELECT COUNT(*) FROM nodes").await, 1);
}

#[tokio::test]
async fn unknown_versions_and_formats_are_refused() {
    let ok = serde_json::to_vec(&json!({
        "format": FORMAT, "version": 1, "exported_at": "x", "hub_version": "0",
    }))
    .unwrap();
    assert!(parse(&ok).is_ok());
    let newer = serde_json::to_vec(&json!({
        "format": FORMAT, "version": 2, "exported_at": "x", "hub_version": "9",
    }))
    .unwrap();
    assert!(parse(&newer).unwrap_err().contains('2'));
    let other = serde_json::to_vec(&json!({ "format": "something", "version": 1 })).unwrap();
    assert!(parse(&other).is_err());
    assert!(parse(b"not json").is_err());
}

#[tokio::test]
async fn bots_cannot_export_or_import() {
    let a = hub().await;
    let mut headers = HeaderMap::new();
    headers.insert(
        crate::bots::CALLER_HEADER,
        json!({"bot": "b1", "handle": "lead", "role": "lead"})
            .to_string()
            .parse()
            .unwrap(),
    );
    let r = export_handler(State(a.st.clone()), headers.clone()).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = import_handler(
        State(a.st.clone()),
        headers,
        Query(ImportQuery {
            dry_run: Some(true),
            local_node: None,
        }),
        Bytes::from_static(b"{}"),
    )
    .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    let r = export_handler(State(a.st.clone()), HeaderMap::new()).await;
    assert_eq!(r.status(), StatusCode::OK);
    let disposition = r.headers()[header::CONTENT_DISPOSITION].to_str().unwrap();
    assert!(disposition.starts_with("attachment; filename=\"blazar-export-"));
    assert!(disposition.ends_with(".json\""));
}
