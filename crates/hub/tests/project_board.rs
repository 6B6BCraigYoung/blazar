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
    session: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if body.is_some() {
        b = b.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(sid) = session {
        b = b.header("x-blazar-session", sid);
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

#[tokio::test]
async fn the_project_board_shows_each_workspace_with_its_latest_progress() {
    let (st, tmp) = state().await;
    let app = common::authenticated_router(st.clone());
    let root = tmp.path().join("code");
    let (_, repo) = call(
        &app,
        "POST",
        "/api/workspaces",
        None,
        Some(json!({ "node": "local", "path": dir(&root, "cad-repo"), "project": "demo" })),
    )
    .await;
    let (_, lane) = call(
        &app,
        "POST",
        "/api/workspaces",
        None,
        Some(json!({ "node": "local", "path": dir(&root, "cad-repo/drawings") })),
    )
    .await;
    assert_eq!(lane["parent"], repo["id"]);
    call(
        &app,
        "POST",
        "/api/workspaces",
        None,
        Some(json!({ "node": "local", "path": dir(&root, "elsewhere"), "project": "other" })),
    )
    .await;
    let lane_id = lane["id"].as_str().unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, title, summary)
         VALUES ('s1', ?1, 'claude', 'done', '2026-11-15T10:00:00Z', '转图纸', '转好了前五张')",
    )
    .bind(lane_id)
    .execute(st.db.pool())
    .await
    .unwrap();
    let (code, _) = call(
        &app,
        "POST",
        "/api/reports",
        Some("s1"),
        Some(json!({ "level": "info", "text": "转好一批", "done": 5, "total": 30, "unit": "件" })),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    call(
        &app,
        "POST",
        "/api/reports",
        Some("s1"),
        Some(json!({ "level": "info", "text": "图层名对上了" })),
    )
    .await;
    sqlx::query(
        "INSERT INTO inbox (id, kind, title, workspace_id, session_id, created_at)
         VALUES ('q1', 'question', '要不要合并图层', ?1, 's1', '2026-11-15T10:05:00Z')",
    )
    .bind(lane_id)
    .execute(st.db.pool())
    .await
    .unwrap();

    let (code, board) = call(&app, "GET", "/api/projects/board?name=demo", None, None).await;
    assert_eq!(code, StatusCode::OK);
    let rows = board["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{board}");
    let row = rows.iter().find(|r| r["id"] == lane["id"]).unwrap();
    assert_eq!(row["parent"], repo["id"]);
    assert_eq!(row["doing"], "转图纸");
    assert_eq!(row["latest"], "图层名对上了");
    assert_eq!(row["waiting"], 1);
    assert_eq!(
        (
            &row["progress"]["done"],
            &row["progress"]["total"],
            &row["progress"]["unit"]
        ),
        (&json!(5), &json!(30), &json!("件"))
    );
    let recent = board["recent"].as_array().unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0]["title"], "图层名对上了");
    assert!(recent[0]["progress"].is_null());
    assert_eq!(recent[1]["progress"]["done"], 5);

    let (_, list) = call(&app, "GET", "/api/workspaces", None, None).await;
    let shown = list
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == lane["id"])
        .unwrap();
    assert_eq!(shown["progress"]["title"], "转好一批");

    let (_, none) = call(&app, "GET", "/api/projects/board?name=nobody", None, None).await;
    assert_eq!(none["rows"], json!([]));
}
