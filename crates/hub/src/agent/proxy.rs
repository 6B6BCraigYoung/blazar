use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_transport::ExecSpec;
use futures::TryStreamExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell};

use crate::api::Shared;

// 远端机器上的 Claude Code 把请求发到这里（经 SSH 反向隧道），这里把账号暗号换成本机保存的长期 token 再转发。
// 凭据始终只在本机：远端进程手里只有暗号，暗号只在隧道存在时、且只在那台机器的本地回环上有用。
const UPSTREAM: &str = "https://api.anthropic.com";
const MAX_BODY: usize = 64 * 1024 * 1024;

pub struct Proxy {
    port: u16,
    key: Vec<u8>,
    client: reqwest::Client,
}

struct Tunnel {
    port: u16,
    child: tokio::process::Child,
}

type NetEnv = Vec<(String, String)>;

#[derive(Default)]
pub struct ProxyState {
    proxy: OnceCell<Arc<Proxy>>,
    tunnels: Mutex<HashMap<String, Tunnel>>,
    watched: Mutex<std::collections::HashSet<String>>,
    net_env: Mutex<HashMap<String, (std::time::Instant, NetEnv)>>,
}

const NET_KEYS: &[&str] = &["http_proxy", "https_proxy", "all_proxy", "no_proxy"];
const NET_ENV_TTL: Duration = Duration::from_secs(600);

#[must_use]
pub fn parse_net_env(env_dump: &str) -> NetEnv {
    let mut out: NetEnv = env_dump
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, v)| NET_KEYS.contains(&k.to_ascii_lowercase().as_str()) && !v.is_empty())
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    out.sort();
    out.dedup();
    out
}

// 远端机器的出网代理通常写在 ~/.bashrc 里，而 .bashrc 对非交互 shell 一开头就 return，后台启动的 agent 拿不到。
// 这里按交互式 shell 读一次那台机器的代理变量，远端运行和登录时原样带上，跟你 SSH 上去手动敲命令的网络环境一致。
pub async fn node_net_env(st: &Shared, node: &str) -> NetEnv {
    if let Some((at, env)) = st.proxy.net_env.lock().await.get(node)
        && at.elapsed() < NET_ENV_TTL
    {
        return env.clone();
    }
    let t = st.transport(node);
    let probe = t.exec(
        ExecSpec::new("bash")
            .arg("-c")
            .arg("bash -ic env </dev/null 2>/dev/null"),
    );
    let env = match tokio::time::timeout(Duration::from_secs(20), probe).await {
        Ok(Ok(out)) => parse_net_env(&out.stdout),
        _ => return Vec::new(),
    };
    st.proxy
        .net_env
        .lock()
        .await
        .insert(node.to_owned(), (std::time::Instant::now(), env.clone()));
    env
}

fn data_dir(st: &Shared) -> PathBuf {
    st.mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
}

fn load_key(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    let path = dir.join("proxy.key");
    if let Ok(k) = std::fs::read(&path)
        && k.len() == 32
    {
        return Ok(k);
    }
    let mut k = vec![0u8; 32];
    getrandom::fill(&mut k).map_err(|e| format!("生成代理密钥失败：{e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    opts.open(&path)
        .and_then(|mut f| f.write_all(&k))
        .map_err(|e| format!("保存代理密钥失败：{e}"))?;
    Ok(k)
}

#[must_use]
pub fn secret_for(key: &[u8], account: &str) -> String {
    let mut h = Sha256::new();
    h.update(key);
    h.update(b":");
    h.update(account.as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("blz-{hex}")
}

async fn proxy(st: &Shared) -> Result<Arc<Proxy>, String> {
    st.proxy
        .proxy
        .get_or_try_init(|| async {
            let key = load_key(&data_dir(st))?;
            let client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .build()
                .map_err(|e| format!("建 HTTP 客户端失败：{e}"))?;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| format!("代理监听失败：{e}"))?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            let app = Router::new().fallback(forward).with_state(st.clone());
            tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::error!(target: "blazar::proxy", "凭据代理停了：{e}");
                }
            });
            tracing::info!(target: "blazar::proxy", "凭据代理在 127.0.0.1:{port}");
            Ok(Arc::new(Proxy { port, key, client }))
        })
        .await
        .cloned()
}

pub async fn secret(st: &Shared, account: &str) -> Result<String, String> {
    Ok(secret_for(&proxy(st).await?.key, account))
}

fn deny(status: StatusCode, msg: &str) -> Response {
    (
        status,
        axum::Json(
            json!({ "type": "error", "error": { "type": "authentication_error", "message": msg } }),
        ),
    )
        .into_response()
}

async fn token_for(st: &Shared, key: &[u8], presented: &str) -> Option<String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, config_dir FROM accounts
         WHERE provider = 'claude' AND auth_mode = 'token' AND disabled = 0 AND config_dir IS NOT NULL",
    )
    .fetch_all(st.db.pool())
    .await
    .ok()?;
    let (_, dir) = rows
        .into_iter()
        .find(|(id, _)| secret_for(key, id) == presented)?;
    let path = PathBuf::from(dir).join(crate::accounts::TOKEN_FILE);
    tokio::fs::read_to_string(path)
        .await
        .ok()
        .map(|t| t.trim().to_owned())
}

// 代理的处理函数又要等代理初始化完，装箱打断 async 的递归类型。
fn proxy_boxed(st: &Shared) -> futures::future::BoxFuture<'_, Result<Arc<Proxy>, String>> {
    Box::pin(proxy(st))
}

async fn forward(State(st): State<Shared>, req: Request) -> Response {
    let Ok(p) = proxy_boxed(&st).await else {
        return deny(StatusCode::SERVICE_UNAVAILABLE, "Blazar 凭据代理没起来");
    };
    let presented = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned();
    let Some(token) = token_for(&st, &p.key, &presented).await else {
        return deny(
            StatusCode::UNAUTHORIZED,
            "Blazar 不认识这个账号暗号（账号可能已删除或停用）",
        );
    };
    let url = format!(
        "{UPSTREAM}{}",
        req.uri().path_and_query().map_or("/", |x| x.as_str())
    );
    let method = req.method().clone();
    let mut headers = req.headers().clone();
    for h in [
        "host",
        "authorization",
        "content-length",
        "connection",
        "proxy-authorization",
        "transfer-encoding",
    ] {
        headers.remove(h);
    }
    let body = match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(e) => return deny(StatusCode::PAYLOAD_TOO_LARGE, &e.to_string()),
    };
    let up = p
        .client
        .request(method, &url)
        .headers(headers)
        .bearer_auth(token)
        .body(body)
        .send()
        .await;
    let up = match up {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({ "type": "error", "error": { "type": "api_error",
                    "message": format!("Blazar 连不上 Anthropic：{e}") } })),
            )
                .into_response();
        }
    };
    let mut out = Response::builder().status(up.status());
    for (k, v) in up.headers() {
        if !matches!(
            k.as_str(),
            "connection" | "transfer-encoding" | "content-length"
        ) {
            out = out.header(k, v);
        }
    }
    let stream = up.bytes_stream().map_err(std::io::Error::other);
    out.body(Body::from_stream(stream))
        .unwrap_or_else(|_| deny(StatusCode::BAD_GATEWAY, "转发响应失败"))
}

fn pick_port() -> u16 {
    let mut b = [0u8; 2];
    let _ = getrandom::fill(&mut b);
    20000 + u16::from_le_bytes(b) % 40000
}

fn port_key(node: &str) -> String {
    format!("proxy.tunnel_port.{node}")
}

async fn open_tunnel(node: &str, remote: u16, local: u16) -> Result<tokio::process::Child, String> {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-N",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ControlPath=none",
            "-R",
            &format!("127.0.0.1:{remote}:127.0.0.1:{local}"),
            node,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("起不了 ssh：{e}"))?;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(Some(_)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let _ = e.read_to_string(&mut err).await;
            }
            return Err(err.trim().chars().take(300).collect());
        }
    }
    Ok(child)
}

// 在远端真去敲一下隧道：代理对没带暗号的请求回 401，拿到它就说明一路是通的。
async fn tunnel_works(st: &Shared, node: &str, port: u16) -> bool {
    let probe = format!(
        "curl -s -o /dev/null -m 8 -w '%{{http_code}}' http://127.0.0.1:{port}/v1/blazar-ping || echo nocurl"
    );
    match st
        .transport(node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(probe))
        .await
    {
        Ok(out) => {
            let s = out.stdout.trim();
            s == "401" || s.ends_with("nocurl")
        }
        Err(_) => false,
    }
}

// 保证到这台机器的反向隧道在，返回远端那头的端口。端口按机器记住，隧道断了重建时尽量还用原来的，正在跑的会话不受影响。
pub async fn ensure_tunnel(st: &Shared, node: &str) -> Result<u16, String> {
    let local = proxy(st).await?.port;
    let mut tunnels = st.proxy.tunnels.lock().await;
    if let Some(t) = tunnels.get_mut(node)
        && matches!(t.child.try_wait(), Ok(None))
    {
        return Ok(t.port);
    }
    tunnels.remove(node);
    let saved = crate::office::kv_get(st, &port_key(node))
        .await
        .as_u64()
        .and_then(|p| u16::try_from(p).ok());
    let mut last = String::new();
    for (i, port) in saved
        .into_iter()
        .chain(std::iter::repeat_with(pick_port))
        .take(4)
        .enumerate()
    {
        match open_tunnel(node, port, local).await {
            Ok(child) => {
                if !tunnel_works(st, node, port).await {
                    last = "隧道建好了但在远端访问不到".into();
                    continue;
                }
                let _ = crate::office::kv_put(st, &port_key(node), &json!(port)).await;
                tunnels.insert(node.to_owned(), Tunnel { port, child });
                drop(tunnels);
                watch(st, node).await;
                if i > 0 {
                    tracing::info!(target: "blazar::proxy", "{node} 的隧道换到了端口 {port}");
                }
                return Ok(port);
            }
            Err(e) => last = e,
        }
    }
    Err(format!("到 {node} 的 SSH 隧道建不起来：{last}"))
}

// 看门狗里重建隧道会再次走到 watch，装箱打断 async 的递归类型。
fn ensure_boxed<'a>(
    st: &'a Shared,
    node: &'a str,
) -> futures::future::BoxFuture<'a, Result<u16, String>> {
    Box::pin(ensure_tunnel(st, node))
}

// 隧道断了（网络抖动、机器重启）而那台机器上还有用代理账号的会话在跑，就自动重建。
async fn watch(st: &Shared, node: &str) {
    if !st.proxy.watched.lock().await.insert(node.to_owned()) {
        return;
    }
    let (st, node) = (st.clone(), node.to_owned());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let alive = {
                let mut t = st.proxy.tunnels.lock().await;
                t.get_mut(&node)
                    .is_some_and(|t| matches!(t.child.try_wait(), Ok(None)))
            };
            if alive || !needs_tunnel(&st, &node).await {
                continue;
            }
            if let Err(e) = ensure_boxed(&st, &node).await {
                tracing::warn!(target: "blazar::proxy", "重建 {node} 的隧道失败：{e}");
            }
        }
    });
}

async fn needs_tunnel(st: &Shared, node: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sessions s JOIN accounts a ON a.id = s.account_id
         WHERE s.status = 'running' AND s.node = ?1 AND a.auth_mode = 'token'",
    )
    .bind(node)
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0)
        > 0
}

// Blazar 重启后，给还在远端跑、用着代理账号的会话把隧道接回来。
pub async fn restore(st: Shared) {
    let nodes: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT s.node FROM sessions s JOIN accounts a ON a.id = s.account_id
         WHERE s.status = 'running' AND s.node IS NOT NULL AND s.node != 'local' AND a.auth_mode = 'token'",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    for node in nodes {
        if let Err(e) = ensure_tunnel(&st, &node).await {
            tracing::warn!(target: "blazar::proxy", "恢复 {node} 的隧道失败：{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_per_account_and_stable() {
        let k = [7u8; 32];
        let a = secret_for(&k, "acc-a");
        assert_eq!(a, secret_for(&k, "acc-a"));
        assert_ne!(a, secret_for(&k, "acc-b"));
        assert_ne!(a, secret_for(&[8u8; 32], "acc-a"), "换了密钥旧暗号就作废");
        assert!(a.starts_with("blz-") && a.len() == 4 + 64);
    }

    #[test]
    fn only_proxy_variables_are_taken_from_the_remote_shell() {
        let dump = "PATH=/usr/bin\nhttps_proxy=http://10.126.126.198:7897\nHTTPS_PROXY=http://10.126.126.198:7897\n\
                    no_proxy=localhost,127.0.0.1,10.0.0.0/8\nALL_PROXY=\nSECRET=x\nnot a var\n";
        let env = parse_net_env(dump);
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["HTTPS_PROXY", "https_proxy", "no_proxy"]);
    }

    #[test]
    fn tunnel_ports_stay_in_the_unprivileged_range() {
        for _ in 0..200 {
            let p = pick_port();
            assert!((20000..60000).contains(&p));
        }
    }
}
