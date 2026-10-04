use gloo_net::http::{Request, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;

pub use blazar_core_types::api::WorkspaceView;

thread_local! {
    static SESSION_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn reset_session() {
    SESSION_READY.with(|ready| ready.set(false));
}

pub async fn session() -> Result<(), ApiError> {
    if SESSION_READY.with(std::cell::Cell::get) {
        return Ok(());
    }
    let response = Request::post("/api/auth/session")
        .send()
        .await
        .map_err(|e| ApiError(format!("连接 Blazar 失败：{e}")))?;
    if !response.ok() {
        return Err(ApiError("无法建立本机会话，请重新打开 Blazar".into()));
    }
    SESSION_READY.with(|ready| ready.set(true));
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

async fn read<T: DeserializeOwned>(resp: Result<Response, gloo_net::Error>) -> Result<T, ApiError> {
    let resp = resp.map_err(|e| ApiError(format!("连不上 hub：{e}")))?;
    if !resp.ok() {
        if resp.status() == 401 {
            reset_session();
        }
        let msg = resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("HTTP {}", resp.status()));
        return Err(ApiError(msg));
    }
    if resp.status() == 204 {
        return serde_json::from_value(serde_json::Value::Null)
            .map_err(|e| ApiError(format!("返回的数据看不懂：{e}")));
    }
    resp.json::<T>()
        .await
        .map_err(|e| ApiError(format!("返回的数据看不懂：{e}")))
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    session().await?;
    read(Request::get(path).send().await).await
}

pub async fn send<T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<T, ApiError> {
    session().await?;
    let b = match method {
        "PUT" => Request::put(path),
        "DELETE" => Request::delete(path),
        _ => Request::post(path),
    };
    let req = b
        .json(body)
        .map_err(|e| ApiError(format!("请求编不出来：{e}")))?;
    read(req.send().await).await
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Machine {
    pub hostname: String,
    #[serde(default)]
    pub blazar_version: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Runtime {
    pub id: String,
    pub label: String,
    pub version: Option<String>,
    pub authed: Option<bool>,
    pub installed: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub auth_hint: Option<String>,
    #[serde(default)]
    pub remote_hands: bool,
    #[serde(default)]
    pub cost_7d: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Runtimes {
    pub machine: Machine,
    pub runtimes: Vec<Runtime>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QuotaWindow {
    pub name: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub status: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub builtin: bool,
    pub kind: String,
    #[serde(default)]
    pub windows: Vec<QuotaWindow>,
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub config_dir: Option<String>,
    #[serde(default)]
    pub model_blocks: Vec<ModelBlock>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelBlock {
    pub model: String,
    #[serde(default)]
    pub observed_at: String,
}

impl Account {
    pub fn usable(&self) -> bool {
        !self.disabled && self.status != "logged_out"
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Accounts {
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub modes: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub root: String,
}

impl Accounts {
    pub fn active(&self, provider: &str) -> Option<String> {
        match self.modes.get(provider).map(String::as_str).unwrap_or("") {
            "" => Some(format!("{provider}-default")),
            "auto" => None,
            id => Some(id.to_owned()),
        }
    }
}

pub async fn use_account(a: &Account) -> Result<serde_json::Value, ApiError> {
    let mode = if a.builtin { "" } else { a.id.as_str() };
    send(
        "PUT",
        "/api/accounts/mode",
        &serde_json::json!({ "provider": a.provider, "mode": mode }),
    )
    .await
}

pub async fn refresh_quota(id: &str) -> Result<serde_json::Value, ApiError> {
    send(
        "POST",
        &format!("/api/accounts/{id}/quota"),
        &serde_json::json!({}),
    )
    .await
}

pub use blazar_core_types::api::{ChangeKind, DiffStat, DirItems, FileContent, TreeEntry, Written};

pub const MAX_DIR_ITEMS: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkspaceDetail {
    pub id: String,
    pub name: String,
    pub node: String,
    pub path: String,
    pub activity: String,
    #[serde(default)]
    pub isolated: bool,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
    #[serde(default)]
    pub truncated: bool,
    pub stat: Option<DiffStat>,
}

pub fn enc(s: &str) -> String {
    String::from(js_sys::encode_uri_component(s))
}

pub async fn read_file(ws: &str, path: &str) -> Result<FileContent, ApiError> {
    get(&format!("/api/workspaces/{ws}/file?path={}", enc(path))).await
}

pub async fn write_file(
    ws: &str,
    path: &str,
    content: &str,
    expect_mtime: Option<u64>,
) -> Result<Written, ApiError> {
    send(
        "PUT",
        &format!("/api/workspaces/{ws}/file"),
        &serde_json::json!({ "path": path, "content": content, "expect_mtime": expect_mtime }),
    )
    .await
}

pub async fn put_editor_context(ws: &str, path: &str) {
    let _ = send::<serde_json::Value>(
        "PUT",
        &format!("/api/workspaces/{ws}/context/editor"),
        &serde_json::json!({ "file": path }),
    )
    .await;
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DiffResp {
    #[serde(default)]
    pub diff: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GitFile {
    pub code: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GitCommit {
    pub sha: String,
    pub author: String,
    pub at: i64,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PrRef {
    pub url: String,
    pub number: Option<i64>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WebLinks {
    #[serde(default)]
    pub new_pr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct GitStatus {
    pub repo: bool,
    pub reason: String,
    pub branch: String,
    pub detached: bool,
    pub head: String,
    pub target: String,
    pub target_guess: bool,
    pub target_ok: bool,
    pub target_local: bool,
    pub same: bool,
    pub ahead: i64,
    pub behind: i64,
    pub upstream: String,
    pub up_ahead: i64,
    pub up_behind: i64,
    pub remote: String,
    pub web: Option<WebLinks>,
    pub op: Option<String>,
    pub gh: bool,
    pub uncommitted: i64,
    pub conflicts: Vec<String>,
    pub files: Vec<GitFile>,
    pub commits: Vec<GitCommit>,
    pub running: i64,
    pub pr: Option<PrRef>,
}

impl GitStatus {
    pub fn has_target(&self) -> bool {
        self.target_ok && !self.same
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct GitOpResult {
    pub ok: bool,
    pub output: String,
    pub status: Option<GitStatus>,
    pub needs_force: bool,
    pub tasks_done: i64,
    pub changed: bool,
    pub commit: Option<String>,
    pub url: Option<String>,
    pub existing: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct PrChecks {
    pub pass: i64,
    pub failed: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct PrDetail {
    pub url: String,
    pub number: Option<i64>,
    pub state: String,
    pub title: String,
    pub draft: bool,
    pub mergeable: String,
    pub review: String,
    pub checks: PrChecks,
}
