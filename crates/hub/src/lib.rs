use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use blazar_db::Db;
pub use blazar_db::ensure_private_dir;

mod agent;
pub mod api;
pub mod auth;
pub mod git;
pub mod mesh;
pub mod office;
mod routes;
mod skills;
mod work;
pub use agent::{accounts, agents, catalog, chat, checkpoint, proxy, remote_cli, run, titles};
pub use skills::{library, skillhub};
pub use work::{analytics, autopilot, inbox, rules, scripts, snippets, tasks};
pub mod services;
pub mod state;

pub use routes::build_router;
pub use state::AppState;

static ASSETS: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/static");

static WEB: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/../web/dist");

#[derive(Debug, Clone)]
pub struct HubConfig {
    pub db_path: std::path::PathBuf,
    pub bind: SocketAddr,

    pub mesh_via: String,
    pub mesh_container: Option<String>,

    pub engine_dir: Option<std::path::PathBuf>,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            db_path: default_db_path().unwrap_or_else(|_| "blazar.sqlite".into()),
            bind: ([127, 0, 0, 1], 7777).into(),
            mesh_via: "local".into(),
            mesh_container: None,
            engine_dir: None,
        }
    }
}

pub fn default_db_path() -> anyhow::Result<std::path::PathBuf> {
    let dir = directories::ProjectDirs::from("ai", "blazar", "Blazar")
        .ok_or_else(|| anyhow::anyhow!("cannot determine the platform data directory"))?
        .data_dir()
        .join("hub");
    ensure_private_dir(&dir)?;
    Ok(dir.join("blazar.sqlite"))
}

pub async fn build_state(cfg: &HubConfig) -> Result<Arc<AppState>> {
    build_state_with_services(cfg, services::Services::Interactive).await
}

pub async fn build_state_with_services(
    cfg: &HubConfig,
    services: services::Services,
) -> Result<Arc<AppState>> {
    let db = open_db(&cfg.db_path).await?;
    reconcile_after_restart(&db).await?;
    let staging = cfg
        .db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("mesh-staging");
    let mesh_ctx = mesh::MeshCtx::new(cfg.engine_dir.as_deref(), staging);
    let st = AppState::with_services(
        db,
        cfg.mesh_via.clone(),
        cfg.mesh_container.clone(),
        mesh_ctx,
        services,
    );
    let auth = auth::Session::new(&cfg.db_path, std::env::var("BLAZAR_DEV_ORIGIN").ok())?;
    let _ = st.auth.set(auth);

    services.spawn(run::reattach_all(st.clone()));
    services.spawn(proxy::restore(st.clone()));

    let cat = st.clone();
    services.spawn(async move {
        let _ = catalog::claude(&cat, false).await;
    });
    let warm = st.clone();
    services.spawn(async move { mesh::MeshCtx::warm(&warm).await });
    services.spawn(git::poll_prs(st.clone()));
    services.spawn(autopilot::scheduler(st.clone()));
    Ok(st)
}

async fn reconcile_after_restart(db: &Db) -> Result<()> {
    let n = sqlx::query(
        "UPDATE sessions SET status = 'interrupted' WHERE status = 'running' AND run_dir IS NULL",
    )
    .execute(db.pool())
    .await?
    .rows_affected();
    if n > 0 {
        tracing::info!(target: "blazar::hub", "重启前遗留的 {n} 个会话已标记为中断");

        sqlx::query(
            "UPDATE workspaces SET activity = 'idle'
             WHERE activity IN ('running', 'awaiting_approval')
               AND id NOT IN (SELECT workspace_id FROM sessions
                              WHERE status = 'running' AND run_dir IS NOT NULL)",
        )
        .execute(db.pool())
        .await?;
    }
    Ok(())
}

async fn open_db(path: &Path) -> Result<Db> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建数据目录 {} 失败", parent.display()))?;
    }
    Db::open(path)
        .await
        .with_context(|| format!("打开数据库 {} 失败", path.display()))
}

pub async fn bind(cfg: &HubConfig) -> Result<(SocketAddr, tokio::net::TcpListener)> {
    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("绑定 {} 失败", cfg.bind))?;
    let addr = listener.local_addr()?;

    let _ = office::HUB_URL.set(format!("http://{addr}"));
    Ok((addr, listener))
}

pub async fn serve(listener: tokio::net::TcpListener, router: Router) -> Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

pub async fn spawn(cfg: HubConfig) -> Result<(SocketAddr, Arc<AppState>)> {
    let st = build_state(&cfg).await?;
    let (addr, listener) = bind(&cfg).await?;
    if let Some(auth) = st.auth.get() {
        auth.publish(addr)?;
    }
    let router = build_router(st.clone());
    tokio::spawn(async move {
        if let Err(err) = serve(listener, router).await {
            tracing::error!("hub 服务退出: {err:#}");
        }
    });
    Ok((addr, st))
}

static ASSET_TAGS: std::sync::OnceLock<std::collections::HashMap<String, String>> =
    std::sync::OnceLock::new();

fn asset_tag(rel: &str) -> &'static str {
    let tags = ASSET_TAGS.get_or_init(|| {
        fn walk(dir: &include_dir::Dir<'_>, out: &mut std::collections::HashMap<String, String>) {
            use std::hash::{Hash, Hasher};
            for f in dir.files() {
                let mut h = std::hash::DefaultHasher::new();
                f.contents().hash(&mut h);
                out.insert(
                    f.path().to_string_lossy().into_owned(),
                    format!("\"{:016x}\"", h.finish()),
                );
            }
            for d in dir.dirs() {
                walk(d, out);
            }
        }
        let mut out = std::collections::HashMap::new();
        walk(&ASSETS, &mut out);
        out
    });
    tags.get(rel).map(String::as_str).unwrap_or("\"0\"")
}

async fn asset(
    uri: axum::http::Uri,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    let prefix = uri
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("");
    let rel = format!("{prefix}/{}", path.trim_start_matches('/'));
    let Some(file) = ASSETS.get_file(&rel) else {
        return (StatusCode::NOT_FOUND, "no such asset").into_response();
    };
    let tag = asset_tag(&rel);
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == tag || t.trim() == "*"));
    if fresh {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, tag), (header::CACHE_CONTROL, "no-cache")],
        )
            .into_response();
    }

    (
        [
            (header::CONTENT_TYPE, mime_of(&path)),
            (header::CACHE_CONTROL, "no-cache"),
            (header::ETAG, tag),
        ],
        file.contents(),
    )
        .into_response()
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "ttf" => "font/ttf",
        "woff2" => "font/woff2",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

async fn v2_redirect(
    uri: axum::http::Uri,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> axum::response::Redirect {
    let q = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    axum::response::Redirect::permanent(&format!("/{}{q}", path.trim_start_matches('/')))
}

async fn web(method: axum::http::Method, uri: axum::http::Uri) -> axum::response::Response {
    use axum::http::{Method, StatusCode, header};
    use axum::response::IntoResponse;
    let rel = uri.path().trim_start_matches('/');
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    match WEB.get_file(rel) {
        Some(f) if !rel.is_empty() && rel != "index.html" => (
            [
                (header::CONTENT_TYPE, mime_of(rel)),
                (
                    header::CACHE_CONTROL,
                    if rel.contains('/') {
                        "no-cache"
                    } else {
                        "public, max-age=31536000, immutable"
                    },
                ),
            ],
            f.contents(),
        )
            .into_response(),
        None if rel.starts_with("api/")
            || rel.starts_with("snippets/")
            || [
                ".js", ".wasm", ".css", ".map", ".png", ".svg", ".woff2", ".ico",
            ]
            .iter()
            .any(|ext| rel.ends_with(ext)) =>
        {
            StatusCode::NOT_FOUND.into_response()
        }
        _ => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            WEB.get_file("index.html").map_or(&[][..], |f| f.contents()),
        )
            .into_response(),
    }
}
