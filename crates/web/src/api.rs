//! hub 的 HTTP 接口。请求都发同源的 `/api/...`：发布时页面就是 hub 给的，开发时 `trunk serve` 把 `/api` 代理到 hub。

use gloo_net::http::{Request, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;

pub use blazar_core_types::api::{StateSnapshot, WorkspaceView};

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
        // hub 出错时回 {"error": "..."}，拿不到就报状态码。
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
    read(Request::get(path).send().await).await
}

pub async fn send<T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<T, ApiError> {
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
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Runtime {
    pub id: String,
    pub label: String,
    pub version: Option<String>,
    pub authed: Option<bool>,
    pub installed: bool,
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
}

impl Accounts {
    /// 这个运行时现在用的账号：没选过（""）就是自带的那个 `<provider>-default`；旧版的「自动」不算任何一个。
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

pub use blazar_core_types::api::{ChangeKind, DiffStat, FileContent, TreeEntry, Written};

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

/// `expect_mtime` 为 None 表示强制覆盖。
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

/// 记下这个工作区在编辑器里开着哪个文件（agent 能看到，下次进来也从这里接着看）。
pub async fn put_editor_context(ws: &str, path: &str) {
    let _ = send::<serde_json::Value>(
        "PUT",
        &format!("/api/workspaces/{ws}/context/editor"),
        &serde_json::json!({ "file": path }),
    )
    .await;
}
