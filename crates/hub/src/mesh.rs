use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use blazar_netmesh::config_import::{ConfigSummary, ImportedConfig};
use blazar_netmesh::engine::{
    self, BundledEngine, Elevation, EngineLayout, ExternalHints, LocalMeshStatus,
};
use blazar_netmesh::invite::{self, Invite, InviteDraft, InviteSummary};
use blazar_netmesh::{CliInvocation, EasyTierMesh, MeshAdmin};
use blazar_transport::{LocalTransport, NodeTransport, SshTransport};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::api::Shared;
use crate::state::ServerEvent;

pub struct MeshCtx {
    pub layout: EngineLayout,
    pub bundle: Option<BundledEngine>,

    pub staging_dir: PathBuf,

    pending: Mutex<Option<String>>,

    op: tokio::sync::Mutex<()>,

    external_rpc: Mutex<Option<String>>,

    issuer: Mutex<Option<IssuerConfig>>,
}

impl MeshCtx {
    #[must_use]
    pub fn new(engine_dir: Option<&std::path::Path>, staging_dir: PathBuf) -> Self {
        Self {
            layout: EngineLayout::system(),
            bundle: BundledEngine::locate(engine_dir),
            staging_dir,
            pending: Mutex::new(None),
            op: tokio::sync::Mutex::new(()),
            external_rpc: Mutex::new(None),
            issuer: Mutex::new(None),
        }
    }

    pub async fn warm(st: &Shared) {
        let _ = issuer_config(st).await;
        let _ = local_status(st).await;
        discover_now(st).await;
    }

    #[must_use]
    pub fn topology_label(&self, startup_via: &str) -> Option<String> {
        if startup_via != "local" {
            return Some(format!("启动参数指定的 {startup_via}"));
        }
        if let Some(cfg) = self
            .issuer
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .filter(|c| c.via != "local")
        {
            return Some(format!(
                "签发节点 {}{}",
                cfg.via,
                cfg.container
                    .map(|c| format!("（容器 {c}）"))
                    .unwrap_or_default()
            ));
        }
        if self.layout.joined() {
            return Some("本机 Blazar 组网引擎".into());
        }
        self.external_rpc
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .map(|rpc| format!("本机 EasyTier 的 RPC（{rpc}）"))
    }

    pub fn set_pending_invite(&self, text: String) {
        if let Ok(mut p) = self.pending.lock() {
            *p = Some(text);
        }
    }

    fn pending(&self) -> Option<String> {
        self.pending.lock().ok().and_then(|p| p.clone())
    }

    #[must_use]
    pub fn topology_source(&self) -> Option<(String, CliInvocation)> {
        if let Some(cfg) = self
            .issuer
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .filter(|c| c.via != "local")
        {
            let inv = match (&cfg.container, &cfg.cli) {
                (Some(c), Some(p)) => CliInvocation::Docker {
                    container: c.clone(),
                    program: p.clone(),
                },
                (Some(c), None) => CliInvocation::docker(c),
                (None, Some(p)) => CliInvocation::Direct {
                    program: p.clone(),
                    rpc: None,
                },
                (None, None) => CliInvocation::direct(),
            };
            return Some((cfg.via, inv));
        }
        if self.layout.joined() {
            return Some(("local".into(), self.layout.invocation()));
        }
        let rpc = self.external_rpc.lock().ok().and_then(|g| g.clone())?;
        Some((
            "local".into(),
            CliInvocation::Direct {
                program: engine::cli_program(&self.layout, self.bundle.as_ref()),
                rpc: Some(rpc),
            },
        ))
    }

    fn clear_pending(&self) {
        if let Ok(mut p) = self.pending.lock() {
            *p = None;
        }
    }
}

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(target: "blazar::mesh", "{e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn now() -> i64 {
    Utc::now().timestamp()
}

pub struct Caller(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}

impl Caller {
    fn is_local(&self) -> bool {
        self.0.is_none_or(|a| a.ip().is_loopback())
    }
}

async fn setting(st: &Shared, key: &str) -> Option<String> {
    sqlx::query("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<String, _>("value").ok())
}

async fn set_setting(st: &Shared, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await
    .map(|_| ())
}

const DEFAULT_RPC: &str = "127.0.0.1:15888";

async fn external_rpc_setting(st: &Shared) -> Option<String> {
    setting(st, "mesh.external_rpc")
        .await
        .filter(|s| !s.is_empty())
}

async fn discover_now(st: &Shared) {
    if st.mesh_ctx.topology_label(&st.mesh_via).is_none() {
        return;
    }
    if crate::api::refresh_mesh(State(st.clone())).await.is_ok() {
        st.emit(ServerEvent::NodesChanged);
    }
}

#[derive(Serialize)]
pub struct TopologyView {
    source: Option<String>,
}

pub async fn topology(State(st): State<Shared>) -> Response {
    Json(TopologyView {
        source: st.mesh_ctx.topology_label(&st.mesh_via),
    })
    .into_response()
}

pub(crate) async fn local_status(st: &Shared) -> LocalMeshStatus {
    let mesh_ips: Vec<Ipv4Addr> = sqlx::query_scalar::<_, String>(
        "SELECT endpoint FROM nodes WHERE network = 'easytier' AND endpoint IS NOT NULL",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .filter_map(|e| invite::parse_ipv4_cidr(e).ok().map(|x| x.0))
    .collect();
    let mut rpc_candidates: Vec<String> = external_rpc_setting(st).await.into_iter().collect();
    if !rpc_candidates.iter().any(|r| r == DEFAULT_RPC) {
        rpc_candidates.push(DEFAULT_RPC.into());
    }
    let status = engine::local_status(
        &st.mesh_ctx.layout,
        st.mesh_ctx.bundle.as_ref(),
        &ExternalHints {
            mesh_ips,
            rpc_candidates,
        },
    )
    .await;
    if let Ok(mut g) = st.mesh_ctx.external_rpc.lock() {
        *g = status.external.as_ref().and_then(|e| e.rpc.clone());
    }
    status
}

#[derive(Deserialize)]
pub struct ExternalRpcBody {
    rpc: String,
}

pub async fn set_external_rpc(
    State(st): State<Shared>,
    Json(body): Json<ExternalRpcBody>,
) -> Response {
    let rpc = body.rpc.trim();

    let ok = rpc.is_empty()
        || rpc
            .parse::<SocketAddr>()
            .is_ok_and(|a| a.ip().is_loopback());
    if !ok {
        return fail(
            StatusCode::BAD_REQUEST,
            "RPC 地址应为本机回环地址，如 127.0.0.1:15888",
        );
    }
    if let Err(e) = set_setting(&st, "mesh.external_rpc", rpc).await {
        return internal(e);
    }
    local(State(st)).await
}

#[derive(Serialize)]
pub struct Preview {
    summary: InviteSummary,

    warnings: Vec<String>,

    can_join: bool,
}

async fn preview_of(st: &Shared, inv: &Invite) -> Preview {
    let status = local_status(st).await;
    let mut warnings = Vec::new();
    if status.joined {
        warnings.push(format!(
            "本机已经在组网「{}」里；继续会换成这份邀请的身份。",
            status
                .node
                .as_ref()
                .and_then(|n| n.network_name.clone())
                .unwrap_or_else(|| "未知".into())
        ));
    }
    if let Some(ext) = &status.external {
        warnings.push(format!(
            "本机已通过 {} 在网里，一般不必再加入；要改由 Blazar 管理，请先退出它。",
            ext.label
        ));
    }
    let days_left = (inv.expires_at - now()) / 86_400;
    if days_left < 7 {
        warnings.push(format!("{days_left} 天后到期，到期后本机会掉线。"));
    }
    if inv.ipv4.is_none() {
        warnings.push("地址由网络自动分配，重连后可能变化。".into());
    }
    Preview {
        summary: inv.summary(),
        warnings,
        can_join: st.mesh_ctx.bundle.is_some(),
    }
}

#[derive(Serialize)]
pub struct LocalView {
    status: LocalMeshStatus,

    pending: Option<Preview>,

    pending_error: Option<String>,
}

pub async fn local(State(st): State<Shared>) -> Response {
    let status = local_status(&st).await;
    let (pending, pending_error) = match st.mesh_ctx.pending() {
        None => (None, None),
        Some(text) => match Invite::parse(&text, now()) {
            Ok(inv) => (Some(preview_of(&st, &inv).await), None),
            Err(e) => {
                st.mesh_ctx.clear_pending();
                (None, Some(e.to_string()))
            }
        },
    };
    Json(LocalView {
        status,
        pending,
        pending_error,
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct InviteBody {
    #[serde(default)]
    invite: Option<String>,
}

fn invite_text(st: &Shared, body: &InviteBody) -> Result<String, Box<Response>> {
    body.invite
        .clone()
        .or_else(|| st.mesh_ctx.pending())
        .ok_or_else(|| Box::new(fail(StatusCode::BAD_REQUEST, "没有收到邀请文件")))
}

pub async fn preview(State(st): State<Shared>, Json(body): Json<InviteBody>) -> Response {
    let text = match invite_text(&st, &body) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    match Invite::parse(&text, now()) {
        Ok(inv) => Json(preview_of(&st, &inv).await).into_response(),
        Err(e) => fail(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

pub async fn join(
    caller: Caller,
    State(st): State<Shared>,
    Json(body): Json<InviteBody>,
) -> Response {
    if !caller.is_local() {
        return fail(StatusCode::FORBIDDEN, "只能在本机操作加入组网");
    }
    let text = match invite_text(&st, &body) {
        Ok(t) => t,
        Err(r) => return *r,
    };
    let inv = match Invite::parse(&text, now()) {
        Ok(i) => i,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let Some(bundle) = st.mesh_ctx.bundle.clone() else {
        return fail(
            StatusCode::CONFLICT,
            "这个 Blazar 安装包没有带组网引擎，请安装完整版桌面端，或在终端用 `blazar join <文件>` 并用 --engine 指定引擎目录",
        );
    };
    let Ok(_guard) = st.mesh_ctx.op.try_lock() else {
        return fail(StatusCode::CONFLICT, "已有一个加入 / 退出操作在进行");
    };
    tracing::info!(target: "blazar::mesh", "加入组网 {}（{}）", inv.network_name, inv.hostname);
    match engine::join(
        &inv,
        &bundle,
        &st.mesh_ctx.layout,
        &st.mesh_ctx.staging_dir,
        Elevation::Gui,
    )
    .await
    {
        Ok(status) => {
            st.mesh_ctx.clear_pending();
            after_topology_change(&st);
            Json(status).into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct ConfigBody {
    toml: String,
}

#[derive(Serialize)]
pub struct ConfigPreview {
    summary: ConfigSummary,
    can_join: bool,
}

async fn config_preview_of(st: &Shared, cfg: &ImportedConfig) -> ConfigPreview {
    let mut summary = cfg.summary.clone();
    let status = local_status(st).await;
    if status.joined {
        summary
            .warnings
            .insert(0, "将替换当前配置并重启引擎。".into());
    }
    if let Some(ext) = &status.external {
        summary.warnings.push(format!(
            "本机已通过 {} 在网里；要改由 Blazar 管理，请先退出它。",
            ext.label
        ));
    }
    ConfigPreview {
        summary,
        can_join: st.mesh_ctx.bundle.is_some(),
    }
}

pub async fn config_preview(State(st): State<Shared>, Json(body): Json<ConfigBody>) -> Response {
    match ImportedConfig::parse(&body.toml) {
        Ok(cfg) => Json(config_preview_of(&st, &cfg).await).into_response(),
        Err(e) => fail(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

pub async fn config_apply(
    caller: Caller,
    State(st): State<Shared>,
    Json(body): Json<ConfigBody>,
) -> Response {
    if !caller.is_local() {
        return fail(StatusCode::FORBIDDEN, "只能在本机修改组网配置");
    }
    let cfg = match ImportedConfig::parse(&body.toml) {
        Ok(c) => c,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let Some(bundle) = st.mesh_ctx.bundle.clone() else {
        return fail(StatusCode::CONFLICT, "这个 Blazar 安装包没有带组网引擎");
    };
    let Ok(_guard) = st.mesh_ctx.op.try_lock() else {
        return fail(StatusCode::CONFLICT, "已有一个加入 / 退出操作在进行");
    };
    tracing::info!(target: "blazar::mesh", "按导入的配置加入组网 {}", cfg.summary.network_name);
    match engine::install(
        cfg.engine_config(),
        &cfg.summary.network_name,
        &bundle,
        &st.mesh_ctx.layout,
        &st.mesh_ctx.staging_dir,
        Elevation::Gui,
    )
    .await
    {
        Ok(status) => {
            after_topology_change(&st);
            Json(status).into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

pub async fn config_get(caller: Caller, State(st): State<Shared>) -> Response {
    if !caller.is_local() {
        return fail(StatusCode::FORBIDDEN, "组网配置含密钥，只能在本机查看");
    }
    if !st.mesh_ctx.layout.joined() {
        return fail(StatusCode::NOT_FOUND, "本机还没有加入组网");
    }
    match engine::running_config(&st.mesh_ctx.layout).await {
        Ok(toml) => Json(serde_json::json!({ "toml": toml })).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

pub async fn dismiss(State(st): State<Shared>) -> Response {
    st.mesh_ctx.clear_pending();
    StatusCode::NO_CONTENT.into_response()
}

pub async fn leave(caller: Caller, State(st): State<Shared>) -> Response {
    if !caller.is_local() {
        return fail(StatusCode::FORBIDDEN, "只能在本机操作退出组网");
    }
    let Ok(_guard) = st.mesh_ctx.op.try_lock() else {
        return fail(StatusCode::CONFLICT, "已有一个加入 / 退出操作在进行");
    };
    match engine::leave(
        &st.mesh_ctx.layout,
        &st.mesh_ctx.staging_dir,
        Elevation::Gui,
    )
    .await
    {
        Ok(()) => {
            after_topology_change(&st);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

fn after_topology_change(st: &Shared) {
    let st = st.clone();
    tokio::spawn(async move {
        for delay in [2u64, 8] {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            let _ = crate::api::refresh_mesh(State(st.clone())).await;
            st.emit(ServerEvent::NodesChanged);
        }
    });
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuerConfig {
    pub via: String,

    #[serde(default)]
    pub container: Option<String>,

    #[serde(default)]
    pub cli: Option<String>,

    #[serde(default)]
    pub entry_points: Vec<String>,
}

async fn issuer_config(st: &Shared) -> Option<IssuerConfig> {
    if let Some(cfg) = setting(st, "mesh.issuer")
        .await
        .and_then(|v| serde_json::from_str::<IssuerConfig>(&v).ok())
    {
        if let Ok(mut g) = st.mesh_ctx.issuer.lock() {
            *g = Some(cfg.clone());
        }
        return Some(cfg);
    }
    if let Ok(mut g) = st.mesh_ctx.issuer.lock() {
        *g = None;
    }
    (st.mesh_via != "local").then(|| IssuerConfig {
        via: st.mesh_via.clone(),
        container: st.mesh_container.clone(),
        cli: None,
        entry_points: Vec::new(),
    })
}

const NO_ISSUER: &str =
    "还没有配置签发节点：在「机器与组网」→「邀请同事」→「签发设置」里填上持有网络密钥的那台机器";

fn issuer_admin(st: &Shared, cfg: &IssuerConfig) -> (MeshAdmin, EasyTierMesh) {
    let transport: Arc<dyn NodeTransport> = if cfg.via == "local" {
        Arc::new(LocalTransport)
    } else {
        Arc::new(SshTransport::new(&cfg.via))
    };
    let invocation = match (&cfg.container, cfg.via.as_str()) {
        (Some(c), _) => match &cfg.cli {
            Some(program) => CliInvocation::Docker {
                container: c.clone(),
                program: program.clone(),
            },
            None => CliInvocation::docker(c),
        },
        (None, _) if cfg.cli.is_some() => CliInvocation::Direct {
            program: cfg.cli.clone().unwrap_or_default(),
            rpc: None,
        },

        (None, "local") if !st.mesh_ctx.layout.joined() => CliInvocation::direct(),
        (None, "local") => st.mesh_ctx.layout.invocation(),
        (None, _) => CliInvocation::direct(),
    };
    (
        MeshAdmin::new(transport.clone(), invocation.clone()),
        EasyTierMesh::new(transport, invocation),
    )
}

#[derive(Serialize)]
pub struct IssuerView {
    configured: bool,
    config: Option<IssuerConfig>,
    reachable: bool,
    error: Option<String>,
    network_name: Option<String>,

    subnet: Option<String>,

    entry_points: Vec<String>,
}

pub async fn issuer(State(st): State<Shared>) -> Response {
    let Some(cfg) = issuer_config(&st).await else {
        return Json(IssuerView {
            configured: false,
            config: None,
            reachable: false,
            error: None,
            network_name: None,
            subnet: None,
            entry_points: Vec::new(),
        })
        .into_response();
    };
    let (admin, _) = issuer_admin(&st, &cfg);
    let view = match tokio::time::timeout(Duration::from_secs(15), admin.status()).await {
        Ok(Ok(node)) => IssuerView {
            configured: true,
            entry_points: if cfg.entry_points.is_empty() {
                node.entry_points()
            } else {
                cfg.entry_points.clone()
            },
            network_name: node.network_name.clone(),
            subnet: Some(node.virtual_ipv4.clone()).filter(|s| !s.is_empty()),
            reachable: true,
            error: None,
            config: Some(cfg),
        },
        Ok(Err(e)) => IssuerView {
            configured: true,
            entry_points: cfg.entry_points.clone(),
            config: Some(cfg),
            reachable: false,
            error: Some(e.to_string()),
            network_name: None,
            subnet: None,
        },
        Err(_) => IssuerView {
            configured: true,
            entry_points: cfg.entry_points.clone(),
            config: Some(cfg),
            reachable: false,
            error: Some("15 秒内没有响应".into()),
            network_name: None,
            subnet: None,
        },
    };
    Json(view).into_response()
}

pub async fn clear_issuer(State(st): State<Shared>) -> Response {
    let r = sqlx::query("DELETE FROM settings WHERE key = 'mesh.issuer'")
        .execute(st.db.pool())
        .await;
    match r {
        Ok(_) => {
            if let Ok(mut g) = st.mesh_ctx.issuer.lock() {
                *g = None;
            }
            let bg = st.clone();
            tokio::spawn(async move { discover_now(&bg).await });
            issuer(State(st)).await
        }
        Err(e) => internal(e),
    }
}

pub async fn set_issuer(State(st): State<Shared>, Json(cfg): Json<IssuerConfig>) -> Response {
    let via_ok = !cfg.via.is_empty()
        && cfg
            .via
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'));
    let container_ok = cfg.container.as_deref().is_none_or(|c| {
        !c.is_empty()
            && c.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    });

    let cli_ok = cfg.cli.as_deref().is_none_or(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
    });
    if !via_ok || !container_ok || !cli_ok {
        return fail(
            StatusCode::BAD_REQUEST,
            "主机名、容器名或 CLI 路径含非法字符",
        );
    }
    for p in &cfg.entry_points {
        if let Err(e) = invite::validate_peer_uri(p) {
            return fail(StatusCode::BAD_REQUEST, e.to_string());
        }
    }
    let value = serde_json::to_string(&cfg).expect("可序列化");
    match set_setting(&st, "mesh.issuer", &value).await {
        Ok(()) => {
            if let Ok(mut g) = st.mesh_ctx.issuer.lock() {
                *g = Some(cfg);
            }

            let bg = st.clone();
            tokio::spawn(async move { discover_now(&bg).await });
            issuer(State(st)).await
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct IssueBody {
    member: String,

    #[serde(default)]
    hostname: Option<String>,

    #[serde(default = "default_days")]
    days: i64,

    #[serde(default)]
    ipv4: Option<String>,
    #[serde(default)]
    note: String,
}

fn default_days() -> i64 {
    180
}

fn hostname_from(member: &str) -> String {
    let mut out = String::new();
    for c in member.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

#[derive(Serialize)]
pub struct Issued {
    file_name: String,

    content: String,
    summary: InviteSummary,
}

pub async fn issue(State(st): State<Shared>, Json(body): Json<IssueBody>) -> Response {
    let member = body.member.trim().to_owned();
    if member.is_empty() || member.chars().count() > 40 || member.chars().any(char::is_control) {
        return fail(StatusCode::BAD_REQUEST, "请填写同事称呼（40 字以内）");
    }
    if !(1..=730).contains(&body.days) {
        return fail(StatusCode::BAD_REQUEST, "有效期应在 1–730 天");
    }
    let hostname = body
        .hostname
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map_or_else(|| hostname_from(&member), str::to_owned);
    if hostname.is_empty() {
        return fail(
            StatusCode::BAD_REQUEST,
            "称呼里没有可用作主机名的字母数字，请另外填写主机名（如 zhangsan-mbp）",
        );
    }

    let Some(cfg) = issuer_config(&st).await else {
        return fail(StatusCode::BAD_REQUEST, NO_ISSUER);
    };
    let (admin, mesh) = issuer_admin(&st, &cfg);
    let node = match admin.status().await {
        Ok(n) => n,
        Err(e) => {
            return fail(
                StatusCode::BAD_GATEWAY,
                format!("连不上签发节点 {}：{e}", cfg.via),
            );
        }
    };
    let Some(network_name) = node.network_name.clone() else {
        return fail(StatusCode::BAD_GATEWAY, "读不到签发节点的网络名");
    };
    let entry_points = if cfg.entry_points.is_empty() {
        node.entry_points()
    } else {
        cfg.entry_points.clone()
    };
    if entry_points.is_empty() {
        return fail(
            StatusCode::BAD_REQUEST,
            "推导不出入网地址（签发节点没有公网映射），请在签发设置里填写，如 tcp://1.2.3.4:11010",
        );
    }

    let peers = mesh.peers().await.unwrap_or_default();
    let active = match active_invites(&st).await {
        Ok(a) => a,
        Err(e) => return internal(e),
    };
    if peers
        .iter()
        .any(|p| p.hostname.eq_ignore_ascii_case(&hostname))
        || active
            .iter()
            .any(|(h, _)| h.eq_ignore_ascii_case(&hostname))
    {
        return fail(
            StatusCode::CONFLICT,
            format!("主机名 {hostname} 已被占用（在线节点或未过期的邀请），请换一个"),
        );
    }

    let ipv4 =
        match body.ipv4.as_deref().map(str::trim).unwrap_or("auto") {
            "dhcp" => None,
            "auto" | "" => {
                let Ok((net, prefix)) = invite::parse_ipv4_cidr(&node.virtual_ipv4) else {
                    return fail(StatusCode::BAD_GATEWAY, "读不到签发节点的虚拟网段");
                };
                let mut taken: Vec<Ipv4Addr> = peers
                    .iter()
                    .filter_map(|p| invite::parse_ipv4_cidr(&p.ipv4).ok().map(|x| x.0))
                    .collect();
                taken.extend(active.iter().filter_map(|(_, ip)| {
                    invite::parse_ipv4_cidr(ip.as_deref()?).ok().map(|x| x.0)
                }));
                match invite::allocate_ipv4(net, prefix, &taken) {
                    Some(ip) => Some(format!("{ip}/{prefix}")),
                    None => return fail(StatusCode::CONFLICT, "网段里没有空闲地址了"),
                }
            }
            explicit => {
                if let Err(e) = invite::parse_ipv4_cidr(explicit) {
                    return fail(StatusCode::BAD_REQUEST, e.to_string());
                }
                Some(explicit.to_owned())
            }
        };

    let issued_at = now();
    let expires_at = issued_at + body.days * 86_400;
    let cred = match admin.issue_credential(body.days * 86_400).await {
        Ok(c) => c,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, format!("签发凭据失败：{e}")),
    };
    let draft = InviteDraft {
        network_name: network_name.clone(),
        peers: entry_points,
        hostname: hostname.clone(),
        ipv4: ipv4.clone(),
        issued_by: whoami(),
        note: body.note.trim().to_owned(),
    };
    let inv = match draft.seal(cred.id.clone(), cred.secret, issued_at, expires_at) {
        Ok(i) => i,
        Err(e) => {
            let _ = admin.revoke_credential(&cred.id).await;
            return fail(StatusCode::BAD_REQUEST, e.to_string());
        }
    };
    let res = sqlx::query(
        "INSERT INTO mesh_invites (id, member, hostname, ipv4, network_name, issued_by, note, issued_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&inv.credential_id)
    .bind(&member)
    .bind(&inv.hostname)
    .bind(&inv.ipv4)
    .bind(&inv.network_name)
    .bind(&inv.issued_by)
    .bind(&inv.note)
    .bind(issued_at)
    .bind(expires_at)
    .execute(st.db.pool())
    .await;
    if let Err(e) = res {
        let _ = admin.revoke_credential(&cred.id).await;
        return internal(e);
    }
    tracing::info!(target: "blazar::mesh", "签发邀请 {} → {member}（{hostname}）", inv.credential_id);
    Json(Issued {
        file_name: inv.file_name(),
        content: inv.to_file(),
        summary: inv.summary(),
    })
    .into_response()
}

async fn active_invites(st: &Shared) -> sqlx::Result<Vec<(String, Option<String>)>> {
    let rows = sqlx::query(
        "SELECT hostname, ipv4 FROM mesh_invites WHERE revoked_at IS NULL AND expires_at > ?1",
    )
    .bind(now())
    .fetch_all(st.db.pool())
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                r.try_get("hostname").unwrap_or_default(),
                r.try_get("ipv4").ok().flatten(),
            )
        })
        .collect())
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(64)
        .collect()
}

#[derive(Serialize)]
pub struct InviteRow {
    id: String,
    member: String,
    hostname: String,
    ipv4: Option<String>,
    network_name: String,
    issued_by: String,
    note: String,
    issued_at: i64,
    expires_at: i64,
    revoked_at: Option<i64>,

    state: &'static str,
}

pub async fn list_invites(State(st): State<Shared>) -> Response {
    let rows = match sqlx::query(
        "SELECT i.*, n.status AS node_status, n.last_seen_at
         FROM mesh_invites i LEFT JOIN nodes n ON n.name = i.hostname
         ORDER BY i.issued_at DESC",
    )
    .fetch_all(st.db.pool())
    .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };

    let live: Option<Vec<String>> = match issuer_config(&st).await {
        Some(cfg) => {
            let (admin, _) = issuer_admin(&st, &cfg);
            tokio::time::timeout(Duration::from_secs(10), admin.credentials())
                .await
                .ok()
                .and_then(Result::ok)
        }
        None => None,
    }
    .map(|v| v.into_iter().map(|c| c.credential_id).collect());
    let t = now();
    let out: Vec<InviteRow> = rows
        .iter()
        .map(|r| {
            let id: String = r.try_get("id").unwrap_or_default();
            let revoked_at: Option<i64> = r.try_get("revoked_at").ok().flatten();
            let expires_at: i64 = r.try_get("expires_at").unwrap_or_default();
            let node_status: Option<String> = r.try_get("node_status").ok().flatten();
            let state = if revoked_at.is_some() {
                "revoked"
            } else if expires_at <= t {
                "expired"
            } else if live.as_ref().is_some_and(|l| !l.contains(&id)) {
                "lost"
            } else if node_status.as_deref() == Some("online") {
                "online"
            } else if node_status.is_some() {
                "offline"
            } else {
                "waiting"
            };
            InviteRow {
                id,
                member: r.try_get("member").unwrap_or_default(),
                hostname: r.try_get("hostname").unwrap_or_default(),
                ipv4: r.try_get("ipv4").ok().flatten(),
                network_name: r.try_get("network_name").unwrap_or_default(),
                issued_by: r.try_get("issued_by").unwrap_or_default(),
                note: r.try_get("note").unwrap_or_default(),
                issued_at: r.try_get("issued_at").unwrap_or_default(),
                expires_at,
                revoked_at,
                state,
            }
        })
        .collect();
    Json(serde_json::json!({ "invites": out, "issuer_checked": live.is_some() })).into_response()
}

pub async fn revoke(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return fail(StatusCode::BAD_REQUEST, "邀请 ID 无效");
    }
    let exists = sqlx::query("SELECT 1 FROM mesh_invites WHERE id = ?1")
        .bind(&id)
        .fetch_optional(st.db.pool())
        .await;
    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return fail(StatusCode::NOT_FOUND, "没有这份邀请"),
        Err(e) => return internal(e),
    }
    let Some(cfg) = issuer_config(&st).await else {
        return fail(StatusCode::BAD_REQUEST, NO_ISSUER);
    };
    let (admin, _) = issuer_admin(&st, &cfg);
    if let Err(e) = admin.revoke_credential(&id).await {
        let still_there = admin
            .credentials()
            .await
            .map(|l| l.iter().any(|c| c.credential_id == id));
        if !matches!(still_there, Ok(false)) {
            return fail(StatusCode::BAD_GATEWAY, format!("吊销失败：{e}"));
        }
    }
    if let Err(e) = sqlx::query("UPDATE mesh_invites SET revoked_at = ?2 WHERE id = ?1")
        .bind(&id)
        .bind(now())
        .execute(st.db.pool())
        .await
    {
        return internal(e);
    }
    tracing::info!(target: "blazar::mesh", "吊销邀请 {id}");
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_from_member_name() {
        assert_eq!(hostname_from("Zhang San"), "zhang-san");
        assert_eq!(hostname_from("  li.si's MBP "), "li-si-s-mbp");
        assert_eq!(
            hostname_from("张三"),
            "",
            "纯中文推不出主机名，要求显式填写"
        );
        assert_eq!(hostname_from("王五 wangwu"), "wangwu");
    }

    #[test]
    fn remote_callers_cannot_trigger_elevation() {
        let remote = Caller(Some("10.99.0.5:5000".parse().unwrap()));
        let local = Caller(Some("127.0.0.1:5000".parse().unwrap()));
        assert!(!remote.is_local());
        assert!(local.is_local());
    }
}
