mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use blazar_hub::{AppState, HubConfig, build_state_with_services, services::Services};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn state() -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    blazar_hub::ensure_private_dir(dir.path()).unwrap();
    let cfg = HubConfig {
        db_path: dir.path().join("blazar.sqlite"),
        bind: ([127, 0, 0, 1], 0).into(),
        mesh_via: "local".into(),
        mesh_container: None,
        mesh_discovery: true,
        engine_dir: None,
    };
    let st = build_state_with_services(&cfg, Services::Isolated)
        .await
        .unwrap();
    (st, dir)
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if body.is_some() {
        b = b.header(header::CONTENT_TYPE, "application/json");
    }
    let mut req = b
        .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
        .unwrap();
    let remote: SocketAddr = "127.0.0.1:40000".parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(remote));
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn dir(root: &std::path::Path, rel: &str) -> String {
    let p = root.join(rel);
    std::fs::create_dir_all(&p).unwrap();
    p.display().to_string()
}

async fn make(app: &axum::Router, path: &str, project: Option<&str>) -> String {
    let (s, v) = call(
        app,
        "POST",
        "/api/workspaces",
        Some(json!({ "node": "local", "path": path, "project": project })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

async fn legacy(st: &AppState, path: &str, name: &str) -> String {
    let id = blazar_core_types::WorkspaceId::new().to_string();
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, created_at)
         VALUES (?1, (SELECT id FROM nodes WHERE name = 'local'), ?2, ?3, '0')",
    )
    .bind(&id)
    .bind(name)
    .bind(path)
    .execute(st.db.pool())
    .await
    .unwrap();
    id
}

async fn listed(app: &axum::Router, archived: bool) -> Vec<Value> {
    let path = if archived {
        "/api/workspaces?archived=true"
    } else {
        "/api/workspaces"
    };
    call(app, "GET", path, None)
        .await
        .1
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn find<'a>(list: &'a [Value], id: &str) -> Option<&'a Value> {
    list.iter().find(|w| w["id"] == id)
}

#[tokio::test]
async fn reparenting_carries_the_project_and_refuses_loops() {
    let (st, tmp) = state().await;
    let app = common::authenticated_router(st.clone());
    let top = make(&app, &dir(tmp.path(), "a/top"), Some("acme")).await;
    let other = make(&app, &dir(tmp.path(), "b/other"), None).await;
    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{other}"),
        Some(json!({ "parent": top })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let list = listed(&app, false).await;
    assert_eq!(find(&list, &other).unwrap()["parent"], top.as_str());
    assert_eq!(find(&list, &other).unwrap()["project"], "acme");

    let (s, v) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{top}"),
        Some(json!({ "parent": other })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");

    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{top}"),
        Some(json!({ "project": "zeta" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let list = listed(&app, false).await;
    assert_eq!(find(&list, &other).unwrap()["project"], "zeta");

    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{other}"),
        Some(json!({ "parent": "" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        find(&listed(&app, false).await, &other)
            .unwrap()
            .get("parent")
            .is_none()
    );
}

#[tokio::test]
async fn archiving_hides_a_family_and_keeps_its_history() {
    let (st, tmp) = state().await;
    let app = common::authenticated_router(st.clone());
    let top = make(&app, &dir(tmp.path(), "a/top"), Some("acme")).await;
    let kid = make(&app, &dir(tmp.path(), "a/top/kid"), None).await;
    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{kid}"),
        Some(json!({ "parent": top })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES ('s1', ?1, 'claude', 'done', '0')",
    )
    .bind(&kid)
    .execute(st.db.pool())
    .await
    .unwrap();

    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{top}"),
        Some(json!({ "archived": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let shown = listed(&app, false).await;
    assert!(find(&shown, &top).is_none() && find(&shown, &kid).is_none());
    let all = listed(&app, true).await;
    assert_eq!(find(&all, &kid).unwrap()["archived"], true);
    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE workspace_id = ?1")
        .bind(&kid)
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert_eq!(kept, 1);

    let (s, _) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{kid}"),
        Some(json!({ "archived": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let shown = listed(&app, false).await;
    assert!(find(&shown, &top).is_some() && find(&shown, &kid).is_some());

    sqlx::query("UPDATE sessions SET status = 'running' WHERE id = 's1'")
        .execute(st.db.pool())
        .await
        .unwrap();
    let (s, v) = call(
        &app,
        "PATCH",
        &format!("/api/workspaces/{top}"),
        Some(json!({ "archived": true })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
}

#[tokio::test]
async fn tidying_previews_first_and_applies_only_what_was_picked() {
    let (st, tmp) = state().await;
    let app = common::authenticated_router(st.clone());
    let repo = make(&app, &dir(tmp.path(), "share/cad-repo"), Some("acme")).await;
    let sub = legacy(&st, &dir(tmp.path(), "share/cad-repo/selfcad"), "selfcad").await;
    let gone = legacy(
        &st,
        &tmp.path().join("share/vanished").display().to_string(),
        "vanished",
    )
    .await;

    let (s, v) = call(&app, "GET", "/api/workspaces/tidy", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let keys: Vec<&str> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["key"].as_str())
        .collect();
    let nest = format!("nest:{sub}");
    assert!(keys.contains(&nest.as_str()), "{v}");
    assert!(keys.contains(&format!("archive:{gone}").as_str()), "{v}");

    let (s, v) = call(
        &app,
        "POST",
        "/api/workspaces/tidy",
        Some(json!({ "only": [nest] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["applied"], json!([nest]));
    let list = listed(&app, false).await;
    assert_eq!(find(&list, &sub).unwrap()["parent"], repo.as_str());
    assert_eq!(find(&list, &sub).unwrap()["project"], "acme");
    assert!(find(&list, &gone).is_some());

    let (_, v) = call(&app, "GET", "/api/workspaces/tidy", None).await;
    let keys: Vec<&str> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["key"].as_str())
        .collect();
    assert!(!keys.contains(&nest.as_str()), "{v}");
}

async fn create(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    call(app, "POST", "/api/workspaces", Some(body)).await
}

#[tokio::test]
async fn new_workspaces_fold_into_the_tree_they_sit_in() {
    let (st, tmp) = state().await;
    let app = common::authenticated_router(st.clone());
    let repo = make(&app, &dir(tmp.path(), "share/cad-repo"), Some("acme")).await;
    let path = dir(tmp.path(), "share/cad-repo/selfcad");
    let (s, v) = create(
        &app,
        json!({ "node": "local", "path": path, "name": "自产" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["parent"], repo.as_str());
    let kid = v["id"].as_str().unwrap().to_owned();
    assert_eq!(
        find(&listed(&app, false).await, &kid).unwrap()["project"],
        "acme"
    );

    let (_, v) = create(&app, json!({ "node": "local", "path": format!("{path}/") })).await;
    assert_eq!(v["reused"], true);
    assert_eq!(v["id"], kid.as_str());
    let list = listed(&app, false).await;
    assert_eq!(find(&list, &kid).unwrap()["project"], "acme");
    assert_eq!(find(&list, &kid).unwrap()["name"], "自产");

    let (_, v) = create(
        &app,
        json!({ "node": "local", "path": dir(tmp.path(), "share/cad-repo/other"), "project": "zeta" }),
    )
    .await;
    assert!(v["parent"].is_null(), "{v}");

    let (_, v) = create(
        &app,
        json!({ "node": "local", "path": dir(tmp.path(), "share/cad-repo/solo"), "top": true }),
    )
    .await;
    assert!(v["parent"].is_null(), "{v}");

    let shelf = make(&app, &dir(tmp.path(), "me/workspace"), None).await;
    let (_, v) = create(
        &app,
        json!({ "node": "local", "path": dir(tmp.path(), "me/workspace/proj") }),
    )
    .await;
    assert!(
        v["parent"].is_null(),
        "catch-all folder {shelf} must not swallow: {v}"
    );
}

#[tokio::test]
async fn agents_need_a_project_for_top_level_workspaces() {
    let (st, tmp) = state().await;
    let app = common::authenticated_router(st.clone());
    let repo = make(&app, &dir(tmp.path(), "share/cad-repo"), Some("acme")).await;
    let (s, v) = create(
        &app,
        json!({ "node": "local", "path": dir(tmp.path(), "fresh"), "agent": true }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let msg = v["error"]
        .as_str()
        .or_else(|| v["message"].as_str())
        .unwrap_or_default();
    assert!(msg.contains("project") && msg.contains("acme"), "{v}");

    let (s, _) = create(
        &app,
        json!({ "node": "local", "path": dir(tmp.path(), "fresh"), "agent": true, "project": "new-one" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = create(
        &app,
        json!({ "node": "local", "path": dir(tmp.path(), "share/cad-repo/lane"), "agent": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["parent"], repo.as_str());
    let (s, v) = create(
        &app,
        json!({ "node": "local", "path": dir(tmp.path(), "elsewhere"), "agent": true, "parent": repo }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["parent"], repo.as_str());
}

#[tokio::test]
async fn a_worktree_lane_is_an_isolated_checkout_under_its_parent() {
    let (st, tmp) = state().await;
    let app = common::authenticated_router(st.clone());
    let repo_dir = dir(tmp.path(), "share/repo");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-C", &repo_dir])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    std::fs::write(tmp.path().join("share/repo/a.txt"), "a").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "init",
    ]);
    let repo = make(&app, &repo_dir, Some("acme")).await;
    let lane = tmp.path().join("share/repo-lane").display().to_string();
    let (s, v) = create(
        &app,
        json!({ "node": "local", "path": lane, "parent": repo, "worktree": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["parent"], repo.as_str());
    assert!(std::path::Path::new(&lane).join("a.txt").exists());
}
