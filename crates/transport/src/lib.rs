pub mod detached;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("执行失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("命令以退出码 {code} 结束: {stderr}")]
    Command { code: i32, stderr: String },

    #[error("输出不是合法 UTF-8")]
    Encoding,
}

pub type Result<T> = std::result::Result<T, TransportError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Local,
    Ssh,
}

#[derive(Debug, Clone)]
pub struct ExecSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,

    pub stdin: Option<Vec<u8>>,
}

impl ExecSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
        }
    }

    #[must_use]
    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(data.into());
        self
    }

    #[must_use]
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    #[must_use]
    pub fn args<I, S>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(items.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn cwd(mut self, p: impl Into<PathBuf>) -> Self {
        self.cwd = Some(p.into());
        self
    }

    #[must_use]
    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }

    pub(crate) fn to_shell(&self) -> String {
        let mut parts = Vec::new();
        if let Some(cwd) = &self.cwd {
            parts.push(format!("cd {} &&", shell_quote(&cwd.display().to_string())));
        }
        for (k, v) in &self.env {
            parts.push(format!("{k}={}", shell_quote(v)));
        }
        parts.push(shell_quote(&self.program));
        parts.extend(self.args.iter().map(|a| shell_quote(a)));
        parts.join(" ")
    }
}

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ExecOutput {
    pub fn ok(self) -> Result<String> {
        if self.code == 0 {
            Ok(self.stdout)
        } else {
            Err(TransportError::Command {
                code: self.code,
                stderr: self.stderr.trim().to_owned(),
            })
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeHealth {
    pub os: String,
    pub arch: String,
    pub cpus: u32,
    pub mem_gb: u32,
    pub disk_free_gb: u32,
    pub load1: f64,

    pub gpus: Vec<GpuInfo>,

    pub egress: BTreeMap<String, u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vram_mb: u64,
}

#[derive(Clone)]
pub struct ProcessKiller(std::sync::Arc<tokio::sync::Notify>);

impl ProcessKiller {
    pub fn kill(&self) {
        self.0.notify_one();
    }
}

impl std::fmt::Debug for ProcessKiller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProcessKiller")
    }
}

pub struct LineStream {
    pub stdout: futures::stream::BoxStream<'static, String>,

    pub stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>>,

    pub killer: ProcessKiller,

    guard: tokio::task::JoinHandle<()>,
}

impl Drop for LineStream {
    fn drop(&mut self) {
        self.killer.kill();
        self.guard.abort();
    }
}

impl LineStream {
    #[must_use]
    pub fn stderr_summary(&self) -> String {
        self.stderr_tail
            .lock()
            .map(|t| t.join(" | "))
            .unwrap_or_default()
    }
}

const STDERR_TAIL_LINES: usize = 20;

#[async_trait]
pub trait NodeTransport: Send + Sync {
    fn kind(&self) -> TransportKind;

    fn target(&self) -> &str;

    async fn exec(&self, spec: ExecSpec) -> Result<ExecOutput>;

    async fn spawn_lines(&self, spec: ExecSpec) -> Result<LineStream>;

    async fn read_file(&self, path: &Path) -> Result<Vec<u8>> {
        let out = self
            .exec(ExecSpec::new("cat").arg(path.display().to_string()))
            .await?;
        if out.code != 0 {
            return Err(TransportError::Command {
                code: out.code,
                stderr: out.stderr,
            });
        }
        Ok(out.stdout.into_bytes())
    }

    async fn health(&self) -> Result<NodeHealth> {
        let out = self
            .exec(ExecSpec::new("bash").arg("-lc").arg(HEALTH_SCRIPT))
            .await?;

        if out.code != 0 {
            return Err(TransportError::Command {
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
            });
        }
        Ok(parse_health(&out.stdout))
    }
}

const HEALTH_SCRIPT: &str = r#"
. /etc/os-release 2>/dev/null
echo "os=${PRETTY_NAME:-$(uname -s)}"
echo "arch=$(uname -m)"
echo "cpus=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 0)"
echo "mem_gb=$(free -g 2>/dev/null | awk '/^Mem:/{print $2}' || echo 0)"
echo "disk_free_gb=$(df -BG / 2>/dev/null | awk 'NR==2{gsub(/G/,"",$4); print $4}' || echo 0)"
echo "load1=$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || echo 0)"
command -v nvidia-smi >/dev/null 2>&1 && \
  nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>/dev/null | \
  while IFS=, read -r n m; do echo "gpu=${n# }|${m# }"; done
for t in openai:api.openai.com/v1/models anthropic:api.anthropic.com/v1/models github:github.com; do
  n=${t%%:*}; u=${t#*:}
  echo "egress_$n=$(curl -s -o /dev/null -w '%{http_code}' --max-time 6 "https://$u" 2>/dev/null || echo 000)"
done
"#;

fn parse_health(raw: &str) -> NodeHealth {
    let mut h = NodeHealth::default();
    for line in raw.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "os" => h.os = v.to_owned(),
            "arch" => h.arch = v.to_owned(),
            "cpus" => h.cpus = v.parse().unwrap_or(0),
            "mem_gb" => h.mem_gb = v.parse().unwrap_or(0),
            "disk_free_gb" => h.disk_free_gb = v.parse().unwrap_or(0),
            "load1" => h.load1 = v.parse().unwrap_or(0.0),
            "gpu" => {
                if let Some((name, vram)) = v.split_once('|') {
                    h.gpus.push(GpuInfo {
                        name: name.trim().to_owned(),
                        vram_mb: vram.trim().parse().unwrap_or(0),
                    });
                }
            }
            other => {
                if let Some(name) = other.strip_prefix("egress_") {
                    h.egress.insert(name.to_owned(), v.parse().unwrap_or(0));
                }
            }
        }
    }
    h
}

pub struct LocalTransport;

#[async_trait]
impl NodeTransport for LocalTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Local
    }

    fn target(&self) -> &str {
        "local"
    }

    async fn exec(&self, spec: ExecSpec) -> Result<ExecOutput> {
        let mut cmd = self.build(&spec);

        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        run_with_stdin(cmd, spec.stdin.as_deref()).await
    }

    async fn spawn_lines(&self, spec: ExecSpec) -> Result<LineStream> {
        let child = self
            .build(&spec)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        pipe_lines(child)
    }
}

impl LocalTransport {
    fn build(&self, spec: &ExecSpec) -> Command {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args).stdin(Stdio::null());
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        cmd
    }
}

async fn run_with_stdin(mut cmd: Command, stdin: Option<&[u8]>) -> Result<ExecOutput> {
    use tokio::io::AsyncWriteExt;
    let Some(data) = stdin else {
        let out = cmd.output().await?;
        return Ok(to_output(&out));
    };
    cmd.stdin(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let Some(mut w) = child.stdin.take() {
        w.write_all(data).await?;
        w.shutdown().await?;
        drop(w);
    }
    let out = child.wait_with_output().await?;
    Ok(to_output(&out))
}

fn to_output(out: &std::process::Output) -> ExecOutput {
    ExecOutput {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

async fn kill_tree(root: u32) {
    let Ok(out) = tokio::process::Command::new("ps")
        .args(["-eo", "pid=,ppid="])
        .output()
        .await
    else {
        return;
    };
    let table: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace().map(str::parse::<u32>);
            Some((it.next()?.ok()?, it.next()?.ok()?))
        })
        .collect();

    let mut victims = descendants(root, &table);
    victims.push(root);
    let ids: Vec<String> = victims.iter().map(u32::to_string).collect();
    for sig in ["-TERM", "-KILL"] {
        let _ = tokio::process::Command::new("kill")
            .arg(sig)
            .args(&ids)
            .stderr(Stdio::null())
            .status()
            .await;
        if sig == "-TERM" {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

fn descendants(root: u32, table: &[(u32, u32)]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut frontier = vec![root];
    while !frontier.is_empty() {
        let next: Vec<u32> = table
            .iter()
            .filter(|(pid, ppid)| frontier.contains(ppid) && *pid != root && !out.contains(pid))
            .map(|(pid, _)| *pid)
            .collect();
        out.extend(&next);
        frontier = next;
    }
    out
}

fn pipe_lines(mut child: tokio::process::Child) -> Result<LineStream> {
    use futures::StreamExt;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdout = child.stdout.take().ok_or(TransportError::Encoding)?;
    let stderr_tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    if let Some(stderr) = child.stderr.take() {
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::debug!(target: "blazar::transport", "stderr: {l}");
                if let Ok(mut t) = tail.lock() {
                    if t.len() == STDERR_TAIL_LINES {
                        t.remove(0);
                    }
                    t.push(l);
                }
            }
        });
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(1024);
    let killer = ProcessKiller(std::sync::Arc::new(tokio::sync::Notify::new()));
    let kill_signal = killer.0.clone();

    let guard = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let killed = loop {
            tokio::select! {

                () = kill_signal.notified() => break true,
                line = lines.next_line() => match line {
                    Ok(Some(l)) => {
                        if tx.send(l).await.is_err() {
                            break true;
                        }
                    }
                    _ => break false,
                },
            }
        };
        if killed {
            if let Some(pid) = child.id() {
                kill_tree(pid).await;
            }
            let _ = child.kill().await;
            return;
        }

        tokio::select! {
            () = kill_signal.notified() => {
                if let Some(pid) = child.id() {
                    kill_tree(pid).await;
                }
                let _ = child.kill().await;
            }
            r = child.wait() => {
                if let Err(err) = r {
                    tracing::warn!(target: "blazar::transport", "回收子进程失败: {err}");
                }
            }
        }
    });

    Ok(LineStream {
        stdout: tokio_stream::wrappers::ReceiverStream::new(rx).boxed(),
        stderr_tail,
        killer,
        guard,
    })
}

pub struct SshTransport {
    host: String,
    connect_timeout_secs: u32,

    multiplex: Option<PathBuf>,
}

impl SshTransport {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            connect_timeout_secs: 10,
            multiplex: None,
        }
    }

    #[must_use]
    pub fn multiplexed(mut self, control_dir: impl Into<PathBuf>) -> Self {
        let dir = control_dir.into();
        if std::fs::create_dir_all(&dir).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
            self.multiplex = Some(dir);
        }
        self
    }

    #[must_use]
    pub fn connect_timeout(mut self, secs: u32) -> Self {
        self.connect_timeout_secs = secs;
        self
    }
}

#[async_trait]
impl NodeTransport for SshTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Ssh
    }

    fn target(&self) -> &str {
        &self.host
    }

    async fn exec(&self, spec: ExecSpec) -> Result<ExecOutput> {
        let mut cmd = self.build(&spec);

        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        run_with_stdin(cmd, spec.stdin.as_deref()).await
    }

    async fn spawn_lines(&self, spec: ExecSpec) -> Result<LineStream> {
        let child = self
            .build_streaming(&spec)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        pipe_lines(child)
    }
}

const COLLECT_TREE: &str = r#"T=$C; F=$C; while [ -n "$F" ]; do N=$(ps -eo pid=,ppid= | awk -v f=" $F " 'index(f, " "$2" ")>0 {print $1}' | tr '\n' ' '); T="$T $N"; F=$(echo $N); done;"#;

fn wrap_with_watchdog(inner: &str) -> String {
    format!(
        "set -m; {{ {inner}; }} </dev/null & C=$!; \
         {{ cat >/dev/null; {tree} kill -TERM -$C $T 2>/dev/null; sleep 2; kill -KILL -$C $T 2>/dev/null; }} & W=$!; \
         wait $C; E=$?; kill $W 2>/dev/null; exit $E",
        tree = COLLECT_TREE,
    )
}

impl SshTransport {
    fn ssh_base(&self, remote: &str) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg(format!("ConnectTimeout={}", self.connect_timeout_secs))
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("NumberOfPasswordPrompts=0")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-o")
            .arg("ServerAliveInterval=30")
            .arg("-o")
            .arg("ServerAliveCountMax=3");
        if let Some(dir) = &self.multiplex {
            cmd.arg("-o")
                .arg("ControlMaster=auto")
                .arg("-o")
                .arg(format!("ControlPath={}/%C", dir.display()))
                .arg("-o")
                .arg("ControlPersist=600")
                .arg("-o")
                .arg("ClearAllForwardings=yes");
        }
        cmd.arg(&self.host)
            .arg("--")
            .arg(format!("bash -lc {remote}"));
        cmd
    }

    fn build(&self, spec: &ExecSpec) -> Command {
        let mut cmd = self.ssh_base(&shell_quote(&spec.to_shell()));
        cmd.stdin(Stdio::null());
        cmd
    }

    fn build_streaming(&self, spec: &ExecSpec) -> Command {
        let mut cmd = self.ssh_base(&shell_quote(&wrap_with_watchdog(&spec.to_shell())));

        cmd.stdin(Stdio::piped());
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quoting_survives_spaces_and_quotes() {
        let spec = ExecSpec::new("grep").arg("it's a test").cwd("/tmp/my dir");
        let rendered = spec.to_shell();
        assert!(rendered.contains(r"'it'\''s a test'"));
        assert!(rendered.contains("cd '/tmp/my dir'"));
    }

    #[test]
    fn env_is_rendered_before_program() {
        let spec = ExecSpec::new("codex").env("CODEX_HOME", "/dev/shm/x");
        let rendered = spec.to_shell();
        assert!(rendered.starts_with("CODEX_HOME='/dev/shm/x' 'codex'"));
    }

    #[test]
    fn health_parses_gpus_and_egress() {
        let raw = "os=Ubuntu 22.04\narch=x86_64\ncpus=128\nmem_gb=503\n\
                   disk_free_gb=675\nload1=1.5\n\
                   gpu=NVIDIA GeForce RTX 5090|32607\n\
                   gpu=NVIDIA GeForce RTX 5090|32607\n\
                   egress_openai=000\negress_anthropic=403\negress_github=200\n";
        let h = parse_health(raw);
        assert_eq!(h.cpus, 128);
        assert_eq!(h.gpus.len(), 2);
        assert_eq!(h.gpus[0].vram_mb, 32607);

        assert_eq!(h.egress.get("github"), Some(&200));
        assert_eq!(h.egress.get("openai"), Some(&0));
    }

    #[tokio::test]
    async fn local_exec_roundtrip() {
        let t = LocalTransport;
        let out = t.exec(ExecSpec::new("echo").arg("hi")).await.unwrap();
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout.trim(), "hi");
    }

    #[tokio::test]
    async fn nonzero_exit_becomes_error() {
        let t = LocalTransport;
        let out = t.exec(ExecSpec::new("false")).await.unwrap();
        assert!(out.ok().is_err());
    }

    #[tokio::test]
    async fn dropping_the_stream_actually_kills_the_process() {
        let mark = std::env::temp_dir().join("blazar-kill-probe.txt");
        let _ = std::fs::remove_file(&mark);
        let script = format!(
            "for i in $(seq 1 100); do echo x >> {} ; sleep 0.2; done",
            mark.display()
        );
        let lines = LocalTransport
            .spawn_lines(ExecSpec::new("bash").arg("-c").arg(&script))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let before = std::fs::read_to_string(&mark)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        assert!(before > 0, "子进程应当已经在写了");

        drop(lines);
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let after = std::fs::read_to_string(&mark)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let _ = std::fs::remove_file(&mark);
        assert_eq!(
            before, after,
            "drop 之后进程不能再写东西（{before} → {after} 行）"
        );
    }

    #[tokio::test]
    async fn killer_stops_a_process_that_produces_no_output() {
        let mark = std::env::temp_dir().join("blazar-kill-probe2.txt");
        let _ = std::fs::remove_file(&mark);
        let script = format!(
            "exec >/dev/null; for i in $(seq 1 100); do echo x >> {} ; sleep 0.2; done",
            mark.display()
        );
        let lines = LocalTransport
            .spawn_lines(ExecSpec::new("bash").arg("-c").arg(&script))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let before = std::fs::read_to_string(&mark)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        assert!(before > 0, "子进程应当已经在写了");

        lines.killer.kill();
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let after = std::fs::read_to_string(&mark)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let _ = std::fs::remove_file(&mark);
        assert_eq!(before, after, "killer 要能掐掉一个不产出任何输出的进程");

        drop(lines);
    }

    #[test]
    fn remote_sessions_carry_a_watchdog() {
        let w = wrap_with_watchdog("'claude' '-p' 'hi'");
        assert!(
            w.contains("set -m"),
            "后台作业要自成进程组才能整组收掉: {w}"
        );
        assert!(
            w.contains("cat >/dev/null"),
            "用 stdin EOF 当通道断开的信号: {w}"
        );
        assert!(
            w.contains("kill -TERM -$C"),
            "杀的是进程组不是单个进程: {w}"
        );
        assert!(w.contains("kill -KILL -$C"), "TERM 之后要有 KILL 兜底: {w}");
        assert!(
            w.contains("exit $E"),
            "退出码要透传，否则失败会被当成成功: {w}"
        );
    }

    #[test]
    fn descendants_walks_the_whole_tree_not_just_children() {
        let t = [(10, 1), (11, 1), (100, 10), (1000, 100), (20, 2)];
        let mut d = descendants(1, &t);
        d.sort_unstable();
        assert_eq!(d, vec![10, 11, 100, 1000]);
        assert!(descendants(99, &t).is_empty());
    }

    #[tokio::test]
    async fn killing_reaches_grandchildren_in_other_process_groups() {
        let mark = std::env::temp_dir().join("blazar-tree-probe.txt");
        let _ = std::fs::remove_file(&mark);
        let script = format!(
            "(command -v setsid >/dev/null && setsid bash -c 'while :; do echo x >> {m}; sleep 0.2; done' \
               || bash -c 'set -m; (while :; do echo x >> {m}; sleep 0.2; done) & wait') & \
             echo started; wait",
            m = mark.display()
        );
        let lines = LocalTransport
            .spawn_lines(ExecSpec::new("bash").arg("-c").arg(&script))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        lines.killer.kill();
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let before = std::fs::read_to_string(&mark)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let after = std::fs::read_to_string(&mark)
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let _ = std::fs::remove_file(&mark);
        assert!(before > 0, "孙进程应当已经在写了");
        assert_eq!(
            before, after,
            "掐断之后孙进程也不能再写（{before} → {after}）"
        );
    }
}
