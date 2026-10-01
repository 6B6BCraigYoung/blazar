use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use blazar_hub::{HubConfig, build_router, build_state};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = HubConfig {
        db_path: dir.path().join("blazar.sqlite"),
        bind: ([127, 0, 0, 1], 0).into(),
        mesh_via: "local".into(),
        mesh_container: None,
        engine_dir: None,
    };
    let st = build_state(&cfg).await.unwrap();
    (build_router(st), dir)
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

#[tokio::test]
async fn default_logins_are_listed_and_cannot_be_deleted() {
    let (app, _dir) = app().await;
    let (s, v) = call(&app, "GET", "/api/accounts", None).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["claude-default", "codex-default"]);
    assert!(v["accounts"][0]["builtin"].as_bool().unwrap());
    assert_eq!(v["modes"]["claude"], "");

    let (s, _) = call(&app, "DELETE", "/api/accounts/claude-default", None).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn mode_accepts_auto_and_own_accounts_only() {
    let (app, _dir) = app().await;
    let set = |mode: &str| json!({ "provider": "claude", "mode": mode });

    let (s, _) = call(&app, "PUT", "/api/accounts/mode", Some(set("auto"))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call(&app, "GET", "/api/accounts", None).await;
    assert_eq!(v["modes"]["claude"], "auto");

    let (s, _) = call(
        &app,
        "PUT",
        "/api/accounts/mode",
        Some(set("codex-default")),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "不能把 Codex 的账号设给 Claude");
    let (s, _) = call(&app, "PUT", "/api/accounts/mode", Some(set("nope"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn disabling_and_renaming_a_login() {
    let (app, _dir) = app().await;
    let (s, v) = call(
        &app,
        "PUT",
        "/api/accounts/codex-default",
        Some(json!({ "label": "  工作号 ", "disabled": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["label"], "工作号");
    assert_eq!(v["disabled"], true);

    let (s, _) = call(
        &app,
        "PUT",
        "/api/accounts/codex-default",
        Some(json!({ "label": "" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agent_profiles_remember_their_account() {
    let (app, _dir) = app().await;
    let (s, v) = call(
        &app,
        "POST",
        "/api/agent-profiles",
        Some(json!({ "name": "审查员", "runtime": "claude", "account": "auto" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["account"], "auto");

    let id = v["id"].as_str().unwrap().to_owned();
    let mut body = v.clone();
    body["account"] = json!("");
    let (s, v) = call(
        &app,
        "PUT",
        &format!("/api/agent-profiles/{id}"),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["account"].is_null(), "空字符串表示跟随默认设置");
}

#[tokio::test]
async fn the_default_login_can_switch_to_a_setup_token() {
    let (app, _dir) = app().await;
    let (s, _) = call(
        &app,
        "PUT",
        "/api/accounts/claude-default/token",
        Some(json!({ "token": "nope" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, v) = call(
        &app,
        "PUT",
        "/api/accounts/claude-default/token",
        Some(json!({ "token": "sk-ant-oat01-blazar-test-token" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["kind"], "token");
    assert_eq!(v["builtin"], false, "转换后有了自己的账号目录");
    assert!(
        !v.to_string().contains("sk-ant-oat01"),
        "token 不能出现在接口返回里"
    );

    let (s, _) = call(
        &app,
        "PUT",
        "/api/accounts/codex-default/token",
        Some(json!({ "token": "sk-ant-oat01-x" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}
