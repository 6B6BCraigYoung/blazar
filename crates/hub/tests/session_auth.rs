use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use blazar_hub::{HubConfig, build_router, build_state_with_services, services::Services};
use tower::ServiceExt;

async fn fixture(
    dev_origin: Option<&str>,
) -> (
    axum::Router,
    String,
    tempfile::TempDir,
    std::sync::Arc<blazar_hub::AppState>,
) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = HubConfig {
        db_path: dir.path().join("blazar.sqlite"),
        bind: ([127, 0, 0, 1], 0).into(),
        mesh_via: "local".into(),
        mesh_container: None,
        engine_dir: None,
    };
    let state = build_state_with_services(&cfg, Services::Isolated)
        .await
        .unwrap();
    let mut state = std::sync::Arc::try_unwrap(state).ok().unwrap();
    let _ = state.auth.take();
    assert!(
        state
            .auth
            .set(
                blazar_hub::auth::Session::new(&cfg.db_path, dev_origin.map(str::to_owned))
                    .unwrap()
            )
            .is_ok()
    );
    let token = state.auth.get().unwrap().token().to_owned();
    let state = std::sync::Arc::new(state);
    (build_router(state.clone()), token, dir, state)
}

fn request(method: &str, path: &str, peer: &str, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("Host", "127.0.0.1:7777");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    request
}

#[tokio::test]
async fn loopback_browser_bootstraps_a_private_cookie_and_remote_callers_need_a_token() {
    let (app, token, _dir, _) = fixture(None).await;
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/auth/session",
            "127.0.0.1:40000",
            &[
                ("Origin", "http://127.0.0.1:7777"),
                ("Sec-Fetch-Site", "same-origin"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly; SameSite=Strict"));
    assert_eq!(response.headers()["cache-control"], "no-store");
    let cookie = cookie.split(';').next().unwrap();
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/snippets",
            "127.0.0.1:40000",
            &[("Cookie", cookie)],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/auth/session",
            "10.99.0.2:40000",
            &[("X-Forwarded-For", "127.0.0.1")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let bearer = format!("Bearer {token}");
    let response = app
        .oneshot(request(
            "GET",
            "/api/snippets",
            "10.99.0.2:40000",
            &[("Authorization", &bearer)],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn credentials_do_not_override_cross_site_checks() {
    let (app, token, _dir, _) = fixture(None).await;
    let bearer = format!("Bearer {token}");
    for path in [
        "/api/snippets",
        "/api/ws",
        "/api/auth/session",
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                path,
                "127.0.0.1:40000",
                &[
                    ("Authorization", &bearer),
                    ("Origin", "https://other.example"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let response = app
        .oneshot(request(
            "POST",
            "/api/auth/session",
            "127.0.0.1:40000",
            &[("Sec-Fetch-Site", "cross-site")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn development_origin_is_explicit_and_only_trusted_through_loopback() {
    let (app, _, _dir, _) = fixture(Some("http://127.0.0.1:8080")).await;
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/auth/session",
            "127.0.0.1:40000",
            &[("Origin", "http://127.0.0.1:8080")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/auth/session",
            "10.99.0.2:40000",
            &[("Origin", "http://127.0.0.1:8080")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .oneshot(request(
            "POST",
            "/api/auth/session",
            "127.0.0.1:40000",
            &[("Origin", "http://127.0.0.1:8081")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn embedded_http_and_websocket_share_the_browser_session() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (app, _, _dir, _) = fixture(None).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(blazar_hub::serve(listener, app));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let origin = format!("http://{address}");
    let denied = client
        .get(format!("{origin}/api/snippets"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let bootstrap = client
        .post(format!("{origin}/api/auth/session"))
        .header("Origin", &origin)
        .send()
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::NO_CONTENT);
    let cookie = bootstrap.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let allowed = client
        .get(format!("{origin}/api/snippets"))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("GET /api/ws HTTP/1.1\r\nHost: {address}\r\nOrigin: {origin}\r\nCookie: {cookie}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").as_bytes()).await.unwrap();
    let mut bytes = [0; 4096];
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), socket.read(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes[..read].starts_with(b"HTTP/1.1 101 "));
    server.abort();
}

#[tokio::test]
async fn api_and_websocket_require_a_session_even_without_origin() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = HubConfig {
        db_path: dir.path().join("blazar.sqlite"),
        bind: ([127, 0, 0, 1], 0).into(),
        mesh_via: "local".into(),
        mesh_container: None,
        engine_dir: None,
    };
    let st = build_state_with_services(&cfg, Services::Isolated)
        .await
        .unwrap();
    let app = build_router(st);
    for peer in ["127.0.0.1:40000", "10.99.0.2:40000"] {
        for path in ["/api/snippets", "/api/ws"] {
            let mut request = Request::builder()
                .uri(path)
                .header("Host", "127.0.0.1:7777")
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{peer} {path}");
        }
    }
}
