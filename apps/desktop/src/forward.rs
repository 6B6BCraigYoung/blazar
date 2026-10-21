use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::remote::{self, RemoteHub, Tunnel};

pub const PORTS_FILE: &str = "forward-ports.json";
pub const LIST_PATH: &str = "/api/loopback-ports";
const DEST: &str = "localhost";
const SYNC_EVERY: Duration = Duration::from_secs(30);

pub fn allowed_ports(list: &Value) -> Vec<u16> {
    list["ports"]
        .as_array()
        .map(|ports| {
            ports
                .iter()
                .filter_map(|p| p["port"].as_u64())
                .filter_map(|p| u16::try_from(p).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub fn check(port: u16, allowed: &[u16]) -> Result<(), String> {
    if allowed.contains(&port) {
        Ok(())
    } else {
        Err(format!("hub 没有交出 {port} 端口，不能转发"))
    }
}

pub struct Forwards {
    program: OsString,
    alias: String,
    file: PathBuf,
    open: Mutex<BTreeMap<u16, Arc<Tunnel>>>,
}

impl Forwards {
    pub fn new(alias: &str, file: PathBuf) -> Self {
        Self::with_program("ssh".into(), alias, file)
    }

    fn with_program(program: OsString, alias: &str, file: PathBuf) -> Self {
        Self {
            program,
            alias: alias.to_owned(),
            file,
            open: Mutex::default(),
        }
    }

    pub fn open(&self, port: u16, allowed: &[u16]) -> Result<Arc<Tunnel>, String> {
        check(port, allowed)?;
        let mut open = self.open.lock().unwrap();
        if let Some(tunnel) = open.get(&port) {
            return Ok(tunnel.clone());
        }
        let local = remote::tunnel_port(&self.file, &self.alias, port)
            .map_err(|e| format!("找不到空闲端口：{e}"))?;
        let tunnel = Tunnel::spawn(&self.program, &self.alias, DEST, port, local);
        tracing::info!(port, local, "开始转发预览端口");
        open.insert(port, tunnel.clone());
        Ok(tunnel)
    }

    pub fn close(&self, port: u16) {
        if let Some(tunnel) = self.open.lock().unwrap().remove(&port) {
            tunnel.shutdown();
        }
    }

    pub fn retain(&self, allowed: &[u16]) {
        self.open.lock().unwrap().retain(|port, tunnel| {
            let keep = allowed.contains(port);
            if !keep {
                tracing::info!(port, "hub 不再交出这个端口，停止转发");
                tunnel.shutdown();
            }
            keep
        });
    }

    pub fn is_empty(&self) -> bool {
        self.open.lock().unwrap().is_empty()
    }

    pub fn kick(&self) {
        for tunnel in self.open.lock().unwrap().values() {
            tunnel.kick();
        }
    }

    pub fn shutdown(&self) {
        for tunnel in std::mem::take(&mut *self.open.lock().unwrap()).into_values() {
            tunnel.shutdown();
        }
    }
}

pub async fn wait_listening(tunnel: &Tunnel) -> Result<(), String> {
    let deadline = Instant::now() + remote::READY_TIMEOUT;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", tunnel.local_port()))
            .await
            .is_ok()
        {
            return Ok(());
        }
        let line = tunnel.last_error();
        if remote::fatal(&line) {
            return Err(line);
        }
        if Instant::now() >= deadline {
            return Err(if line.is_empty() {
                format!("{} 秒内没有转发上", remote::READY_TIMEOUT.as_secs())
            } else {
                line
            });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

pub async fn keep_in_sync(r: Arc<RemoteHub>) {
    let Some(forwards) = &r.forwards else { return };
    loop {
        tokio::time::sleep(SYNC_EVERY).await;
        if forwards.is_empty() {
            continue;
        }
        match r.loopback_ports().await {
            Ok(allowed) => forwards.retain(&allowed),
            Err(e) => tracing::debug!("这次没对上转发列表：{e}"),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    use serde_json::json;

    struct Fake {
        dir: tempfile::TempDir,
        forwards: Forwards,
    }

    impl Fake {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;

            let dir = tempfile::tempdir().unwrap();
            let program = dir.path().join("ssh");
            std::fs::write(
                &program,
                "#!/bin/sh\nd=$(dirname \"$0\")\necho \"$$ $*\" >> \"$d/log\"\n[ -f \"$d/down\" ] && exit 255\nexec sleep 60\n",
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let forwards = Forwards::with_program(
                program.into_os_string(),
                "hub-host",
                dir.path().join(PORTS_FILE),
            );
            Self { dir, forwards }
        }

        fn runs(&self) -> Vec<(String, String)> {
            std::fs::read_to_string(self.dir.path().join("log"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.split_once(' '))
                .map(|(pid, args)| (pid.to_owned(), args.to_owned()))
                .collect()
        }

        async fn wait_runs(&self, n: usize, within: Duration) -> Vec<(String, String)> {
            let deadline = Instant::now() + within;
            loop {
                let runs = self.runs();
                if runs.len() >= n || Instant::now() >= deadline {
                    return runs;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.forwards.shutdown();
        }
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn hub_list_becomes_allowed_ports() {
        let list = json!({ "ports": [
            { "port": 5173, "workspace_id": "w1", "purpose": "preview" },
            { "port": 51234, "workspace_id": "w2", "purpose": "preview-tunnel" },
            { "port": 70000 },
            { "workspace_id": "w3" }
        ] });
        assert_eq!(allowed_ports(&list), [5173, 51234]);
        assert!(allowed_ports(&json!({ "error": "x" })).is_empty());
    }

    #[tokio::test]
    async fn ports_the_hub_did_not_hand_out_are_refused() {
        let _ports = remote::TEST_PORTS.lock().await;
        let fake = Fake::new();
        let err = fake.forwards.open(22, &[5173, 3000]).err().unwrap();
        assert!(err.contains("22"), "{err}");
        assert!(fake.forwards.open(5173, &[]).is_err());
        assert!(fake.forwards.is_empty());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(fake.runs().is_empty());
        assert!(check(3000, &[5173, 3000]).is_ok());
    }

    #[tokio::test]
    async fn each_port_gets_its_own_ssh_to_localhost() {
        let _ports = remote::TEST_PORTS.lock().await;
        let fake = Fake::new();
        let a = fake.forwards.open(5173, &[5173, 3000]).unwrap();
        let b = fake.forwards.open(3000, &[5173, 3000]).unwrap();
        let runs = fake.wait_runs(2, Duration::from_secs(3)).await;
        assert_eq!(runs.len(), 2);
        let args: Vec<&str> = runs.iter().map(|(_, a)| a.as_str()).collect();
        for (tunnel, port) in [(&a, 5173), (&b, 3000)] {
            let forward = format!("127.0.0.1:{}:localhost:{port}", tunnel.local_port());
            assert!(
                args.iter().any(|a| a.starts_with("-N -o BatchMode=yes")
                    && a.contains(&forward)
                    && a.ends_with("-- hub-host")),
                "{args:?}"
            );
        }
    }

    #[tokio::test]
    async fn local_port_is_reused_per_remote_port() {
        let _ports = remote::TEST_PORTS.lock().await;
        let fake = Fake::new();
        let allowed = [5173, 3000];
        let first = fake.forwards.open(5173, &allowed).unwrap();
        let again = fake.forwards.open(5173, &allowed).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        let local = first.local_port();
        fake.wait_runs(1, Duration::from_secs(3)).await;
        fake.forwards.close(5173);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let reopened = fake.forwards.open(5173, &allowed).unwrap();
        assert_eq!(reopened.local_port(), local);
        let other = fake.forwards.open(3000, &allowed).unwrap();
        assert_ne!(other.local_port(), local);
    }

    #[tokio::test]
    async fn forwards_the_hub_no_longer_lists_are_closed() {
        let _ports = remote::TEST_PORTS.lock().await;
        let fake = Fake::new();
        let gone = fake.forwards.open(5173, &[5173, 3000]).unwrap();
        let kept = fake.forwards.open(3000, &[5173, 3000]).unwrap();
        let runs = fake.wait_runs(2, Duration::from_secs(3)).await;
        let pid_of = |t: &Tunnel| {
            let forward = format!("127.0.0.1:{}:", t.local_port());
            runs.iter()
                .find(|(_, a)| a.contains(&forward))
                .map(|(p, _)| p.clone())
                .unwrap()
        };
        let (gone_pid, kept_pid) = (pid_of(&gone), pid_of(&kept));
        fake.forwards.retain(&[3000, 8080]);
        let deadline = Instant::now() + Duration::from_secs(3);
        while alive(&gone_pid) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!alive(&gone_pid));
        assert!(alive(&kept_pid));
        assert!(
            fake.forwards
                .open(3000, &[3000])
                .is_ok_and(|t| Arc::ptr_eq(&t, &kept))
        );
        fake.forwards.retain(&[]);
        assert!(fake.forwards.is_empty());
    }

    #[tokio::test]
    async fn forwards_come_back_as_soon_as_the_hub_reconnects() {
        let _ports = remote::TEST_PORTS.lock().await;
        let fake = Fake::new();
        std::fs::write(fake.dir.path().join("down"), "").unwrap();
        fake.forwards.open(5173, &[5173]).unwrap();
        assert_eq!(fake.wait_runs(2, Duration::from_secs(4)).await.len(), 2);
        std::fs::remove_file(fake.dir.path().join("down")).unwrap();
        fake.forwards.kick();
        let runs = fake.wait_runs(3, Duration::from_millis(900)).await;
        assert_eq!(runs.len(), 3, "{runs:?}");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(alive(&runs[2].0));
        assert_eq!(fake.runs().len(), 3);
    }
}
