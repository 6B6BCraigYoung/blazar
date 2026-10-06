use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_core_types::SessionId;
use blazar_runtime::McpServerSpec;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::api::Shared;

pub const PATH: &str = "/mcp/fleet";

const PROTOCOL_VERSION: &str = "2025-06-18";
const TUNNEL_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Clone)]
pub struct Grant {
    pub session: SessionId,
    pub node: String,
}

struct Tunnel {
    port: u16,
    child: tokio::process::Child,
}

#[derive(Default)]
pub struct FleetState {
    grants: Mutex<HashMap<String, Grant>>,
    tunnels: tokio::sync::Mutex<HashMap<String, Tunnel>>,
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

impl FleetState {
    pub fn issue(&self, session: SessionId, node: &str) -> anyhow::Result<String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow::anyhow!("生成调度令牌失败：{error}"))?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        grants.retain(|_, grant| grant.session != session);
        grants.insert(
            token.clone(),
            Grant {
                session,
                node: node.to_owned(),
            },
        );
        Ok(token)
    }

    #[must_use]
    pub fn grant(&self, token: &str) -> Option<Grant> {
        let grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        grants
            .iter()
            .find(|(issued, _)| same(issued, token))
            .map(|(_, grant)| grant.clone())
    }

    pub async fn revoke(&self, session: SessionId) {
        let node = {
            let mut grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
            let node = grants
                .values()
                .find(|grant| grant.session == session)
                .map(|grant| grant.node.clone());
            grants.retain(|_, grant| grant.session != session);
            let still_used = node
                .as_ref()
                .is_some_and(|n| grants.values().any(|grant| &grant.node == n));
            if still_used { None } else { node }
        };
        if let Some(node) = node
            && node != "local"
        {
            self.tunnels.lock().await.remove(&node);
        }
    }

    pub async fn tunnel_port(&self, node: &str, hub_port: u16) -> anyhow::Result<u16> {
        let mut tunnels = self.tunnels.lock().await;
        if let Some(tunnel) = tunnels.get_mut(node) {
            if matches!(tunnel.child.try_wait(), Ok(None)) {
                return Ok(tunnel.port);
            }
            tunnels.remove(node);
        }
        let tunnel = open_tunnel(node, hub_port).await?;
        let port = tunnel.port;
        tunnels.insert(node.to_owned(), tunnel);
        Ok(port)
    }
}

fn allocated_port(line: &str) -> Option<u16> {
    line.trim()
        .strip_prefix("Allocated port ")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

async fn open_tunnel(node: &str, hub_port: u16) -> anyhow::Result<Tunnel> {
    blazar_transport::validate_ssh_target(node).map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "ConnectTimeout=10",
            "-o",
            "BatchMode=yes",
            "-o",
            "NumberOfPasswordPrompts=0",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ExitOnForwardFailure=yes",
            "-N",
            "-R",
        ])
        .arg(format!("127.0.0.1:0:127.0.0.1:{hub_port}"))
        .arg("--")
        .arg(node)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("起不了 ssh")?;
    let stderr = child.stderr.take().context("拿不到 ssh 的输出")?;
    let mut lines = BufReader::new(stderr).lines();
    let port = tokio::time::timeout(TUNNEL_TIMEOUT, async {
        let mut last = String::new();
        while let Some(line) = lines.next_line().await? {
            if let Some(port) = allocated_port(&line) {
                return Ok(port);
            }
            if !line.trim().is_empty() {
                last = line;
            }
        }
        anyhow::bail!("ssh 没有建立反向转发就退出了：{last}")
    })
    .await
    .map_err(|_| anyhow::anyhow!("等 {node} 分配回程端口超时"))??;
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    Ok(Tunnel { port, child })
}

pub async fn enabled(st: &Shared) -> bool {
    crate::office::prefs(st).await["fleet"]["enabled"]
        .as_bool()
        .unwrap_or(false)
}

pub async fn mcp_spec(
    st: &Shared,
    session: SessionId,
    node: &str,
) -> anyhow::Result<Option<McpServerSpec>> {
    if !enabled(st).await {
        return Ok(None);
    }
    let base = crate::office::HUB_URL.get().context("hub 地址还没就绪")?;
    let url = if node == "local" {
        format!("{base}{PATH}")
    } else {
        let hub_port: u16 = base
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .context("hub 地址里没有端口")?;
        let port = st.fleet.tunnel_port(node, hub_port).await?;
        format!("http://127.0.0.1:{port}{PATH}")
    };
    let token = st.fleet.issue(session, node)?;
    Ok(Some(McpServerSpec {
        name: blazar_mcp::fleet::SERVER_NAME.into(),
        http: true,
        url,
        headers: BTreeMap::from([("Authorization".to_owned(), format!("Bearer {token}"))]),
        ..Default::default()
    }))
}

pub async fn mcp_http(State(st): State<Shared>, body: Bytes) -> Response {
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "无法解析 JSON-RPC").into_response();
    };
    let messages = match request {
        Value::Array(list) => list,
        single => vec![single],
    };
    let mut replies = Vec::new();
    for message in &messages {
        if let Some(reply) = handle(&st, message).await {
            replies.push(reply);
        }
    }
    if replies.is_empty() {
        return StatusCode::ACCEPTED.into_response();
    }
    if replies.len() == 1 && !matches!(serde_json::from_slice::<Value>(&body), Ok(Value::Array(_)))
    {
        return Json(replies.remove(0)).into_response();
    }
    Json(Value::Array(replies)).into_response()
}

async fn handle(st: &Shared, req: &Value) -> Option<Value> {
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = req.get("id").cloned()?;
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": blazar_mcp::fleet::SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => Ok(blazar_mcp::tools::manifest()),
        "tools/call" => call(st, req.get("params")).await,
        "ping" => Ok(json!({})),
        other => Err(format!("不支持的方法: {other}")),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": -32603, "message": message }
        }),
    })
}

async fn call(st: &Shared, params: Option<&Value>) -> Result<Value, String> {
    let params = params.ok_or("tools/call 缺少 params")?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("缺少工具名")?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let tool = blazar_mcp::tools::find(name).ok_or_else(|| format!("未知工具: {name}"))?;
    let (method, path, body) = (tool.build)(&args).map_err(|error| error.to_string())?;
    let (ok, text) = internal_request(st, method, &path, body).await?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": !ok,
    }))
}

async fn internal_request(
    st: &Shared,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(bool, String), String> {
    let base = crate::office::HUB_URL.get().ok_or("hub 地址还没就绪")?;
    let token = st.auth.get().ok_or("hub 会话还没就绪")?.token();
    let method: reqwest::Method = method
        .parse()
        .map_err(|_| format!("不支持的方法 {method}"))?;
    let mut request = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request
            .header("Content-Type", "application/json")
            .body(body.to_string());
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("连不上 hub：{error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("读 hub 回复失败：{error}"))?;
    if status.is_success() {
        Ok((true, text))
    } else {
        Ok((false, format!("hub 返回 {status}: {}", text.trim())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_port_ssh_allocated() {
        assert_eq!(
            allocated_port("Allocated port 43211 for remote forward to 127.0.0.1:50870"),
            Some(43211)
        );
        assert_eq!(
            allocated_port("Warning: Permanently added 'x' to the list of known hosts."),
            None
        );
        assert_eq!(allocated_port("Allocated port x for remote forward"), None);
    }

    #[test]
    fn tokens_are_per_session_and_can_be_revoked() {
        let fleet = FleetState::default();
        let a = SessionId::new();
        let b = SessionId::new();
        let first = fleet.issue(a, "gpu-1").unwrap();
        let second = fleet.issue(a, "gpu-1").unwrap();
        assert_ne!(first, second);
        assert!(fleet.grant(&first).is_none(), "重新签发后旧令牌作废");
        assert_eq!(fleet.grant(&second).unwrap().node, "gpu-1");
        let other = fleet.issue(b, "gpu-2").unwrap();
        assert!(fleet.grant(&other[..63]).is_none());
        assert!(fleet.grant(&format!("{other}0")).is_none());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(fleet.revoke(a));
        assert!(fleet.grant(&second).is_none());
        assert!(fleet.grant(&other).is_some());
    }

    #[tokio::test]
    async fn http_endpoint_speaks_json_rpc() {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
            crate::services::Services::Isolated,
        );
        let post = |body: Value| {
            let st = st.clone();
            async move {
                let response = mcp_http(State(st), Bytes::from(body.to_string())).await;
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
                    .await
                    .unwrap();
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };
        let (status, init) = post(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "blazar-fleet");
        let (status, _) =
            post(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let (_, list) = post(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await;
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"copy_files") && names.contains(&"send_prompt"));
        let (_, unknown) = post(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "nope" } })).await;
        assert_eq!(unknown["error"]["code"], -32603);
        let (_, batch) = post(json!([
            { "jsonrpc": "2.0", "id": 4, "method": "ping" },
            { "jsonrpc": "2.0", "method": "notifications/initialized" }
        ]))
        .await;
        assert_eq!(batch.as_array().unwrap().len(), 1);
        assert_eq!(batch[0]["id"], 4);
    }
}
