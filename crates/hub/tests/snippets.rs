mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use blazar_hub::{AppState, auth, mesh::MeshCtx, services::Services};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn app() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    let mut mesh = MeshCtx::new(Some(dir.path()), dir.path().join("mesh-staging"));
    mesh.layout.root = dir.path().join("mesh");
    mesh.bundle = None;
    let state = AppState::with_services(db, "local".into(), None, mesh, Services::Isolated);
    assert!(
        state
            .auth
            .set(auth::Session::new(&dir.path().join("hub.sqlite"), None).unwrap())
            .is_ok()
    );
    (dir, common::authenticated_router(state))
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<String>,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(path);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.unwrap_or_default())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (status, body) = request(app, method, path, body.map(|body| body.to_string())).await;
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn snippet_name_conflicts_preserve_existing_records() {
    let (_dir, app) = app().await;
    let original = "  original\n'quoted' $HOME 中文\n";
    let (status, first) = call(
        &app,
        "POST",
        "/api/snippets",
        Some(json!({"name": " first ", "body": original})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, second) = call(
        &app,
        "POST",
        "/api/snippets",
        Some(json!({"name": "second", "body": "second body"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let second_path = format!("/api/snippets/{}", second["id"].as_str().unwrap());
    let (status, before) = call(&app, "GET", "/api/snippets", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = before.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let first_row = rows.iter().find(|row| row["id"] == first["id"]).unwrap();
    assert_eq!(first_row["name"], "first");
    assert_eq!(first_row["body"], original);
    for (method, path) in [("POST", "/api/snippets"), ("PUT", second_path.as_str())] {
        let (status, error) = call(
            &app,
            method,
            path,
            Some(json!({"name": " first ", "body": "replacement"})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(!error["error"].as_str().unwrap().is_empty());
        let (status, after) = call(&app, "GET", "/api/snippets", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(after, before);
    }
    let replacement = "\nnew body — keep its whitespace\n  ";
    let (status, result) = call(
        &app,
        "PUT",
        &second_path,
        Some(json!({"name": "renamed", "body": replacement})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result, json!({"ok": true}));
    let (status, after) = call(&app, "GET", "/api/snippets", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = after.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().find(|row| row["id"] == first["id"]).unwrap(),
        first_row
    );
    let renamed = rows.iter().find(|row| row["id"] == second["id"]).unwrap();
    assert_eq!(renamed["name"], "renamed");
    assert_eq!(renamed["body"], replacement);
}

#[tokio::test]
async fn snippet_http_validation_keeps_character_and_byte_boundaries() {
    let (_dir, app) = app().await;
    let name = "界".repeat(40);
    let body = format!("{}ab", "界".repeat(6666));
    let (status, created) = call(
        &app,
        "POST",
        "/api/snippets",
        Some(json!({"name": name, "body": body})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let path = format!("/api/snippets/{}", created["id"].as_str().unwrap());
    let (status, before) = call(&app, "GET", "/api/snippets", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before.as_array().unwrap().len(), 1);
    assert_eq!(before[0]["name"], name);
    assert_eq!(before[0]["body"], body);
    let invalid = [
        (
            r#"{"name":"missing-body"}"#.to_owned(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            r#"{"name":"wrong-type","body":7}"#.to_owned(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        ("{broken".to_owned(), StatusCode::BAD_REQUEST),
        (
            json!({"name": "界".repeat(41), "body": "valid"}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"name": "valid", "body": format!("{body}c")}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"name": "valid", "body": " \n\t "}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (body, expected) in invalid {
        for (method, uri) in [("POST", "/api/snippets"), ("PUT", path.as_str())] {
            let (status, error) = request(&app, method, uri, Some(body.clone())).await;
            assert_eq!(status, expected, "{method}: {body}");
            assert!(!error.is_empty());
            let (status, after) = call(&app, "GET", "/api/snippets", None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(after, before);
        }
    }
}
