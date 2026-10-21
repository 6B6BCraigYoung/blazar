use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;

#[derive(Debug, PartialEq, Eq)]
pub struct Address {
    pub port: u16,
    pub path: String,
    pub https: bool,
    pub source: &'static str,
}

pub fn choose_address(
    manual: Option<u16>,
    from_log: Option<(u16, String, bool)>,
    listening: &[u16],
) -> Option<Address> {
    if let Some(port) = manual {
        return Some(Address {
            port,
            path: "/".into(),
            https: false,
            source: "manual",
        });
    }
    if let Some((port, path, https)) = from_log {
        return Some(Address {
            port,
            path,
            https,
            source: "log",
        });
    }
    listening.iter().min().map(|&port| Address {
        port,
        path: "/".into(),
        https: false,
        source: "listen",
    })
}

struct Tunnel {
    remote_port: u16,
    local_port: u16,
    child: tokio::process::Child,
}

fn tunnels() -> &'static Mutex<HashMap<String, Tunnel>> {
    static T: OnceLock<Mutex<HashMap<String, Tunnel>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) async fn ensure_tunnel(ws: &str, node: &str, remote_port: u16) -> Result<u16, String> {
    blazar_transport::validate_ssh_target(node).map_err(|e| e.to_string())?;
    let mut map = tunnels().lock().await;
    if let Some(t) = map.get_mut(ws) {
        if t.remote_port == remote_port && t.child.try_wait().ok().flatten().is_none() {
            return Ok(t.local_port);
        }
        let _ = t.child.start_kill();
        map.remove(ws);
    }

    let local_port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map_err(|e| e.to_string())?
        .port();
    let resolved = blazar_transport::ssh_opts_without_forwards(node).await;
    let child = tokio::process::Command::new("ssh")
        .args([
            "-N",
            "-o",
            "BatchMode=yes",
            "-o",
            "NumberOfPasswordPrompts=0",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=30",
            "-L",
        ])
        .arg(format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"))
        .args(&resolved)
        .arg("--")
        .arg(node)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("起不了 ssh 隧道：{e}"))?;
    map.insert(
        ws.to_owned(),
        Tunnel {
            remote_port,
            local_port,
            child,
        },
    );
    drop(map);

    for _ in 0..30 {
        if tokio::net::TcpStream::connect(("127.0.0.1", local_port))
            .await
            .is_ok()
        {
            return Ok(local_port);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "到 {node} 的隧道没连上（那台机器的 {remote_port} 端口可能还没开始监听）"
    ))
}

pub(super) async fn close_tunnel(ws: &str) {
    if let Some(mut t) = tunnels().lock().await.remove(ws) {
        let _ = t.child.start_kill();
    }
}

pub(super) fn handed() -> &'static std::sync::Mutex<HashMap<String, (u16, &'static str)>> {
    static H: OnceLock<std::sync::Mutex<HashMap<String, (u16, &'static str)>>> = OnceLock::new();
    H.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn handed_port(v: &Value) -> Option<(u16, &'static str)> {
    if v["preview_url"].is_null() {
        return None;
    }
    let (port, purpose) = match v["tunnel"].as_u64() {
        Some(lp) => (lp, "preview-tunnel"),
        None => (v["port"].as_u64()?, "preview"),
    };
    Some((u16::try_from(port).ok()?, purpose))
}

pub(super) fn remember(ws: &str, v: &Value) {
    let mut map = handed().lock().unwrap();
    match handed_port(v) {
        Some(entry) => map.insert(ws.to_owned(), entry),
        None => map.remove(ws),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn ssh_targets_are_rejected_before_preview_tunnel_setup() {
        for node in ["-invalid", "", "Alice@-invalid", "host name"] {
            assert!(ensure_tunnel("test-workspace", node, 30000).await.is_err());
        }
    }

    #[test]
    fn only_previews_with_an_address_hand_out_a_port() {
        assert_eq!(
            handed_port(
                &json!({ "running": true, "port": 5173, "preview_url": "http://localhost:5173/" })
            ),
            Some((5173, "preview"))
        );
        assert_eq!(
            handed_port(
                &json!({ "running": true, "port": 3000, "tunnel": 51234, "preview_url": "http://127.0.0.1:51234/" })
            ),
            Some((51234, "preview-tunnel"))
        );
        assert_eq!(
            handed_port(&json!({ "running": true, "port": 3000, "tunnel_error": "x" })),
            None
        );
        assert_eq!(
            handed_port(&json!({ "running": false, "port": 3000 })),
            None
        );
    }
}
