mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use blazar_hub::{HubConfig, build_state_with_services, services::Services};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn get(app: &axum::Router, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn history_drops_progress_before_the_last_prompt_and_keeps_payloads_as_stored() {
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
    let pool = state.db.pool().clone();
    let app = common::authenticated_router(state);
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, created_at)
         SELECT 'w', id, 'w', '/w', 't' FROM nodes WHERE name = 'local'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, thread_id)
         VALUES ('s1', 'w', 'claude_cli', 'done', '2026-10-19T00:00:00Z', 's1'),
                ('s2', 'w', 'claude_cli', 'done', '2026-10-19T01:00:00Z', 's1')",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (sid, seq, payload, parent) in [
        ("s1", 1, r#"{"type":"user_message","text":"第一轮"}"#, None),
        (
            "s1",
            2,
            r#"{"type":"progress","message_id":"m1","output_tokens":10}"#,
            None,
        ),
        ("s1", 3, r#"{"type":"assistant_message","text":"好"}"#, None),
        (
            "s1",
            4,
            r#"{"type":"progress","message_id":"m1","output_tokens":20}"#,
            None,
        ),
        ("s2", 1, r#"{"type":"user_message","text":"第二轮"}"#, None),
        (
            "s2",
            2,
            r#"{"type":"progress","message_id":"m2","output_tokens":30}"#,
            None,
        ),
        (
            "s2",
            3,
            r#"{"type":"thinking","text":"想一想"}"#,
            Some("t1"),
        ),
    ] {
        sqlx::query(
            "INSERT INTO events (session_id, seq, ts, payload, parent_tool_id)
             VALUES (?1, ?2, '2026-10-19T00:00:00Z', ?3, ?4)",
        )
        .bind(sid)
        .bind(seq)
        .bind(payload)
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    }

    let rows = get(&app, "/api/workspaces/w/history?session=s1").await;
    let rows = rows.as_array().unwrap();
    let kinds: Vec<&str> = rows
        .iter()
        .map(|r| r["kind"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "user_message",
            "assistant_message",
            "user_message",
            "progress",
            "thinking"
        ]
    );
    assert_eq!(
        rows[3]["kind"],
        json!({"type":"progress","message_id":"m2","output_tokens":30})
    );
    assert_eq!(rows[3]["cursor"], "s2:2");
    assert_eq!(rows[4]["parent"], "t1");
    assert_eq!(rows[4]["rewound"], false);

    let paged = get(&app, "/api/workspaces/w/history?session=s1&limit=10").await;
    assert_eq!(paged.as_array().unwrap().len(), 7);
}
