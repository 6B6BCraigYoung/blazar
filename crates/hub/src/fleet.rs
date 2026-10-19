use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use blazar_core_types::SessionId;
use blazar_runtime::McpServerSpec;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::api::Shared;
use crate::bots::Caller;

pub const PATH: &str = "/mcp/fleet";

const PROTOCOL_VERSION: &str = "2025-06-18";
const TUNNEL_TIMEOUT: Duration = Duration::from_secs(25);
const WATCHDOG_EVERY: Duration = Duration::from_secs(20);

const WORKER_WRITE_TOOLS: &[&str] = &["send_prompt", "copy_files", "wait_workspace"];
const WORKER_DENIED_TOOLS: &[&str] = &[
    "spawn_bot",
    "create_workspace",
    "create_task",
    "update_task",
    "start_task",
    "run_autopilot",
];

#[derive(Debug, Clone)]
pub struct Grant {
    pub session: SessionId,
    pub node: String,
    pub caller: Option<Caller>,
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

fn digest(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn tunnel_key(node: &str) -> String {
    format!("fleet.tunnel_port.{node}")
}

impl FleetState {
    pub fn issue(
        &self,
        session: SessionId,
        node: &str,
        caller: Option<Caller>,
    ) -> anyhow::Result<String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow::anyhow!("生成调度令牌失败：{error}"))?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        grants.retain(|_, grant| grant.session != session);
        grants.insert(
            digest(&token),
            Grant {
                session,
                node: node.to_owned(),
                caller,
            },
        );
        Ok(token)
    }

    #[must_use]
    pub fn grant(&self, token: &str) -> Option<Grant> {
        let grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        grants.get(&digest(token)).cloned()
    }

    fn keep(&self, digest: String, grant: Grant) {
        self.grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(digest, grant);
    }

    fn remote_nodes(&self) -> Vec<String> {
        let grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        let mut nodes: Vec<String> = grants
            .values()
            .filter(|grant| grant.node != "local")
            .map(|grant| grant.node.clone())
            .collect();
        nodes.sort();
        nodes.dedup();
        nodes
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
        let mut wanted = None;
        if let Some(tunnel) = tunnels.get_mut(node) {
            if matches!(tunnel.child.try_wait(), Ok(None)) {
                return Ok(tunnel.port);
            }
            wanted = Some(tunnel.port);
            tunnels.remove(node);
        }
        let tunnel = open_tunnel(node, hub_port, wanted).await?;
        let port = tunnel.port;
        tunnels.insert(node.to_owned(), tunnel);
        Ok(port)
    }

    async fn reopen(&self, node: &str, hub_port: u16, port: u16) -> anyhow::Result<()> {
        let mut tunnels = self.tunnels.lock().await;
        if tunnels
            .get_mut(node)
            .is_some_and(|t| matches!(t.child.try_wait(), Ok(None)))
        {
            return Ok(());
        }
        let tunnel = open_tunnel(node, hub_port, Some(port)).await?;
        tunnels.insert(node.to_owned(), tunnel);
        Ok(())
    }

    pub async fn repair_tunnels(&self, hub_port: u16) -> Vec<(String, Result<u16, String>)> {
        let mut tunnels = self.tunnels.lock().await;
        let mut dead: Vec<(String, u16)> = Vec::new();
        for (n, t) in tunnels.iter_mut() {
            if !matches!(t.child.try_wait(), Ok(None)) {
                dead.push((n.clone(), t.port));
            }
        }
        let mut out = Vec::new();
        for (node, port) in dead {
            tunnels.remove(&node);
            match open_tunnel(&node, hub_port, Some(port)).await {
                Ok(t) => {
                    let got = t.port;
                    tunnels.insert(node.clone(), t);
                    out.push((node, Ok(got)));
                }
                Err(e) => out.push((node, Err(e.to_string()))),
            }
        }
        out
    }

    #[must_use]
    pub fn tunnel_count(&self) -> usize {
        self.tunnels.try_lock().map(|t| t.len()).unwrap_or_default()
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

async fn open_tunnel(node: &str, hub_port: u16, wanted: Option<u16>) -> anyhow::Result<Tunnel> {
    blazar_transport::validate_ssh_target(node).map_err(|error| anyhow::anyhow!("{error}"))?;
    let remote_port = wanted.unwrap_or(0);
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
        .arg(format!("127.0.0.1:{remote_port}:127.0.0.1:{hub_port}"))
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
    if let Some(port) = wanted {
        let settled = tokio::time::timeout(Duration::from_secs(8), async {
            let mut last = String::new();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if !line.trim().is_empty() => last = line,
                    Ok(Some(_)) => {}
                    _ => break,
                }
            }
            last
        })
        .await;
        return match (settled, child.try_wait()) {
            (Err(_), Ok(None)) | (Ok(_), Ok(None)) => {
                tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                Ok(Tunnel { port, child })
            }
            (Ok(last), _) => anyhow::bail!("ssh 没能按原端口 {port} 重建反向转发：{last}"),
            (Err(_), Ok(Some(status))) => {
                anyhow::bail!("ssh 重建反向转发时退出了：{status}")
            }
            (Err(_), Err(e)) => anyhow::bail!("ssh 状态读不到：{e}"),
        };
    }
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

fn hub_port() -> Option<u16> {
    crate::office::HUB_URL
        .get()?
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
}

pub async fn watchdog(st: Shared) {
    loop {
        tokio::time::sleep(WATCHDOG_EVERY).await;
        if st.fleet.tunnel_count() == 0 {
            continue;
        }
        let Some(port) = hub_port() else {
            continue;
        };
        for (node, result) in st.fleet.repair_tunnels(port).await {
            match result {
                Ok(p) => {
                    tracing::info!(target: "blazar::fleet", "{node} 的调度回程断了，已按端口 {p} 重建")
                }
                Err(e) => {
                    tracing::warn!(target: "blazar::fleet", "{node} 的调度回程断了，重建失败：{e}")
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Child {
    pub id: String,
    pub name: String,
    pub node: String,
    pub path: String,
}

pub async fn children_of(st: &Shared, workspace: &str) -> Vec<Child> {
    sqlx::query(
        "SELECT w.id, w.name, w.path, n.name AS node FROM workspaces w
         JOIN nodes n ON n.id = w.node_id WHERE w.parent_id = ?1 ORDER BY w.created_at",
    )
    .bind(workspace)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| Child {
        id: r.try_get("id").unwrap_or_default(),
        name: r.try_get("name").unwrap_or_default(),
        node: r.try_get("node").unwrap_or_default(),
        path: r.try_get("path").unwrap_or_default(),
    })
    .collect()
}

#[must_use]
pub fn briefing(parent: &str, children: &[Child]) -> String {
    if children.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "这个工作区（id {parent}）挂了下面这些子工作区，都是可以把活派过去的机器。\
         用 blazar-fleet 的工具操作：send_prompt 派活（立即返回），wait_workspace 等它停下并拿到最后一句回复，\
         get_history / read_file / get_diff 看它做了什么，copy_files 在工作区之间搬文件；派活前可以先 probe_node 看机器状态。\
         子工作区：\n"
    );
    for c in children {
        out.push_str(&format!(
            "- {}：机器 {}，目录 {}，工作区 id {}\n",
            c.name, c.node, c.path, c.id
        ));
    }
    out
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
    force: bool,
    caller: Option<Caller>,
) -> anyhow::Result<Option<McpServerSpec>> {
    if !force && !enabled(st).await {
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
        let _ = crate::office::kv_put(st, &tunnel_key(node), &json!(port)).await;
        format!("http://127.0.0.1:{port}{PATH}")
    };
    let token = st.fleet.issue(session, node, caller)?;
    remember(st, &token).await;
    Ok(Some(McpServerSpec {
        name: blazar_mcp::fleet::SERVER_NAME.into(),
        http: true,
        url,
        headers: BTreeMap::from([("Authorization".to_owned(), format!("Bearer {token}"))]),
        ..Default::default()
    }))
}

async fn remember(st: &Shared, token: &str) {
    let Some(grant) = st.fleet.grant(token) else {
        return;
    };
    let caller = grant
        .caller
        .as_ref()
        .and_then(|c| serde_json::to_string(c).ok());
    let saved: Result<(), sqlx::Error> = async {
        let mut tx = st.db.pool().begin().await?;
        sqlx::query("DELETE FROM fleet_grants WHERE session_id = ?1")
            .bind(grant.session.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO fleet_grants (token_sha256, session_id, node, caller, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(digest(token))
        .bind(grant.session.to_string())
        .bind(&grant.node)
        .bind(caller)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }
    .await;
    if let Err(error) = saved {
        tracing::warn!(target: "blazar::fleet", "调度令牌没存进库，hub 重启后这轮会用不了调度工具: {error}");
    }
}

pub async fn revoke(st: &Shared, session: SessionId) {
    st.fleet.revoke(session).await;
    if let Err(error) = sqlx::query("DELETE FROM fleet_grants WHERE session_id = ?1")
        .bind(session.to_string())
        .execute(st.db.pool())
        .await
    {
        tracing::warn!(target: "blazar::fleet", "作废调度令牌时没删掉库里的记录: {error}");
    }
}

pub async fn restore(st: &Shared) {
    let pool = st.db.pool();
    if let Err(error) = sqlx::query(
        "DELETE FROM fleet_grants WHERE session_id NOT IN
           (SELECT id FROM sessions WHERE status = 'running' AND run_dir IS NOT NULL)",
    )
    .execute(pool)
    .await
    {
        tracing::warn!(target: "blazar::fleet", "清理过期的调度令牌失败: {error}");
    }
    let rows: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT token_sha256, session_id, node, caller FROM fleet_grants")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    for (digest, session, node, caller) in rows {
        let Ok(id) = session.parse() else {
            continue;
        };
        st.fleet.keep(
            digest,
            Grant {
                session: SessionId(id),
                node,
                caller: caller.and_then(|c| serde_json::from_str(&c).ok()),
            },
        );
    }
}

pub async fn reopen_tunnels(st: Shared) {
    let nodes = st.fleet.remote_nodes();
    if nodes.is_empty() {
        return;
    }
    let mut hub = hub_port();
    for _ in 0..300 {
        if hub.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        hub = hub_port();
    }
    let Some(hub) = hub else {
        return;
    };
    for node in nodes {
        let Some(port) = crate::office::kv_get(&st, &tunnel_key(&node))
            .await
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
        else {
            tracing::warn!(target: "blazar::fleet", "没记下 {node} 的调度回程端口，重启前在那台机器上跑的会话用不了调度工具");
            continue;
        };
        match st.fleet.reopen(&node, hub, port).await {
            Ok(()) => {
                tracing::info!(target: "blazar::fleet", "{node} 的调度回程已按端口 {port} 恢复")
            }
            Err(error) => {
                tracing::warn!(target: "blazar::fleet", "{node} 的调度回程没能按端口 {port} 恢复: {error}")
            }
        }
    }
}

fn grant_of(st: &Shared, headers: &HeaderMap) -> Option<Grant> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|t| st.fleet.grant(t))
}

pub async fn mcp_http(State(st): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, "无法解析 JSON-RPC").into_response();
    };
    let grant = grant_of(&st, &headers);
    let messages = match request {
        Value::Array(list) => list,
        single => vec![single],
    };
    let mut replies = Vec::new();
    for message in &messages {
        if let Some(reply) = handle(&st, grant.as_ref(), message).await {
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

async fn handle(st: &Shared, grant: Option<&Grant>, req: &Value) -> Option<Value> {
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
        "tools/call" => call(st, grant, req.get("params")).await,
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

fn workspace_args(name: &str, args: &Value) -> Vec<String> {
    let keys: &[&str] = match name {
        "copy_files" => &["from_workspace_id", "to_workspace_id"],
        _ => &["workspace_id"],
    };
    keys.iter()
        .filter_map(|k| args.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

pub async fn allowed(st: &Shared, caller: &Caller, name: &str, args: &Value) -> Result<(), String> {
    if caller.is_lead() {
        return Ok(());
    }
    if WORKER_DENIED_TOOLS.contains(&name) {
        return Err(format!(
            "worker 不能用 {name}；要派活请 send_message 给总控 bot"
        ));
    }
    if WORKER_WRITE_TOOLS.contains(&name) {
        let mine = crate::bots::claims::workspaces_of(st, &caller.bot).await;
        let home: Option<String> = crate::bots::get(st, &caller.bot)
            .await
            .map(|b| b.home_ws_id);
        for ws in workspace_args(name, args) {
            if !mine.contains(&ws) && home.as_deref() != Some(ws.as_str()) {
                return Err(format!(
                    "工作区 {ws} 不在你的认领里；先 claim 它，或者 send_message 给它的 owner"
                ));
            }
        }
    }
    Ok(())
}

async fn call(st: &Shared, grant: Option<&Grant>, params: Option<&Value>) -> Result<Value, String> {
    let params = params.ok_or("tools/call 缺少 params")?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("缺少工具名")?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let caller = grant.and_then(|g| g.caller.clone());
    if let Some(c) = &caller {
        allowed(st, c, name, &args).await?;
    }
    let tool = blazar_mcp::tools::find(name).ok_or_else(|| format!("未知工具: {name}"))?;
    let (method, path, body) = (tool.build)(&args).map_err(|error| error.to_string())?;
    let mut extra = Vec::new();
    if let Some(c) = &caller {
        let (k, v) = crate::bots::api::caller_header(c);
        extra.push((k, v));
    }
    let (ok, text) = internal_request(st, method, &path, body, &extra).await?;
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
    extra_headers: &[(String, String)],
) -> Result<(bool, String), String> {
    let base = crate::office::HUB_URL.get().ok_or("hub 地址还没就绪")?;
    let token = st.auth.get().ok_or("hub 会话还没就绪")?.token();
    let method: reqwest::Method = method
        .parse()
        .map_err(|_| format!("不支持的方法 {method}"))?;
    let mut request = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .bearer_auth(token);
    for (k, v) in extra_headers {
        request = request.header(k, v);
    }
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
    fn briefing_lists_every_child_with_its_id() {
        assert!(briefing("p", &[]).is_empty());
        let text = briefing(
            "parent-1",
            &[Child {
                id: "child-1".into(),
                name: "orbit @ gpu-1".into(),
                node: "gpu-1".into(),
                path: "/data/orbit".into(),
            }],
        );
        assert!(text.contains("parent-1") && text.contains("child-1"));
        assert!(text.contains("gpu-1") && text.contains("/data/orbit"));
        assert!(text.contains("send_prompt") && text.contains("copy_files"));
    }

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
        let first = fleet.issue(a, "gpu-1", None).unwrap();
        let second = fleet.issue(a, "gpu-1", None).unwrap();
        assert_ne!(first, second);
        assert!(fleet.grant(&first).is_none(), "重新签发后旧令牌作废");
        assert_eq!(fleet.grant(&second).unwrap().node, "gpu-1");
        let caller = Caller {
            bot: "b1".into(),
            handle: "trainer".into(),
            role: "worker".into(),
            session: Some(b.to_string()),
        };
        let other = fleet.issue(b, "gpu-2", Some(caller.clone())).unwrap();
        assert!(fleet.grant(&other[..63]).is_none());
        assert!(fleet.grant(&format!("{other}0")).is_none());
        assert_eq!(fleet.grant(&other).unwrap().caller, Some(caller));
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(fleet.revoke(a));
        assert!(fleet.grant(&second).is_none());
        assert!(fleet.grant(&other).is_some());
    }

    #[tokio::test]
    async fn grants_of_running_sessions_survive_a_hub_restart() {
        let root = tempfile::tempdir().unwrap();
        crate::ensure_private_dir(root.path()).unwrap();
        let path = root.path().join("hub.sqlite");
        let open = |db: blazar_db::Db| {
            crate::state::AppState::with_services(
                db,
                "local".into(),
                None,
                crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
                crate::services::Services::Isolated,
            )
        };
        let st = open(blazar_db::Db::open(&path).await.unwrap());
        let live = SessionId::new();
        let done = SessionId::new();
        for setup in [
            "INSERT INTO nodes (id, name, transport, status, created_at)
             VALUES ('n', 'local', 'local', 'online', 't')",
            "INSERT INTO workspaces (id, node_id, name, path, created_at)
             VALUES ('w', 'n', 'w', '/w', 't')",
        ] {
            sqlx::query(setup).execute(st.db.pool()).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, run_dir)
             VALUES (?1, 'w', 'claude_cli', 'running', 't', '/tmp/run'),
                    (?2, 'w', 'claude_cli', 'done', 't', NULL)",
        )
        .bind(live.to_string())
        .bind(done.to_string())
        .execute(st.db.pool())
        .await
        .unwrap();
        let caller = Caller {
            bot: "b1".into(),
            handle: "trainer".into(),
            role: "lead".into(),
            session: Some(live.to_string()),
        };
        let token = st.fleet.issue(live, "local", Some(caller.clone())).unwrap();
        remember(&st, &token).await;
        let gone = st.fleet.issue(done, "gpu-1", None).unwrap();
        remember(&st, &gone).await;
        let stored: Vec<String> = sqlx::query_scalar("SELECT token_sha256 FROM fleet_grants")
            .fetch_all(st.db.pool())
            .await
            .unwrap();
        assert_eq!(stored.len(), 2);
        assert!(!stored.contains(&token) && !stored.contains(&gone));
        drop(st);

        let st = open(blazar_db::Db::open(&path).await.unwrap());
        assert!(st.fleet.grant(&token).is_none());
        restore(&st).await;
        let back = st.fleet.grant(&token).unwrap();
        assert_eq!(
            (back.session, back.node.as_str(), back.caller),
            (live, "local", Some(caller))
        );
        assert!(st.fleet.grant(&gone).is_none());
        let count = || async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fleet_grants")
                .fetch_one(st.db.pool())
                .await
                .unwrap()
        };
        assert_eq!(count().await, 1);
        revoke(&st, live).await;
        assert!(st.fleet.grant(&token).is_none());
        assert_eq!(count().await, 0);
    }

    #[tokio::test]
    async fn workers_are_scoped_and_leads_are_not() {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
            crate::services::Services::Isolated,
        );
        sqlx::query(
            "INSERT INTO agents (id, name, runtime, created_at, updated_at)
             VALUES ('a', 'x', 'claude', '2026-10-13T00:00:00Z', '2026-10-13T00:00:00Z')",
        )
        .execute(st.db.pool())
        .await
        .unwrap();
        let bot = crate::bots::create(
            &st,
            crate::bots::NewBot {
                handle: "w".into(),
                agent_id: "a".into(),
                home_path: Some(root.path().join("w").display().to_string()),
                ..crate::bots::NewBot::default()
            },
        )
        .await
        .unwrap();
        let worker = Caller {
            bot: bot.id.clone(),
            handle: "w".into(),
            role: "worker".into(),
            session: None,
        };
        let lead = Caller {
            role: "lead".into(),
            ..worker.clone()
        };
        assert!(
            allowed(&st, &worker, "fleet_status", &json!({}))
                .await
                .is_ok()
        );
        assert!(
            allowed(&st, &worker, "send_message", &json!({ "to": "x" }))
                .await
                .is_ok()
        );
        assert!(
            allowed(&st, &worker, "create_task", &json!({}))
                .await
                .is_err()
        );
        assert!(
            allowed(
                &st,
                &worker,
                "send_prompt",
                &json!({ "workspace_id": "other" })
            )
            .await
            .is_err()
        );
        assert!(
            allowed(
                &st,
                &worker,
                "send_prompt",
                &json!({ "workspace_id": bot.home_ws_id })
            )
            .await
            .is_ok(),
            "自己的家工作区可以"
        );
        assert!(
            allowed(
                &st,
                &lead,
                "send_prompt",
                &json!({ "workspace_id": "other" })
            )
            .await
            .is_ok()
        );
        crate::bots::claims::create(
            &st,
            crate::bots::claims::NewClaim {
                scope: "repo".into(),
                workspace_id: Some(bot.home_ws_id.clone()),
                ..crate::bots::claims::NewClaim::default()
            },
            Some(&worker),
        )
        .await
        .unwrap();
        assert!(
            allowed(
                &st,
                &worker,
                "copy_files",
                &json!({ "from_workspace_id": bot.home_ws_id, "to_workspace_id": "elsewhere" })
            )
            .await
            .is_err()
        );
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
                let response =
                    mcp_http(State(st), HeaderMap::new(), Bytes::from(body.to_string())).await;
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
        assert!(names.contains(&"send_message") && names.contains(&"fleet_status"));
        assert!(names.contains(&"spawn_bot"));
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
