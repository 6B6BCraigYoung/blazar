use super::*;

#[tokio::test]
async fn invalid_environment_names_are_rejected_before_session_lookup() {
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let st = AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
        crate::services::Services::Isolated,
    );
    let request =
        serde_json::from_value(serde_json::json!({"text": "test", "env": {"A-B": "test-value"}}))
            .unwrap();
    let response = prompt(
        State(st.clone()),
        Path("missing-workspace".into()),
        Json(request),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

fn profile() -> crate::agents::Agent {
    serde_json::from_value(serde_json::json!({
        "name": "Demo", "runtime": "claude", "custom_args": ["--profile-option", "profile"],
        "model": "profile-model", "env": {"SHARED": "profile"}
    }))
    .unwrap()
}

fn capabilities() -> crate::library::Caps {
    crate::library::Caps {
        mcp: vec![blazar_runtime::McpServerSpec {
            name: "docs".into(),
            command: "fake-docs".into(),
            ..Default::default()
        }],
        env: Default::default(),
        skill_root: None,
        skills: Vec::new(),
    }
}

fn runtime_config() -> AgentConfigView {
    AgentConfigView {
        id: "runtime".into(),
        node: None,
        agent_id: "claude".into(),
        display_name: None,
        program_path: None,
        model: Some("runtime-model".into()),
        permission_mode: None,
        custom_args: vec!["--runtime-option".into(), "runtime".into()],
        custom_env: [("SHARED".into(), "runtime".into())].into(),
        max_concurrent: 2,
        enabled: true,
    }
}

#[test]
fn profile_keeps_built_in_office_mcp() {
    let mut spec = SessionSpec::new("/home/me/repo", "hello");
    spec.mcp_servers.push(blazar_runtime::McpServerSpec {
        name: "office".into(),
        command: "fake-office".into(),
        ..Default::default()
    });
    apply_profile(&mut spec, &profile(), capabilities());
    assert_eq!(
        spec.mcp_servers
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["office", "docs"]
    );
}

#[test]
fn empty_profile_capabilities_keep_built_in_office_mcp() {
    let mut spec = SessionSpec::new("/home/me/repo", "hello");
    spec.mcp_servers.push(blazar_runtime::McpServerSpec {
        name: "blazar-office".into(),
        command: "fake-office".into(),
        ..Default::default()
    });
    let mut caps = capabilities();
    caps.mcp.clear();
    apply_profile(&mut spec, &profile(), caps);
    assert_eq!(spec.mcp_servers.len(), 1);
    assert_eq!(spec.mcp_servers[0].name, "blazar-office");
}

#[test]
fn empty_runtime_args_keep_profile_args() {
    let mut spec = SessionSpec::new("/home/me/repo", "hello");
    apply_profile(&mut spec, &profile(), capabilities());
    let mut config = runtime_config();
    config.custom_args.clear();
    apply_runtime_config(&mut spec, &config);
    assert_eq!(spec.extra_args, ["--profile-option", "profile"]);
}

#[test]
fn runtime_defaults_preserve_profile_args_and_request_precedence() {
    let mut spec = SessionSpec::new("/home/me/repo", "hello");
    spec.env.insert("SHARED".into(), "request".into());
    apply_profile(&mut spec, &profile(), capabilities());
    apply_runtime_config(&mut spec, &runtime_config());
    assert_eq!(
        spec.extra_args,
        ["--runtime-option", "runtime", "--profile-option", "profile"]
    );
    assert_eq!(spec.model.as_deref(), Some("profile-model"));
    assert_eq!(spec.env["SHARED"], "request");
}
