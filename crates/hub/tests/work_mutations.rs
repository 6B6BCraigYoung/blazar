mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use blazar_hub::{AppState, HubConfig, build_state_with_services, services::Services};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn fixture() -> (tempfile::TempDir, Arc<AppState>, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
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
    sqlx::query("INSERT INTO tasks (id, number, title, created_at, updated_at) VALUES ('task', 1, 'original', '0', '0')")
        .execute(state.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO autopilots (id, name, instructions, cron, created_at, updated_at) VALUES ('auto', 'original', 'work', '* * * * *', '0', '0')")
        .execute(state.db.pool()).await.unwrap();
    let router = common::authenticated_router(state.clone());
    (dir, state, router)
}

async fn call(app: &axum::Router, method: &str, path: &str, body: Value) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn task_update_rolls_back_every_field_when_a_later_field_fails() {
    let (_dir, state, app) = fixture().await;
    let result = call(
        &app,
        "PUT",
        "/api/tasks/task",
        json!({"title":"changed","workspace_id":"missing"}),
    )
    .await;
    assert_eq!(result, StatusCode::INTERNAL_SERVER_ERROR);
    let row: (String, String) =
        sqlx::query_as("SELECT title, updated_at FROM tasks WHERE id='task'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(row, ("original".into(), "0".into()));
}

#[tokio::test]
async fn task_completion_is_atomic_with_its_timestamp() {
    let (_dir, state, app) = fixture().await;
    sqlx::query("CREATE TRIGGER reject_completion BEFORE UPDATE OF completed_at ON tasks BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(state.db.pool()).await.unwrap();
    assert_eq!(
        call(&app, "PUT", "/api/tasks/task", json!({"status":"done"})).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id='task'")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert_eq!(status, "todo");
}

#[tokio::test]
async fn automation_update_rolls_back_every_field_when_a_later_field_fails() {
    let (_dir, state, app) = fixture().await;
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/autopilots/auto",
            json!({"name":"changed","workspace_id":"missing"})
        )
        .await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let row: (String, String) =
        sqlx::query_as("SELECT name, updated_at FROM autopilots WHERE id='auto'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(row, ("original".into(), "0".into()));
}

#[tokio::test]
async fn automation_schedule_is_committed_with_its_settings() {
    let (_dir, state, app) = fixture().await;
    sqlx::query("CREATE TRIGGER reject_schedule BEFORE UPDATE OF next_run_at ON autopilots BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(state.db.pool()).await.unwrap();
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/autopilots/auto",
            json!({"status":"paused"})
        )
        .await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let status: String = sqlx::query_scalar("SELECT status FROM autopilots WHERE id='auto'")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert_eq!(status, "active");
}

#[tokio::test]
async fn automation_delete_reports_database_failure_and_missing_rows() {
    let (_dir, state, app) = fixture().await;
    assert_eq!(
        call(&app, "DELETE", "/api/autopilots/missing", Value::Null).await,
        StatusCode::NOT_FOUND
    );
    sqlx::query("CREATE TRIGGER reject_delete BEFORE DELETE ON autopilots BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(state.db.pool()).await.unwrap();
    assert_eq!(
        call(&app, "DELETE", "/api/autopilots/auto", Value::Null).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM autopilots WHERE id='auto'")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn updates_report_database_failure_instead_of_missing_resources() {
    let (_dir, state, app) = fixture().await;
    state.db.pool().close().await;
    for path in ["/api/tasks/task", "/api/autopilots/auto"] {
        assert_eq!(
            call(&app, "PUT", path, json!({})).await,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}

#[tokio::test]
async fn automation_create_rolls_back_when_its_schedule_cannot_be_saved() {
    let (_dir, state, app) = fixture().await;
    sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('demo-node', 'hub-host', 'local', '0')")
        .execute(state.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('workspace', 'demo-node', 'demo', '/home/me/repo', '0')")
        .execute(state.db.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_schedule BEFORE UPDATE OF next_run_at ON autopilots BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(state.db.pool()).await.unwrap();
    assert_eq!(call(&app, "POST", "/api/autopilots", json!({"name":"new","instructions":"work","workspace_id":"workspace","cron":"* * * * *"})).await, StatusCode::INTERNAL_SERVER_ERROR);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM autopilots")
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn task_note_reports_failed_persistence() {
    let (_dir, state, app) = fixture().await;
    sqlx::query("CREATE TRIGGER reject_note BEFORE INSERT ON task_comments BEGIN SELECT RAISE(FAIL, 'disk full'); END")
        .execute(state.db.pool()).await.unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/tasks/task/comments",
            json!({"body":"remember this","note":true})
        )
        .await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn successful_updates_persist_all_requested_fields_and_schedules() {
    let (_dir, state, app) = fixture().await;
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/tasks/task",
            json!({"title":"changed","status":"done"})
        )
        .await,
        StatusCode::OK
    );
    let task: (String, String, Option<String>) =
        sqlx::query_as("SELECT title, status, completed_at FROM tasks WHERE id='task'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!((task.0.as_str(), task.1.as_str()), ("changed", "done"));
    assert!(task.2.is_some());
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/autopilots/auto",
            json!({"name":"changed","status":"active"})
        )
        .await,
        StatusCode::OK
    );
    let automation: (String, Option<i64>) =
        sqlx::query_as("SELECT name, next_run_at FROM autopilots WHERE id='auto'")
            .fetch_one(state.db.pool())
            .await
            .unwrap();
    assert_eq!(automation.0, "changed");
    assert!(automation.1.is_some());
}
