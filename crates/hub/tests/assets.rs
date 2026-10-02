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
    let path = "/vendor/xterm.js";

    let res = app.clone().oneshot(get(path, None)).await.unwrap();
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
    assert!(res.into_body().collect().await.unwrap().to_bytes().len() > 100);

    let res = app.clone().oneshot(get(path, Some(&tag))).await.unwrap();
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
        .oneshot(get(path, Some("\"stale\"")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .clone()
        .oneshot(get(path, Some(&format!("\"other\", {tag}"))))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);

    let res = app.oneshot(get("/vendor/nope.js", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_ui_is_served_at_the_root_and_missing_assets_are_not_html() {
    let (app, _dir) = app().await;
    for path in [
        "/",
        "/index.html",
        "/w/example",
        "/nodes/local",
        "/agents/new",
        "/tasks",
    ] {
        let res = app.clone().oneshot(get(path, None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
        assert_eq!(
            res.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8",
            "{path}"
        );
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache", "{path}");
        assert!(
            !res.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty(),
            "{path}"
        );
    }
    // 页面引用的文件：带哈希的永久缓存，snippets/ 下文件名不变的每次重新验证
    let index = app.clone().oneshot(get("/", None)).await.unwrap();
    let index = String::from_utf8(
        index
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let refs: Vec<&str> = index
        .split("href=\"/")
        .skip(1)
        .filter_map(|r| r.split('"').next())
        .filter(|r| r.ends_with(".js") || r.ends_with(".wasm"))
        .collect();
    assert!(refs.iter().any(|r| r.starts_with("snippets/")), "{refs:?}");
    for r in refs {
        let res = app
            .clone()
            .oneshot(get(&format!("/{r}"), None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{r}");
        let want = if r.contains('/') {
            "no-cache"
        } else {
            "public, max-age=31536000, immutable"
        };
        assert_eq!(res.headers()[header::CACHE_CONTROL], want, "{r}");
    }
    for path in [
        "/missing.js",
        "/missing_bg.wasm",
        "/missing.css",
        "/snippets/missing/file.js",
        "/api/missing",
    ] {
        assert_eq!(
            app.clone().oneshot(get(path, None)).await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    // 界面曾经挂在 /v2 下：旧链接跳到同一页。
    for (from, to) in [
        ("/v2", "/"),
        ("/v2/", "/"),
        ("/v2/w/example?thread=t1", "/w/example?thread=t1"),
    ] {
        let res = app.clone().oneshot(get(from, None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::PERMANENT_REDIRECT, "{from}");
        assert_eq!(res.headers()[header::LOCATION], to, "{from}");
    }
}
