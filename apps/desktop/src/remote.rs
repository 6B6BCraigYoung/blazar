use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::process::Child;
use tokio::sync::Notify;

use crate::forward::Forwards;
use crate::hub_mode::{Target, ssh_args};

const BACKOFF: [u64; 5] = [1, 2, 5, 10, 30];
pub const READY_TIMEOUT: Duration = Duration::from_secs(15);

pub const PORTS_FILE: &str = "tunnel-ports.json";

pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port())
}

fn port_is_free(port: u16) -> bool {
    port != 0 && std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub fn tunnel_port(file: &Path, alias: &str, remote: u16) -> std::io::Result<u16> {
    let key = format!("{alias}:{remote}");
    let mut saved: BTreeMap<String, u16> = std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if let Some(port) = saved.get(&key).copied()
        && port_is_free(port)
    {
        return Ok(port);
    }
    let port = free_port()?;
    saved.insert(key, port);
    match serde_json::to_string_pretty(&saved) {
        Ok(text) => {
            if let Err(e) = std::fs::write(file, text + "\n") {
                tracing::warn!("记不下隧道端口：{e}");
            }
        }
        Err(e) => tracing::warn!("记不下隧道端口：{e}"),
    }
    Ok(port)
}

pub const HUB_HOST: &str = "127.0.0.1";

fn spawn_ssh(
    program: &std::ffi::OsStr,
    alias: &str,
    local: u16,
    dest: &str,
    remote: u16,
) -> std::io::Result<Child> {
    tokio::process::Command::new(program)
        .args(ssh_args(alias, local, dest, remote))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

const FATAL: [&str; 5] = [
    "open failed",
    "Connection refused",
    "Could not resolve hostname",
    "Permission denied",
    "Host key verification failed",
];

pub fn fatal(line: &str) -> bool {
    FATAL.iter().any(|pattern| line.contains(pattern))
}

fn capture_stderr(child: &mut Child, last: Arc<Mutex<String>>) -> tokio::task::JoinHandle<()> {
    let stderr = child.stderr.take();
    tokio::spawn(async move {
        let Some(stderr) = stderr else { return };
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim();
            if !line.is_empty() {
                tracing::info!(target: "blazar_desktop::tunnel", "ssh: {line}");
                *last.lock().unwrap() = line.to_owned();
            }
        }
    })
}

pub struct Tunnel {
    program: std::ffi::OsString,
    alias: String,
    dest: &'static str,
    local: u16,
    remote: u16,
    child: Mutex<Option<Child>>,
    last_error: Arc<Mutex<String>>,
    stopped: AtomicBool,
    kick: Notify,
}

impl Tunnel {
    pub fn start(alias: &str, remote: u16, local: u16) -> Arc<Self> {
        Self::spawn("ssh".as_ref(), alias, HUB_HOST, remote, local)
    }

    pub fn spawn(
        program: &std::ffi::OsStr,
        alias: &str,
        dest: &'static str,
        remote: u16,
        local: u16,
    ) -> Arc<Self> {
        let tunnel = Arc::new(Self {
            program: program.to_owned(),
            alias: alias.to_owned(),
            dest,
            local,
            remote,
            child: Mutex::new(None),
            last_error: Arc::default(),
            stopped: AtomicBool::new(false),
            kick: Notify::new(),
        });
        tokio::spawn(tunnel.clone().guard());
        tunnel
    }

    pub fn local_port(&self) -> u16 {
        self.local
    }

    pub fn last_error(&self) -> String {
        self.last_error.lock().unwrap().clone()
    }

    pub fn kick(&self) {
        self.kick.notify_one();
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.kill();
    }

    fn kill(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.start_kill();
            tracing::info!(alias = %self.alias, "SSH 隧道已关闭");
        }
    }

    async fn guard(self: Arc<Self>) {
        let mut attempt = 0;
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            let started = Instant::now();
            match spawn_ssh(
                &self.program,
                &self.alias,
                self.local,
                self.dest,
                self.remote,
            ) {
                Ok(mut child) => {
                    tracing::info!(
                        alias = %self.alias,
                        local = self.local,
                        remote = self.remote,
                        "SSH 隧道已启动"
                    );
                    capture_stderr(&mut child, self.last_error.clone());
                    *self.child.lock().unwrap() = Some(child);
                    loop {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        if self.stopped.load(Ordering::SeqCst) {
                            self.kill();
                            return;
                        }
                        let mut slot = self.child.lock().unwrap();
                        let Some(child) = slot.as_mut() else { return };
                        match child.try_wait() {
                            Ok(None) => {}
                            Ok(Some(status)) => {
                                tracing::warn!(alias = %self.alias, %status, "SSH 隧道断了");
                                *slot = None;
                                break;
                            }
                            Err(e) => {
                                tracing::warn!(alias = %self.alias, "SSH 隧道状态未知：{e}");
                                let _ = child.start_kill();
                                *slot = None;
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    *self.last_error.lock().unwrap() = format!("启动 ssh 失败：{e}");
                }
            }
            if started.elapsed() > Duration::from_secs(60) {
                attempt = 0;
            }
            let delay = Duration::from_secs(BACKOFF[attempt.min(BACKOFF.len() - 1)]);
            attempt += 1;
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = self.kick.notified() => {}
            }
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

pub async fn reachable(client: &reqwest::Client, base: &str) -> bool {
    client
        .get(format!("{base}/"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

pub async fn wait_reachable(client: &reqwest::Client, base: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if reachable(client, base).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

pub async fn session_cookie(client: &reqwest::Client, base: &str) -> Result<String, String> {
    let response = client
        .post(format!("{base}/api/auth/session"))
        .send()
        .await
        .map_err(|e| format!("连不上 hub：{e}"))?;
    if response.status() != reqwest::StatusCode::NO_CONTENT {
        return Err(format!("hub 拒绝了会话：HTTP {}", response.status()));
    }
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .find(|v| v.starts_with("blazar_session_") && v.contains('='))
        .map(str::to_owned)
        .ok_or_else(|| "hub 没有给会话 cookie".to_owned())
}

pub const NOT_A_HUB: &str = "那个端口上不是 Blazar hub";

pub async fn probe(alias: &str, port: u16) -> Result<(), String> {
    let local = free_port().map_err(|e| format!("找不到空闲端口：{e}"))?;
    let mut child = spawn_ssh("ssh".as_ref(), alias, local, HUB_HOST, port)
        .map_err(|e| format!("启动 ssh 失败：{e}"))?;
    let last = Arc::new(Mutex::new(String::new()));
    let reader = capture_stderr(&mut child, last.clone());
    let base = format!("http://127.0.0.1:{local}");
    let client = client();
    let deadline = Instant::now() + READY_TIMEOUT;
    let result = loop {
        let answered = client
            .get(format!("{base}/"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .is_ok();
        if answered {
            break session_cookie(&client, &base)
                .await
                .map(drop)
                .map_err(|_| NOT_A_HUB.to_owned());
        }
        let line = last.lock().unwrap().clone();
        if fatal(&line) {
            break Err(line);
        }
        if let Ok(Some(_)) = child.try_wait() {
            let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
            let line = last.lock().unwrap().clone();
            break Err(if line.is_empty() {
                "ssh 退出了".to_owned()
            } else {
                line
            });
        }
        if Instant::now() >= deadline {
            let line = last.lock().unwrap().clone();
            break Err(if line.is_empty() {
                format!("{} 秒内没有连上", READY_TIMEOUT.as_secs())
            } else {
                line
            });
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    let _ = child.kill().await;
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Connected,
    Failed(String),
}

pub struct RemoteHub {
    pub target: Target,
    pub base: String,
    pub tunnel: Option<Arc<Tunnel>>,
    pub forwards: Option<Forwards>,
    client: reqwest::Client,
    cookie: tokio::sync::Mutex<Option<String>>,
    phase: Mutex<Phase>,
    pub phase_changed: Notify,
    pub retry: Notify,
    pub accounts_kick: Arc<Notify>,
    pub inbox_kick: Arc<Notify>,
}

impl RemoteHub {
    pub fn new(target: Target, ports: &Path) -> std::io::Result<Self> {
        let (base, tunnel, forwards) = match &target {
            Target::Direct(url) => (url.clone(), None, None),
            Target::Ssh { alias, port } => {
                let local = tunnel_port(ports, alias, *port)?;
                let tunnel = Tunnel::start(alias, *port, local);
                (
                    format!("http://127.0.0.1:{}", tunnel.local_port()),
                    Some(tunnel),
                    Some(Forwards::new(
                        alias,
                        ports.with_file_name(crate::forward::PORTS_FILE),
                    )),
                )
            }
        };
        Ok(Self {
            target,
            base,
            tunnel,
            forwards,
            client: client(),
            cookie: tokio::sync::Mutex::new(None),
            phase: Mutex::new(Phase::Connecting),
            phase_changed: Notify::new(),
            retry: Notify::new(),
            accounts_kick: Arc::new(Notify::new()),
            inbox_kick: Arc::new(Notify::new()),
        })
    }

    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap().clone()
    }

    pub fn set_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap() = phase;
        self.phase_changed.notify_waiters();
    }

    pub fn failure(&self) -> String {
        let line = self
            .tunnel
            .as_ref()
            .map(|t| t.last_error())
            .unwrap_or_default();
        if line.is_empty() {
            format!("{} 没有响应", self.target.label())
        } else {
            line
        }
    }

    pub async fn reachable(&self) -> bool {
        reachable(&self.client, &self.base).await
    }

    pub async fn wait_reachable(&self) -> bool {
        wait_reachable(&self.client, &self.base, READY_TIMEOUT).await
    }

    pub fn shutdown(&self) {
        if let Some(tunnel) = &self.tunnel {
            tunnel.shutdown();
        }
        if let Some(forwards) = &self.forwards {
            forwards.shutdown();
        }
    }

    pub fn kick(&self) {
        if let Some(tunnel) = &self.tunnel {
            tunnel.kick();
        }
        if let Some(forwards) = &self.forwards {
            forwards.kick();
        }
    }

    pub async fn loopback_ports(&self) -> Result<Vec<u16>, String> {
        let list = self
            .request(reqwest::Method::GET, crate::forward::LIST_PATH, None)
            .await?;
        Ok(crate::forward::allowed_ports(&list))
    }

    pub async fn forward(&self, port: u16) -> Result<u16, String> {
        let allowed = self.loopback_ports().await?;
        let Some(forwards) = &self.forwards else {
            return crate::forward::check(port, &allowed).map(|()| port);
        };
        let tunnel = forwards.open(port, &allowed)?;
        match crate::forward::wait_listening(&tunnel).await {
            Ok(()) => Ok(tunnel.local_port()),
            Err(e) => {
                forwards.close(port);
                Err(e)
            }
        }
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, String> {
        for attempt in 0..2 {
            let cookie = {
                let mut slot = self.cookie.lock().await;
                if slot.is_none() {
                    *slot = Some(session_cookie(&self.client, &self.base).await?);
                }
                slot.clone().unwrap_or_default()
            };
            let mut request = self
                .client
                .request(method.clone(), format!("{}{path}", self.base))
                .header(reqwest::header::COOKIE, cookie);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request
                .send()
                .await
                .map_err(|e| format!("连不上 hub：{e}"))?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                *self.cookie.lock().await = None;
                continue;
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|e| format!("读 hub 回复失败：{e}"))?;
            let value: Value = serde_json::from_slice(&bytes).unwrap_or_default();
            if !status.is_success() {
                return Err(value["error"]
                    .as_str()
                    .map_or_else(|| format!("HTTP {status}"), str::to_owned));
            }
            return Ok(value);
        }
        Err("hub 会话失效".into())
    }
}

impl Drop for RemoteHub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
pub static TEST_PORTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_port_is_kept_per_target_while_it_is_free() {
        let _ports = TEST_PORTS.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(PORTS_FILE);
        let first = tunnel_port(&file, "hub-host", 7777).unwrap();
        assert_eq!(tunnel_port(&file, "hub-host", 7777).unwrap(), first);
        let other = tunnel_port(&file, "hub-host", 7000).unwrap();
        assert_ne!(other, first);
        assert_eq!(tunnel_port(&file, "hub-host", 7777).unwrap(), first);
        assert_eq!(tunnel_port(&file, "hub-host", 7000).unwrap(), other);
    }

    #[test]
    fn busy_tunnel_port_is_replaced_and_remembered() {
        let _ports = TEST_PORTS.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(PORTS_FILE);
        let first = tunnel_port(&file, "hub-host", 7777).unwrap();
        let busy = std::net::TcpListener::bind(("127.0.0.1", first)).unwrap();
        let second = tunnel_port(&file, "hub-host", 7777).unwrap();
        assert_ne!(second, first);
        drop(busy);
        assert_eq!(tunnel_port(&file, "hub-host", 7777).unwrap(), second);
    }

    #[test]
    fn unreadable_port_file_starts_over() {
        let _ports = TEST_PORTS.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(PORTS_FILE);
        std::fs::write(&file, "not json").unwrap();
        let port = tunnel_port(&file, "hub-host", 7777).unwrap();
        assert_ne!(port, 0);
        assert_eq!(tunnel_port(&file, "hub-host", 7777).unwrap(), port);
    }

    #[test]
    fn clear_ssh_failures_are_fatal() {
        assert!(fatal(
            "channel 2: open failed: connect failed: Connection refused"
        ));
        assert!(fatal(
            "ssh: Could not resolve hostname nowhere: nodename nor servname provided"
        ));
        assert!(fatal("craig@hub-host: Permission denied (publickey)."));
        assert!(fatal("Host key verification failed."));
        assert!(!fatal(
            "Warning: Permanently added 'hub-host' to the list of known hosts."
        ));
    }

    async fn serve_once(response: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
        });
        base
    }

    async fn fake_hub(list: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = tokio::io::AsyncReadExt::read(&mut socket, &mut buf)
                        .await
                        .unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let response = if request.starts_with("POST /api/auth/session ") {
                        "HTTP/1.1 204 No Content\r\nSet-Cookie: blazar_session_ab12=tok; Path=/\r\nConnection: close\r\n\r\n".to_owned()
                    } else if request.starts_with("GET /api/loopback-ports ")
                        && request.contains("blazar_session_ab12=tok")
                    {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{list}",
                            list.len()
                        )
                    } else {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_owned()
                    };
                    let _ =
                        tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
                });
            }
        });
        base
    }

    #[tokio::test]
    async fn forward_only_accepts_ports_on_the_hub_list() {
        let _ports = TEST_PORTS.lock().await;
        let base =
            fake_hub(r#"{"ports":[{"port":5173,"workspace_id":"w1","purpose":"preview"}]}"#).await;
        let dir = tempfile::tempdir().unwrap();
        let hub = RemoteHub::new(Target::Direct(base), &dir.path().join(PORTS_FILE)).unwrap();
        assert_eq!(hub.forward(5173).await, Ok(5173));
        let refused = hub.forward(22).await.unwrap_err();
        assert!(refused.contains("22"), "{refused}");
    }

    #[tokio::test]
    async fn session_cookie_needs_a_blazar_session() {
        let _ports = TEST_PORTS.lock().await;
        let ok = serve_once(
            "HTTP/1.1 204 No Content\r\nSet-Cookie: blazar_session_ab12=tok; Path=/; HttpOnly\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert_eq!(
            session_cookie(&client(), &ok).await.unwrap(),
            "blazar_session_ab12=tok"
        );
        let other = serve_once(
            "HTTP/1.1 204 No Content\r\nSet-Cookie: sid=1; Path=/\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(session_cookie(&client(), &other).await.is_err());
        let page =
            serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
        assert!(session_cookie(&client(), &page).await.is_err());
    }
}
