use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use blazar_hub::{HubConfig, build_router, build_state};
use http_body_util::BodyExt;
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

fn get(path: &str, if_none_match: Option<&str>) -> Request<Body> {
    let mut b = Request::get(path);
    if let Some(v) = if_none_match {
        b = b.header(header::IF_NONE_MATCH, v);
    }
    let mut req = b.body(Body::empty()).unwrap();
    let remote: SocketAddr = "127.0.0.1:40000".parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(remote));
    req
}

#[tokio::test]
async fn assets_revalidate_with_an_etag_instead_of_a_day_long_cache() {
    let (app, _dir) = app().await;

    let res = app.clone().oneshot(get("/js/core.js", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/javascript; charset=utf-8"
    );
    let tag = res.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert!(
        tag.starts_with('"') && tag.ends_with('"') && tag.len() > 4,
        "{tag}"
    );
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert!(body.len() > 100);

    let res = app
        .clone()
        .oneshot(get("/js/core.js", Some(&tag)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(res.headers()[header::ETAG], tag.as_str());
    assert!(
        res.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );

    let res = app
        .clone()
        .oneshot(get("/js/core.js", Some("\"stale\"")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .clone()
        .oneshot(get("/js/core.js", Some(&format!("\"other\", {tag}"))))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    let css = app
        .clone()
        .oneshot(get("/css/app.css", None))
        .await
        .unwrap();
    assert_eq!(css.headers()[header::CACHE_CONTROL], "no-cache");
    assert_ne!(css.headers()[header::ETAG], tag.as_str());

    let res = app.clone().oneshot(get("/js/nope.js", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = app.clone().oneshot(get("/", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
    let html =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let versioned = format!("src=\"/js/core.js?v={}\"", tag.trim_matches('"'));
    assert!(html.contains(&versioned), "{versioned}");
    assert!(
        html.contains("href=\"/css/app.css?v="),
        "css reference is versioned too"
    );
    assert!(
        !html.contains("src=\"/js/core.js\""),
        "unversioned reference must be gone"
    );
    assert!(
        html.contains("/vendor/"),
        "vendor references are left alone"
    );

    let res = app
        .oneshot(get("/js/core.js?v=whatever", None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::ETAG], tag.as_str());
}
