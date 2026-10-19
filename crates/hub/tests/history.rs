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

async fn setup() -> (axum::Router, sqlx::SqlitePool, tempfile::TempDir) {
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
    (app, pool, dir)
}

async fn insert(pool: &sqlx::SqlitePool, events: &[(&str, i64, &str, Option<&str>)]) {
    for (sid, seq, payload, parent) in events {
        sqlx::query(
            "INSERT INTO events (session_id, seq, ts, payload, parent_tool_id)
             VALUES (?1, ?2, '2026-10-19T00:00:00Z', ?3, ?4)",
        )
        .bind(sid)
        .bind(seq)
        .bind(payload)
        .bind(parent)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn history_drops_progress_before_the_last_prompt_and_keeps_payloads_as_stored() {
    let (app, pool, _dir) = setup().await;
    insert(
        &pool,
        &[
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
        ],
    )
    .await;

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

#[tokio::test]
async fn subagent_results_and_thinking_are_elided_until_their_task_is_asked_for() {
    let (app, pool, _dir) = setup().await;
    let task = r#"{"type":"tool_use","id":"task1","name":"Task","input":{"description":"查"}}"#;
    let sub_use = r#"{"type":"tool_use","id":"c1","name":"Read","input":{"file_path":"/w/a"}}"#;
    let sub_result = r#"{"type":"tool_result","id":"c1","ok":false,"content":"很长的输出","structured":{"x":1}}"#;
    let sub_thinking = r#"{"type":"thinking","text":"子智能体在想"}"#;
    let other = r#"{"type":"tool_result","id":"c2","ok":true,"content":"别的任务"}"#;
    let main_result = r#"{"type":"tool_result","id":"task1","ok":true,"content":"完成"}"#;
    insert(
        &pool,
        &[
            ("s1", 1, r#"{"type":"user_message","text":"开始"}"#, None),
            ("s1", 2, task, None),
            ("s1", 3, sub_thinking, Some("task1")),
            ("s1", 4, sub_use, Some("task1")),
            ("s1", 5, sub_result, Some("task1")),
            ("s1", 6, other, Some("task2")),
            ("s1", 7, main_result, None),
        ],
    )
    .await;
    let raw = |p: &str| serde_json::from_str::<Value>(p).unwrap();

    let rows = get(&app, "/api/workspaces/w/history?session=s1").await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[1]["kind"], raw(task));
    assert_eq!(rows[2]["kind"], json!({"type":"thinking","text":""}));
    assert_eq!(rows[2]["elided"], true);
    assert_eq!(rows[3]["kind"], raw(sub_use));
    assert_ne!(rows[3]["elided"], true);
    assert_eq!(
        rows[4]["kind"],
        json!({"type":"tool_result","id":"c1","ok":false,"content":""})
    );
    assert_eq!(rows[4]["elided"], true);
    assert_eq!(
        rows[5]["kind"],
        json!({"type":"tool_result","id":"c2","ok":true,"content":""})
    );
    assert_eq!(rows[6]["kind"], raw(main_result));
    assert_ne!(rows[6]["elided"], true);

    let paged = get(&app, "/api/workspaces/w/history?session=s1&limit=50").await;
    let paged = paged.as_array().unwrap();
    assert_eq!(paged.len(), 7);
    assert_eq!(paged[2]["kind"], raw(sub_thinking));
    assert_eq!(paged[4]["kind"], raw(sub_result));
    assert!(paged.iter().all(|r| r.get("elided").is_none()));

    let sub = get(&app, "/api/workspaces/w/history?session=s1&parent=task1").await;
    let sub = sub.as_array().unwrap();
    let cursors: Vec<&str> = sub.iter().map(|r| r["cursor"].as_str().unwrap()).collect();
    assert_eq!(cursors, ["s1:3", "s1:4", "s1:5"]);
    assert_eq!(sub[0]["kind"], raw(sub_thinking));
    assert_eq!(sub[1]["kind"], raw(sub_use));
    assert_eq!(sub[2]["kind"], raw(sub_result));
    assert!(
        sub.iter()
            .all(|r| r["elided"] != true && r["parent"] == "task1")
    );
}
