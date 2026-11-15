mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use blazar_hub::routing::{self, Ask, Failure, Why};
use blazar_hub::{AppState, HubConfig, build_state_with_services, services::Services};
use chrono::{Duration, Utc};
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

async fn used(st: &AppState, account: &str, utilization: f64) {
    sqlx::query(
        "INSERT INTO rate_limit_snapshots (account_id, window_name, utilization, resets_at, observed_at)
         VALUES (?1, 'five_hour', ?2, ?3, ?4)",
    )
    .bind(account)
    .bind(utilization)
    .bind((Utc::now() + Duration::hours(2)).to_rfc3339())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await
    .unwrap();
}

fn ask(remote: bool) -> Ask<'static> {
    Ask {
        text: "fix the typo in the readme",
        images: false,
        remote,
        run_node: "local",
    }
}

#[tokio::test]
async fn settings_start_with_the_subscriptions_and_refuse_unknown_channels() {
    let (st, _dir) = state().await;
    let app = common::authenticated_router(st);
    let (s, v) = call(&app, "GET", "/api/routing", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["channels"], json!(["claude", "codex", "devin"]));
    assert_eq!(v["first"], "claude");
    assert_eq!(v["jev"]["key"], false);
    let ids: Vec<&str> = v["options"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["id"].as_str())
        .collect();
    assert_eq!(ids, ["claude", "codex", "devin", "droid", "grok"]);
    assert_eq!(v["enabled"], true);

    for bad in [
        json!({ "channels": ["claude", "gemini"], "first": "claude" }),
        json!({ "channels": ["codex"], "first": "claude" }),
        json!({ "channels": [], "first": "claude" }),
    ] {
        let (s, _) = call(&app, "PUT", "/api/routing", Some(bad)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
    let (s, _) = call(
        &app,
        "PUT",
        "/api/routing",
        Some(json!({ "channels": ["codex", "droid", "codex"], "first": "droid" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call(&app, "GET", "/api/routing", None).await;
    assert_eq!(v["channels"], json!(["codex", "droid"]));
    assert_eq!(v["first"], "droid");

    let (s, _) = call(
        &app,
        "PUT",
        "/api/routing",
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call(&app, "GET", "/api/routing", None).await;
    assert_eq!(v["enabled"], false);
    assert_eq!(v["channels"], json!(["codex", "droid"]));
}

#[tokio::test]
async fn the_key_stays_private_and_can_be_removed() {
    let (st, _dir) = state().await;
    let app = common::authenticated_router(st.clone());
    let (s, _) = call(
        &app,
        "PUT",
        "/api/routing/key",
        Some(json!({ "key": "has space" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        &app,
        "PUT",
        "/api/routing/key",
        Some(json!({ "key": "Bearer ts-test-0123456789" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call(&app, "GET", "/api/routing", None).await;
    assert_eq!(v["jev"]["key"], true);
    assert!(!v.to_string().contains("ts-test-0123456789"));
    assert_eq!(
        routing::jev::key(&st).await.as_deref(),
        Some("ts-test-0123456789")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = st
            .mesh_ctx
            .staging_dir
            .parent()
            .unwrap()
            .join("routing/typesafe.key");
        let mode = std::fs::metadata(file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let (s, _) = call(&app, "DELETE", "/api/routing/key", None).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call(&app, "GET", "/api/routing", None).await;
    assert_eq!(v["jev"]["key"], false);
    let (_, v) = call(&app, "POST", "/api/routing/test", Some(json!({}))).await;
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("key"));
}

#[tokio::test]
async fn auto_passes_over_a_full_subscription_and_says_why() {
    let (st, _dir) = state().await;
    used(&st, "claude-default", 0.9).await;
    let d = routing::decide(&st, ask(false)).await;
    assert_eq!(d.runtime, "codex");
    assert_eq!(d.note, "Auto → Codex · Claude Code is over its usage line");
    assert!(
        d.skipped
            .iter()
            .any(|k| k.channel == "claude" && k.why == Why::OverLine)
    );

    let away = routing::decide(&st, ask(true)).await;
    assert_eq!(away.runtime, "codex");
    assert!(
        away.skipped
            .iter()
            .any(|k| k.channel == "devin" && k.why == Why::LocalOnly)
    );
}

#[tokio::test]
async fn auto_stays_on_the_preferred_channel_while_it_has_room() {
    let (st, _dir) = state().await;
    used(&st, "claude-default", 0.3).await;
    used(&st, "codex-default", 0.05).await;
    let d = routing::decide(&st, ask(false)).await;
    assert_eq!(d.runtime, "claude");
    assert_eq!(d.note, "Auto → Claude Code");
    assert_eq!(d.model, None);
}

#[tokio::test]
async fn auto_falls_back_to_the_preferred_channel_when_nothing_fits() {
    let (st, _dir) = state().await;
    for a in ["claude-default", "codex-default", "devin-default"] {
        used(&st, a, 0.95).await;
    }
    let d = routing::decide(&st, ask(false)).await;
    assert_eq!(d.runtime, "claude");
    assert_eq!(
        d.note,
        "Auto → Claude Code · fallback: no channel is available"
    );
}

async fn fake_jev(
    status: StatusCode,
    reply: Value,
) -> (String, Arc<Mutex<Option<(String, Value)>>>) {
    let seen: Arc<Mutex<Option<(String, Value)>>> = Arc::default();
    let keep = seen.clone();
    let app = axum::Router::new().route(
        "/v1/systemone",
        axum::routing::post(move |headers: HeaderMap, body: axum::Json<Value>| {
            let keep = keep.clone();
            let reply = reply.clone();
            async move {
                let auth = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                *keep.lock().unwrap() = Some((auth, body.0));
                (status, axum::Json(reply))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/v1/systemone"), seen)
}

#[tokio::test]
async fn jev_gets_the_key_and_both_questions() {
    let (url, seen) = fake_jev(
        StatusCode::OK,
        json!({
            "model": "jev-1.14.0",
            "answers": {
                "difficulty": { "type": "score", "score": 0.3, "confidence": 0.9 },
                "kind": { "type": "choice", "choice": "docs", "confidence": 0.8 }
            },
            "usage": { "input_tokens": 120, "output_tokens": 0 }
        }),
    )
    .await;
    let two = [
        ("m0".to_owned(), "Claude Code with Opus".to_owned()),
        ("m1".to_owned(), "Claude Code with Haiku".to_owned()),
    ];
    let v = routing::jev::judge_at(&url, "ts-key-123456", "Fix a typo in the README", &two)
        .await
        .unwrap();
    assert_eq!(v.kind, "docs");
    assert!(v.difficulty < 0.5);
    assert_eq!(v.model.as_deref(), Some("jev-1.14.0"));
    let (auth, body) = seen.lock().unwrap().clone().unwrap();
    assert_eq!(auth, "Bearer ts-key-123456");
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"], "Fix a typo in the README");
    assert_eq!(body["questions"]["difficulty"]["type"], "score");
    assert_eq!(body["questions"]["kind"]["type"], "choice");
    assert_eq!(
        body["questions"]["model"]["criteria"]["m1"],
        "Claude Code with Haiku"
    );

    let (broke, _) = fake_jev(
        StatusCode::PAYMENT_REQUIRED,
        json!({ "detail": { "error_type": "insufficient_credits", "message": "Top up" } }),
    )
    .await;
    assert_eq!(
        routing::jev::judge_at(&broke, "ts-key-123456", "hi", &[]).await,
        Err(Failure::Credits)
    );
}

async fn picking_jev(wanted: &'static str) -> String {
    let app = axum::Router::new().route(
        "/v1/systemone",
        axum::routing::post(move |body: axum::Json<Value>| async move {
            let criteria = body.0["questions"]["model"]["criteria"].clone();
            let keys: Vec<String> = criteria
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let pick = keys
                .iter()
                .find(|k| {
                    criteria[k.as_str()]
                        .as_str()
                        .is_some_and(|a| a.contains(wanted))
                })
                .cloned()
                .unwrap_or_default();
            let probs: serde_json::Map<String, Value> = keys
                .iter()
                .map(|k| (k.clone(), json!(if *k == pick { 0.8 } else { 0.01 })))
                .collect();
            axum::Json(json!({
                "model": "jev-1.14.0",
                "answers": {
                    "difficulty": { "score": 0.2, "confidence": 0.9 },
                    "kind": { "choice": "question", "confidence": 0.9 },
                    "model": { "choice": pick, "confidence": 0.8, "probabilities": probs }
                },
                "usage": { "input_tokens": 300, "output_tokens": 0 }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1/systemone")
}

async fn use_jev(st: &AppState, url: &str) {
    sqlx::query(
        "INSERT INTO kv_settings (key, value, updated_at) VALUES ('routing', ?1, '0')
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(json!({ "jev_url": url }).to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn jev_picks_the_model_and_the_account_follows_the_quota() {
    let (st, _dir) = state().await;
    routing::jev::save_key(&st, "ts-key-123456").unwrap();
    use_jev(&st, &picking_jev("with Sonnet").await).await;
    used(&st, "claude-default", 0.2).await;
    let d = routing::decide(&st, ask(false)).await;
    assert_eq!(d.runtime, "claude");
    assert_eq!(d.model.as_deref(), Some("sonnet"));
    assert_eq!(d.account.as_deref(), Some("claude-default"));
    assert_eq!(d.note, "Auto → Claude Code · Sonnet · trivial question");
    assert!(d.verdict.is_some_and(|v| v.pick.is_some()));

    use_jev(&st, &picking_jev("high reasoning effort").await).await;
    let d = routing::decide(&st, ask(false)).await;
    assert_eq!(d.runtime, "codex");
    assert_eq!(d.effort.as_deref(), Some("high"));
    assert_eq!(d.account.as_deref(), Some("codex-default"));
}

#[tokio::test]
async fn typed_model_names_resolve_across_every_agent() {
    let (st, _dir) = state().await;
    let app = common::authenticated_router(st);
    let (_, v) = call(
        &app,
        "POST",
        "/api/routing/resolve",
        Some(json!({ "query": "codex high" })),
    )
    .await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["pick"]["agent"], "r:codex");
    assert_eq!(v["pick"]["effort"], "high");
    let (_, v) = call(
        &app,
        "POST",
        "/api/routing/resolve",
        Some(json!({ "query": "haiku" })),
    )
    .await;
    assert_eq!(v["pick"]["model"], "haiku");
    let (_, v) = call(
        &app,
        "POST",
        "/api/routing/resolve",
        Some(json!({ "query": "nothing-like-this" })),
    )
    .await;
    assert_eq!(v["ok"], false);
}
