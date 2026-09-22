use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::{delete, get, post, put};
use blazar_db::Db;

mod agent;
pub mod api;
pub mod git;
pub mod mesh;
pub mod office;
mod skills;
mod work;
pub use agent::{agents, catalog, chat, checkpoint, run, titles};
pub use skills::{library, skillhub};
pub use work::{analytics, autopilot, inbox, rules, scripts, snippets, tasks};
pub mod state;

pub use state::AppState;

pub const INDEX_HTML: &str = include_str!("../static/index.html");

static ASSETS: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/static");

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
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("blazar.sqlite"))
}

pub async fn build_state(cfg: &HubConfig) -> Result<Arc<AppState>> {
    let db = open_db(&cfg.db_path).await?;
    reconcile_after_restart(&db).await?;
    let staging = cfg
        .db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("mesh-staging");
    let mesh_ctx = mesh::MeshCtx::new(cfg.engine_dir.as_deref(), staging);
    let st = AppState::new(
        db,
        cfg.mesh_via.clone(),
        cfg.mesh_container.clone(),
        mesh_ctx,
    );

    tokio::spawn(run::reattach_all(st.clone()));

    let cat = st.clone();
    tokio::spawn(async move {
        let _ = catalog::claude(&cat, false).await;
    });
    let warm = st.clone();
    tokio::spawn(async move { mesh::MeshCtx::warm(&warm).await });
    tokio::spawn(git::poll_prs(st.clone()));
    tokio::spawn(autopilot::scheduler(st.clone()));
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

pub fn build_router(st: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/vendor/{*path}", get(asset))
        .route("/css/{*path}", get(asset))
        .route("/js/{*path}", get(asset))
        .route("/img/{*path}", get(asset))
        .route("/api/state", get(api::get_state))
        .route("/api/agents", get(api::list_agents))
        .route("/api/runtimes", get(api::runtimes))
        .route("/api/runtimes/{id}/models", get(api::runtime_models))
        .route("/api/runtimes/claude/catalog", get(catalog::claude_catalog))
        .route(
            "/api/agent-profiles",
            get(agents::list).post(agents::create),
        )
        .route(
            "/api/agent-profiles/{id}",
            get(agents::detail)
                .put(agents::update)
                .delete(agents::delete),
        )
        .route("/api/agent-profiles/{id}/archive", post(agents::archive))
        .route("/api/agent-profiles/{id}/restore", post(agents::restore))
        .route(
            "/api/agent-profiles/{id}/cancel-runs",
            post(agents::cancel_runs),
        )
        .route(
            "/api/agent-profiles/{id}/duplicate",
            post(agents::duplicate),
        )
        .route("/api/nodes/{name}/probe", post(api::probe_node))
        .route("/api/nodes/{name}/agents", get(api::node_agents))
        .route("/api/nodes/{name}/browse", get(api::browse_node))
        .route(
            "/api/agent-configs",
            get(api::list_agent_configs).post(api::upsert_agent_config),
        )
        .route("/api/agent-configs/{id}", delete(api::delete_agent_config))
        .route("/api/hooks", get(api::list_hooks).post(api::create_hook))
        .route("/api/hooks/{id}", delete(api::delete_hook))
        .route("/api/mesh/refresh", post(api::refresh_mesh))
        .route("/api/mesh/local", get(mesh::local))
        .route("/api/mesh/external-rpc", put(mesh::set_external_rpc))
        .route("/api/mesh/topology", get(mesh::topology))
        .route("/api/mesh/join/preview", post(mesh::preview))
        .route("/api/mesh/join", post(mesh::join).delete(mesh::dismiss))
        .route("/api/mesh/leave", post(mesh::leave))
        .route(
            "/api/mesh/config",
            get(mesh::config_get).put(mesh::config_apply),
        )
        .route("/api/mesh/config/preview", post(mesh::config_preview))
        .route(
            "/api/mesh/issuer",
            get(mesh::issuer)
                .put(mesh::set_issuer)
                .delete(mesh::clear_issuer),
        )
        .route(
            "/api/mesh/invites",
            get(mesh::list_invites).post(mesh::issue),
        )
        .route("/api/mesh/invites/{id}", delete(mesh::revoke))
        .route("/api/nodes/health", get(api::node_health))
        .route("/api/workspaces", post(api::create_workspace))
        .route("/api/workspaces/isolated", post(api::create_isolated))
        .route("/api/workspaces/{id}/push", post(api::push_workspace))
        .route("/api/nodes/{name}/branches", get(api::list_branches))
        .route("/api/nodes/{name}/leftovers", get(api::node_leftovers))
        .route("/api/approvals", get(api::list_approvals))
        .route("/api/approvals/{id}", post(api::decide_approval))
        .route("/api/sessions/{id}/input", post(api::session_input))
        .route("/api/sessions/{id}/control", post(api::session_control))
        .route("/api/workspaces/{id}/checkpoints", get(checkpoint::list))
        .route(
            "/api/workspaces/{id}/sessions",
            get(api::workspace_sessions),
        )
        .route("/api/sessions/{id}/title", put(titles::rename))
        .route("/api/tasks", get(tasks::list).post(tasks::create))
        .route(
            "/api/tasks/{id}",
            get(tasks::detail).put(tasks::update).delete(tasks::delete),
        )
        .route("/api/tasks/{id}/start", post(tasks::start))
        .route("/api/tasks/{id}/comments", post(tasks::comment))
        .route("/api/checkpoints/{id}/restore", post(checkpoint::restore))
        .route("/api/nodes/{name}/sweep", post(api::sweep_node))
        .route("/api/workspaces/{id}", delete(api::delete_workspace))
        .route("/api/workspaces/{id}/pause", post(api::pause_workspace))
        .route("/api/workspaces/{id}/resume", post(api::resume_workspace))
        .route("/api/workspaces/{id}/commit", post(api::commit_workspace))
        .route("/api/workspaces/{id}/destroy", post(api::destroy_workspace))
        .route("/api/workspaces/{id}/tree", get(api::workspace_tree))
        .route("/api/workspaces/{id}/raw", get(api::workspace_raw))
        .route(
            "/api/workspaces/{id}/file",
            get(api::workspace_file).put(api::workspace_file_write),
        )
        .route("/api/workspaces/{id}/diff", get(api::workspace_diff))
        .route(
            "/api/autopilots",
            get(autopilot::list).post(autopilot::create),
        )
        .route("/api/autopilots/preview", post(autopilot::preview))
        .route(
            "/api/autopilots/{id}",
            get(autopilot::detail)
                .put(autopilot::update)
                .delete(autopilot::delete),
        )
        .route("/api/autopilots/{id}/run", post(autopilot::run_now))
        .route(
            "/api/autopilots/{id}/webhook",
            post(autopilot::rotate_webhook).delete(autopilot::disable_webhook),
        )
        .route("/api/webhooks/{token}", post(autopilot::hook))
        .route(
            "/api/workspaces/{id}/scripts",
            get(scripts::get).put(scripts::put),
        )
        .route(
            "/api/workspaces/{id}/scripts/{kind}/run",
            post(scripts::run),
        )
        .route("/api/workspaces/{id}/script-runs", get(scripts::runs))
        .route("/api/workspaces/{id}/dev", get(scripts::dev_get))
        .route("/api/workspaces/{id}/dev/start", post(scripts::dev_start))
        .route("/api/workspaces/{id}/dev/stop", post(scripts::dev_stop))
        .route(
            "/api/skills",
            get(library::skills).post(library::create_skill),
        )
        .route(
            "/api/skills/import",
            get(library::import_scan).post(library::import),
        )
        .route("/api/skills/market/providers", get(skillhub::providers))
        .route("/api/skills/market/key", put(skillhub::set_key))
        .route("/api/skills/market/search", get(skillhub::search))
        .route("/api/skills/market/repo", get(skillhub::repo))
        .route("/api/skills/market/preview", post(skillhub::preview))
        .route("/api/skills/market/install", post(skillhub::install))
        .route(
            "/api/skills/{id}",
            get(library::skill).delete(library::delete_skill),
        )
        .route(
            "/api/skills/{id}/files",
            put(library::put_file).delete(library::delete_file),
        )
        .route(
            "/api/mcp-servers",
            get(library::mcp_list).post(library::mcp_create),
        )
        .route(
            "/api/mcp-servers/{id}",
            put(library::mcp_update).delete(library::mcp_delete),
        )
        .route(
            "/api/agent-profiles/{id}/capabilities",
            get(library::agent_caps).put(library::set_agent_caps),
        )
        .route(
            "/api/settings",
            get(office::get_prefs).put(office::put_prefs),
        )
        .route("/api/about", get(office::about))
        .route("/api/office/lark", get(office::lark_status))
        .route("/api/office/lark/settings", put(office::put_lark_settings))
        .route("/api/office/lark/run", post(office::lark_run))
        .route("/api/office/lark/calls", get(office::lark_calls))
        .route("/api/office/github", get(office::github_status))
        .route("/api/office/obsidian", get(office::obsidian::status))
        .route(
            "/api/office/obsidian/settings",
            put(office::obsidian::put_settings),
        )
        .route("/api/office/obsidian/note", post(office::obsidian::note))
        .route(
            "/api/tasks/{id}/obsidian-note",
            post(office::obsidian::task_note),
        )
        .route("/api/office/{app}/connect", post(office::connect::start))
        .route("/api/office/{app}/job", get(office::connect::job))
        .route(
            "/api/office/{app}/job/cancel",
            post(office::connect::cancel),
        )
        .route("/api/office/lark/test-notify", post(office::test_notify))
        .route("/api/tasks/{id}/lark-doc", post(office::task_doc))
        .route("/api/inbox", get(inbox::list).post(inbox::mark))
        .route("/api/inbox/count", get(inbox::count))
        .route("/api/inbox/seen/{thread}", post(inbox::seen_thread))
        .route("/api/approval-rules", get(rules::list).post(rules::create))
        .route(
            "/api/approval-rules/{id}",
            put(rules::toggle).delete(rules::delete),
        )
        .route("/api/snippets", get(snippets::list).post(snippets::create))
        .route(
            "/api/snippets/{id}",
            put(snippets::update).delete(snippets::delete),
        )
        .route("/api/workspaces/{id}/queue", get(chat::list).put(chat::put))
        .route("/api/workspaces/{id}/queue/{qid}", delete(chat::remove))
        .route(
            "/api/workspaces/{id}/queue/{qid}/send",
            post(chat::send_now),
        )
        .route("/api/workspaces/{id}/retry", post(chat::retry))
        .route("/api/workspaces/{id}/wait", get(chat::wait))
        .route("/api/analytics", get(analytics::overview))
        .route("/api/workspaces/{id}/git", get(git::status))
        .route("/api/workspaces/{id}/git/branches", get(git::branches))
        .route("/api/workspaces/{id}/git/describe", post(git::describe))
        .route("/api/workspaces/{id}/git/{op}", post(git::op))
        .route("/api/workspaces/{id}/pr", get(git::pr))
        .route("/api/workspaces/{id}/history", get(api::workspace_history))
        .route("/api/workspaces/{id}/prompt", post(api::prompt))
        .route("/api/workspaces/{id}/context", get(api::get_context))
        .route("/api/workspaces/{id}/context/{kind}", put(api::put_context))
        .route("/api/sessions/{id}/events", get(api::session_events))
        .route("/api/sessions/{id}/interrupt", post(api::interrupt))
        .route("/api/usage", get(api::usage))
        .route("/api/workspaces/{id}/detail", get(api::workspace_detail))
        .route("/api/search", get(api::search))
        .route("/api/workspaces/{id}/terminal/ws", get(api::terminal_ws))
        .route("/api/ws", get(api::ws_handler))
        .layer(axum::middleware::from_fn(same_origin_only))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(st)
}

async fn same_origin_only(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned);
    if let Some(host) = &host
        && !host_allowed(host)
    {
        return (
            StatusCode::MISDIRECTED_REQUEST,
            format!("不接受经 {host} 访问；如需经域名访问，设置 BLAZAR_ALLOWED_HOSTS"),
        )
            .into_response();
    }
    if let Some(origin) = req.headers().get(header::ORIGIN) {
        let same = origin
            .to_str()
            .ok()
            .and_then(|o| o.split_once("://"))
            .zip(host.as_deref())
            .is_some_and(|((_, o), h)| o.eq_ignore_ascii_case(h));
        if !same {
            return (StatusCode::FORBIDDEN, "拒绝跨站请求").into_response();
        }
    }
    next.run(req).await
}

fn host_allowed(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    if name.eq_ignore_ascii_case("localhost") || name.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    std::env::var("BLAZAR_ALLOWED_HOSTS").is_ok_and(|list| {
        list.split(',')
            .map(str::trim)
            .any(|h| !h.is_empty() && h.eq_ignore_ascii_case(name))
    })
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
    let router = build_router(st.clone());
    tokio::spawn(async move {
        if let Err(err) = serve(listener, router).await {
            tracing::error!("hub 服务退出: {err:#}");
        }
    });
    Ok((addr, st))
}

async fn index() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CACHE_CONTROL, "no-cache")],
        axum::response::Html(versioned_index()),
    )
}

static INDEX_VERSIONED: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn versioned_index() -> &'static str {
    INDEX_VERSIONED.get_or_init(|| {
        let mut out = String::with_capacity(INDEX_HTML.len() + 2048);
        let mut rest = INDEX_HTML;
        while let Some(i) = rest.find("=\"/") {
            let (head, tail) = rest.split_at(i + 2);
            out.push_str(head);
            let end = tail.find('"').unwrap_or(tail.len());
            let path = &tail[..end];
            let rel = path.trim_start_matches('/');
            out.push_str(path);
            if (rel.starts_with("js/") || rel.starts_with("css/"))
                && !path.contains('?')
                && ASSETS.get_file(rel).is_some()
            {
                out.push_str("?v=");
                out.push_str(asset_tag(rel).trim_matches('"'));
            }
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    })
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

    let mime = match path.rsplit('.').next().unwrap_or("") {
        "js" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "ttf" => "font/ttf",
        "woff2" => "font/woff2",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "map" => "application/json",
        _ => "application/octet-stream",
    };
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "no-cache"),
            (header::ETAG, tag),
        ],
        file.contents(),
    )
        .into_response()
}
