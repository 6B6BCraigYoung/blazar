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
