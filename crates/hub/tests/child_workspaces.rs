mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use blazar_hub::{AppState, HubConfig, build_state_with_services, services::Services};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn fixture() -> (tempfile::TempDir, Arc<AppState>, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
    blazar_hub::ensure_private_dir(dir.path()).unwrap();
    let state = build_state_with_services(
        &HubConfig {
            db_path: dir.path().join("hub.sqlite"),
            bind: ([127, 0, 0, 1], 0).into(),
            mesh_via: "local".into(),
            mesh_container: None,
            mesh_discovery: true,
            engine_dir: None,
        },
        Services::Isolated,
    )
    .await
    .unwrap();
    let router = common::authenticated_router(state.clone());
    (dir, state, router)
}

async fn call(app: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Content-Type", "application/json")
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn workspace(app: &axum::Router, path: &str, name: &str) -> String {
    let (status, body) = call(
        app,
        "POST",
        "/api/workspaces",
        json!({"node":"local","path":path,"name":name}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_child_cannot_take_over_another_workspace() {
    let (_dir, state, app) = fixture().await;
    let parent = workspace(&app, "/srv/parent", "主控").await;
    let other = workspace(&app, "/srv/other", "别的").await;

    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/workspaces/{parent}/children"),
        json!({"node":"local","path":"/srv/other/"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (name, owner): (String, Option<String>) =
        sqlx::query_as("SELECT name, parent_id FROM workspaces WHERE id = ?1")
            .bind(&other)
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!((name.as_str(), owner), ("别的", None));

    let (status, child) = call(
        &app,
        "POST",
        &format!("/api/workspaces/{parent}/children"),
        json!({"node":"local","path":"/srv/child"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{child}");
    assert_eq!(child["parent"], parent.as_str());
    assert_eq!(child["name"], "child");

    let (status, again) = call(
        &app,
        "POST",
        &format!("/api/workspaces/{parent}/children"),
        json!({"node":"local","path":"/srv/child"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["id"], child["id"]);
}

#[tokio::test]
async fn glance_reports_the_newest_prompt_and_reply_of_a_workspace() {
    let (_dir, state, app) = fixture().await;
    let ws = workspace(&app, "/srv/glance", "看一眼").await;
    let pool = state.db.pool();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at)
         VALUES ('s1', ?1, 'claude_cli', 'done', '2026-10-18T00:00:00Z'),
                ('s2', ?1, 'claude_cli', 'running', '2026-10-18T01:00:00Z')",
    )
    .bind(&ws)
    .execute(pool)
    .await
    .unwrap();
    for (sid, seq, ts, payload) in [
        (
            "s1",
            1,
            "2026-10-18T00:00:01Z",
            r#"{"type":"user_message","text":"第一轮"}"#,
        ),
        (
            "s1",
            2,
            "2026-10-18T00:00:02Z",
            r#"{"type":"assistant_message","text":"做完了"}"#,
        ),
        (
            "s2",
            1,
            "2026-10-18T01:00:01Z",
            r#"{"type":"user_message","text":"第二轮"}"#,
        ),
    ] {
        sqlx::query("INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, ?2, ?3, ?4)")
            .bind(sid)
            .bind(seq)
            .bind(ts)
            .bind(payload)
            .execute(pool)
            .await
            .unwrap();
    }
    let (status, glance) = call(
        &app,
        "GET",
        &format!("/api/workspaces/{ws}/glance"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(glance["prompt"], "第二轮");
    assert_eq!(glance["reply"], "做完了");
    assert_eq!(glance["failed"], false);
}

#[tokio::test]
async fn glance_reads_back_past_a_long_run_to_its_prompt() {
    let (_dir, state, app) = fixture().await;
    let ws = workspace(&app, "/srv/long", "长跑").await;
    let pool = state.db.pool();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at)
         VALUES ('old', ?1, 'claude_cli', 'done', '2026-10-18T00:00:00Z'),
                ('new', ?1, 'claude_cli', 'running', '2026-10-18T01:00:00Z')",
    )
    .bind(&ws)
    .execute(pool)
    .await
    .unwrap();
    let mut events = vec![
        (
            "old",
            1,
            r#"{"type":"user_message","text":"旧的一轮"}"#.to_owned(),
        ),
        (
            "new",
            1,
            r#"{"type":"user_message","text":"新的一轮"}"#.to_owned(),
        ),
    ];
    for i in 1..=40 {
        events.push((
            "new",
            2 * i,
            format!(r#"{{"type":"tool_use","id":"t{i}","name":"Bash","input":{{}}}}"#),
        ));
        events.push((
            "new",
            2 * i + 1,
            format!(r#"{{"type":"assistant_message","text":"进度 {i}"}}"#),
        ));
    }
    for (sid, seq, payload) in events {
        sqlx::query("INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, ?2, ?3, ?4)")
            .bind(sid)
            .bind(seq)
            .bind(format!("2026-10-18T01:{:02}:{:02}Z", seq / 60, seq % 60))
            .bind(payload)
            .execute(pool)
            .await
            .unwrap();
    }
    let (status, glance) = call(
        &app,
        "GET",
        &format!("/api/workspaces/{ws}/glance"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(glance["prompt"], "新的一轮");
    assert_eq!(glance["reply"], "进度 40");
}

#[tokio::test]
async fn workspaces_and_nodes_are_listed_on_their_own() {
    let (_dir, _state, app) = fixture().await;
    let id = workspace(&app, "/srv/listed", "列出来").await;
    let (status, workspaces) = call(&app, "GET", "/api/workspaces", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let workspaces = workspaces.as_array().unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["id"], id.as_str());
    let (status, nodes) = call(&app, "GET", "/api/nodes", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        nodes
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == "local")
    );
}
