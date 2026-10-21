use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use blazar_transport::ExecSpec;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use tokio::sync::Mutex;

use crate::api::Shared;
use crate::state::ServerEvent;

const MAX_SCRIPT: usize = 8000;
const KEEP_OUTPUT: usize = 64 * 1024;
const SETUP_TIMEOUT: u64 = 900;
const CLEANUP_TIMEOUT: u64 = 300;
const LISTEN_AFTER_SECS: u64 = 5;
const LISTEN_RETRY: Duration = Duration::from_secs(10);

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub(crate) fn cd_target(path: &str) -> String {
    let p = path.trim();
    match p.strip_prefix("~/") {
        Some(rest) => format!("~/{}", q(rest)),
        None if p == "~" || p.is_empty() => "~".to_owned(),
        None => q(p),
    }
}

fn changed(st: &Shared, ws: &str) {
    if let Ok(id) = ws.parse() {
        st.emit(ServerEvent::ScriptsChanged {
            workspace_id: blazar_core_types::WorkspaceId(id),
        });
    }
}

struct Place {
    node: String,
    path: String,
    repo_root: Option<String>,
}

async fn place(st: &Shared, ws: &str) -> Option<Place> {
    let r = sqlx::query(
        "SELECT n.name AS node, w.path, w.repo_root FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()??;
    Some(Place {
        node: r.try_get("node").ok()?,
        path: r.try_get("path").ok()?,
        repo_root: r.try_get("repo_root").ok().flatten(),
    })
}

async fn load(st: &Shared, ws: &str) -> Value {
    let r = sqlx::query("SELECT setup, cleanup, dev, copy_files, preview_port, updated_at FROM workspace_scripts WHERE workspace_id = ?1")
        .bind(ws)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
    let s = |k: &str| {
        r.as_ref()
            .and_then(|r| r.try_get::<String, _>(k).ok())
            .unwrap_or_default()
    };
    json!({ "setup": s("setup"), "cleanup": s("cleanup"), "dev": s("dev"), "copy_files": s("copy_files"),
            "preview_port": r.as_ref().and_then(|r| r.try_get::<Option<i64>, _>("preview_port").ok().flatten()),
            "updated_at": r.as_ref().and_then(|r| r.try_get::<String, _>("updated_at").ok()) })
}

fn preview_port_of(v: Option<i64>) -> Result<Option<u16>, String> {
    match v {
        None => Ok(None),
        Some(p) => u16::try_from(p)
            .ok()
            .filter(|p| *p > 0)
            .map(Some)
            .ok_or_else(|| "预览端口要在 1 到 65535 之间".to_owned()),
    }
}

async fn preview_port(st: &Shared, ws: &str) -> Option<u16> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT preview_port FROM workspace_scripts WHERE workspace_id = ?1")
            .bind(ws)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .flatten();
    preview_port_of(v).ok().flatten()
}

pub async fn get(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    Json(load(&st, &id).await).into_response()
}

#[derive(Deserialize, Default)]
pub struct ScriptsBody {
    #[serde(default)]
    pub setup: String,
    #[serde(default)]
    pub cleanup: String,
    #[serde(default)]
    pub dev: String,
    #[serde(default)]
    pub copy_files: String,
    #[serde(default)]
    pub preview_port: Option<i64>,
}

fn check_copy(list: &str) -> Result<(), String> {
    for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('/') || line.starts_with('~') || line.split('/').any(|s| s == "..") {
            return Err(format!("拷贝清单里只能写仓库内的相对路径：{line}"));
        }
        if line.chars().any(char::is_control) {
            return Err("拷贝清单里有控制字符".into());
        }
    }
    Ok(())
}

pub async fn put(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<ScriptsBody>,
) -> Response {
    for (name, v) in [
        ("setup", &b.setup),
        ("cleanup", &b.cleanup),
        ("dev", &b.dev),
        ("copy_files", &b.copy_files),
    ] {
        if v.len() > MAX_SCRIPT {
            return fail(
                StatusCode::BAD_REQUEST,
                format!("{name} 太长了（上限 {MAX_SCRIPT} 字节）"),
            );
        }
    }
    if let Err(e) = check_copy(&b.copy_files) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    let port = match preview_port_of(b.preview_port) {
        Ok(p) => p.map(i64::from),
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let r = sqlx::query(
        "INSERT INTO workspace_scripts (workspace_id, setup, cleanup, dev, copy_files, preview_port, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (workspace_id) DO UPDATE SET setup = excluded.setup, cleanup = excluded.cleanup,
             dev = excluded.dev, copy_files = excluded.copy_files, preview_port = excluded.preview_port,
             updated_at = excluded.updated_at",
    )
    .bind(&id)
    .bind(b.setup.trim())
    .bind(b.cleanup.trim())
    .bind(b.dev.trim())
    .bind(b.copy_files.trim())
    .bind(port)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    match r {
        Ok(_) => Json(load(&st, &id).await).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn tail(s: &str, keep: usize) -> String {
    if s.len() <= keep {
        return s.to_owned();
    }
    let mut at = s.len() - keep;
    while !s.is_char_boundary(at) {
        at += 1;
    }
    format!("…（前面省略 {} 字节）\n{}", at, &s[at..])
}

async fn run_script(
    st: &Shared,
    ws: &str,
    kind: &str,
    trigger: &str,
    script: String,
    timeout: u64,
) -> (String, bool) {
    let id = uuid::Uuid::now_v7().to_string();
    let _ = sqlx::query(
        "INSERT INTO script_runs (id, workspace_id, kind, trigger, status, started_at) VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
    )
    .bind(&id)
    .bind(ws)
    .bind(kind)
    .bind(trigger)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    changed(st, ws);
    let Some(p) = place(st, ws).await else {
        return (id, false);
    };

    let body = format!(
        "cd {} || exit 97\nexec 2>&1\n{script}\n",
        cd_target(&p.path)
    );
    let spec = ExecSpec::new("bash")
        .arg("-l")
        .arg("-s")
        .stdin(body.into_bytes());
    let out = tokio::time::timeout(
        Duration::from_secs(timeout),
        st.transport(&p.node).exec(spec),
    )
    .await;
    let (status, code, output) = match out {
        Ok(Ok(o)) => (
            if o.code == 0 { "ok" } else { "failed" },
            Some(o.code),
            format!("{}{}", o.stdout, o.stderr),
        ),
        Ok(Err(e)) => ("failed", None, format!("连不上 {}：{e}", p.node)),
        Err(_) => (
            "timeout",
            None,
            format!("超过 {timeout} 秒没跑完，已放弃等待"),
        ),
    };
    let _ = sqlx::query(
        "UPDATE script_runs SET status = ?2, exit_code = ?3, output = ?4, finished_at = ?5 WHERE id = ?1",
    )
    .bind(&id)
    .bind(status)
    .bind(code)
    .bind(tail(&output, KEEP_OUTPUT))
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    changed(st, ws);
    (id, status == "ok")
}

pub async fn run(State(st): State<Shared>, Path((id, kind)): Path<(String, String)>) -> Response {
    if !matches!(kind.as_str(), "setup" | "cleanup" | "copy") {
        return fail(
            StatusCode::NOT_FOUND,
            "只有 setup / cleanup / copy 能这样跑",
        );
    }
    let cfg = load(&st, &id).await;
    if kind == "copy" {
        let st2 = st.clone();
        tokio::spawn(async move { copy_files(&st2, &id, "manual").await });
        return Json(json!({ "started": true })).into_response();
    }
    let script = cfg[kind.as_str()].as_str().unwrap_or_default().to_owned();
    if script.trim().is_empty() {
        return fail(StatusCode::CONFLICT, "这个脚本还是空的");
    }
    let st2 = st.clone();
    let timeout = if kind == "setup" {
        SETUP_TIMEOUT
    } else {
        CLEANUP_TIMEOUT
    };
    tokio::spawn(async move { run_script(&st2, &id, &kind, "manual", script, timeout).await });
    Json(json!({ "started": true })).into_response()
}

pub async fn runs(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let rows = sqlx::query(
        "SELECT id, kind, trigger, status, exit_code, output, started_at, finished_at FROM script_runs
         WHERE workspace_id = ?1 ORDER BY started_at DESC LIMIT 20",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    Json(
        rows.iter()
            .map(|r| {
                let s = |k: &str| r.try_get::<Option<String>, _>(k).ok().flatten();
                json!({ "id": s("id"), "kind": s("kind"), "trigger": s("trigger"), "status": s("status"),
                        "exit_code": r.try_get::<Option<i64>, _>("exit_code").ok().flatten(),
                        "output": s("output"), "started_at": s("started_at"), "finished_at": s("finished_at") })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn copy_files(st: &Shared, ws: &str, trigger: &str) {
    let cfg = load(st, ws).await;
    let list = cfg["copy_files"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let Some(p) = place(st, ws).await else { return };
    let Some(src) = p.repo_root.filter(|r| !r.is_empty() && *r != p.path) else {
        if trigger == "manual" {
            run_script(
                st,
                ws,
                "copy",
                trigger,
                "echo '这不是隔离工作区（没有源仓库），没什么可拷的'; exit 1".into(),
                30,
            )
            .await;
        }
        return;
    };
    if list.is_empty() {
        return;
    }
    let script = format!(
        r#"SRC={src}; DST="$PWD"
cd "$SRC" || {{ echo "源仓库 $SRC 不在了"; exit 1; }}
N=0
while IFS= read -r pat; do
  [ -z "$pat" ] && continue
  case "$pat" in /*|*..*) echo "跳过：$pat"; continue;; esac
  FOUND=$(compgen -G "$pat") || {{ echo "没匹配到：$pat"; continue; }}
  while IFS= read -r f; do
    mkdir -p "$DST/$(dirname "$f")" && cp -Rp "$f" "$DST/$f" && {{ echo "已拷贝 $f"; N=$((N+1)); }}
  done <<< "$FOUND"
done <<'BLAZAR_COPY_LIST'
{list}
BLAZAR_COPY_LIST
echo "共 $N 项""#,
        src = cd_target(&src),
        list = list
            .lines()
            .filter(|l| l.trim() != "BLAZAR_COPY_LIST")
            .collect::<Vec<_>>()
            .join("\n"),
    );
    run_script(st, ws, "copy", trigger, script, 120).await;
}

pub async fn on_workspace_created(st: Shared, ws: String, repo_root: String) {
    let prev: Option<String> = sqlx::query_scalar(
        "SELECT s.workspace_id FROM workspace_scripts s JOIN workspaces w ON w.id = s.workspace_id
         WHERE w.repo_root = ?1 AND w.id != ?2 ORDER BY s.updated_at DESC LIMIT 1",
    )
    .bind(&repo_root)
    .bind(&ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let Some(prev) = prev else { return };
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO workspace_scripts (workspace_id, setup, cleanup, dev, copy_files, preview_port, updated_at)
         SELECT ?1, setup, cleanup, dev, copy_files, preview_port, ?3 FROM workspace_scripts WHERE workspace_id = ?2",
    )
    .bind(&ws)
    .bind(&prev)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    copy_files(&st, &ws, "create").await;
    let setup = load(&st, &ws).await["setup"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if !setup.trim().is_empty() {
        run_script(&st, &ws, "setup", "create", setup, SETUP_TIMEOUT).await;
    }
}

pub async fn on_run_finished(st: Shared, ws: blazar_core_types::WorkspaceId, status: &'static str) {
    if status != "done" {
        return;
    }
    let ws = ws.to_string();
    let cleanup = load(&st, &ws).await["cleanup"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if cleanup.trim().is_empty() {
        return;
    }
    let Some(p) = place(&st, &ws).await else {
        return;
    };
    let probe = format!(
        "cd {} && {{ git rev-parse --git-dir >/dev/null 2>&1 || exit 0; [ -n \"$(git status --porcelain | head -1)\" ]; }}",
        cd_target(&p.path)
    );
    let dirty = st
        .transport(&p.node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(probe))
        .await
        .is_ok_and(|o| o.code == 0);
    if dirty {
        run_script(&st, &ws, "cleanup", "turn_end", cleanup, CLEANUP_TIMEOUT).await;
    }
}

fn dev_dir(ws: &str) -> String {
    let safe: String = ws
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!("${{TMPDIR:-/tmp}}/blazar-dev-{safe}")
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for n in it.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[must_use]
pub fn detect_url(log: &str) -> Option<(u16, String, bool)> {
    let clean = strip_ansi(log);
    let mut found = None;
    for (at, _) in clean.match_indices("http") {
        let rest = &clean[at..];
        let (https, rest) = if let Some(r) = rest.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = rest.strip_prefix("http://") {
            (false, r)
        } else {
            continue;
        };
        let host_end = rest
            .find([' ', '\n', '\r', '\t', '"', '\'', ')', '>'])
            .unwrap_or(rest.len());
        let url = &rest[..host_end];
        let (hostport, path) = url.split_once('/').map_or((url, ""), |(h, p)| (h, p));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p),
            None => continue,
        };
        if !matches!(
            host,
            "localhost" | "127.0.0.1" | "0.0.0.0" | "[::]" | "[::1]"
        ) {
            continue;
        }
        let Ok(port) = port.parse::<u16>() else {
            continue;
        };
        if port < 80 {
            continue;
        }
        found = Some((
            port,
            format!("/{}", path.trim_end_matches(['.', ','])),
            https,
        ));
    }
    found
}

struct Running {
    pid: String,
    age: u64,
}

fn running_of(head: &str) -> Option<Running> {
    let mut it = head.split_whitespace();
    (it.next()? == "__RUNNING__").then(|| Running {
        pid: it.next().unwrap_or_default().to_owned(),
        age: it.next().and_then(|a| a.parse().ok()).unwrap_or(u64::MAX),
    })
}

fn listen_script(dir: &str) -> String {
    format!(
        r#"D={dir}; [ -f "$D/pid" ] || exit 0
echo "__ROOT__ $(cat "$D/pid")"
echo __PS__
ps -eo pid=,ppid=
if [ "$(uname -s)" = Darwin ] || ! command -v ss >/dev/null 2>&1; then echo __LSOF__; lsof -nP -iTCP -sTCP:LISTEN 2>/dev/null
else echo __SS__; ss -ltnpH 2>/dev/null; fi
exit 0"#
    )
}

pub fn descendants(root: u32, tree: &[(u32, u32)]) -> HashSet<u32> {
    let mut found = HashSet::from([root]);
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for &(pid, ppid) in tree {
            if ppid == parent && found.insert(pid) {
                queue.push(pid);
            }
        }
    }
    found
}

fn parse_ps(s: &str) -> Vec<(u32, u32)> {
    s.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect()
}

fn port_of(addr: &str) -> Option<u16> {
    addr.rsplit_once(':')?.1.parse().ok()
}

pub fn parse_lsof(s: &str) -> Vec<(u32, u16)> {
    s.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 9 || !l.contains("(LISTEN)") {
                return None;
            }
            Some((f[1].parse().ok()?, port_of(f[f.len() - 2])?))
        })
        .collect()
}

pub fn parse_ss(s: &str) -> Vec<(u32, u16)> {
    let pids = |l: &str| {
        l.split("pid=")
            .skip(1)
            .filter_map(|p| {
                p.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse::<u32>()
                    .ok()
            })
            .collect::<Vec<_>>()
    };
    let mut out = Vec::new();
    let mut port = None;
    for l in s.lines() {
        if let Some(p) = l
            .strip_prefix("LISTEN")
            .and_then(|r| r.split_whitespace().nth(2))
            .and_then(port_of)
        {
            port = Some(p);
        }
        if let Some(p) = port {
            out.extend(pids(l).into_iter().map(|pid| (pid, p)));
        }
    }
    out
}

pub fn listening_ports(out: &str) -> Vec<u16> {
    let root: Option<u32> = out
        .lines()
        .find_map(|l| l.strip_prefix("__ROOT__"))
        .and_then(|r| r.trim().parse().ok());
    let section = |name: &str, end: &[&str]| -> String {
        let mut lines = out.lines().skip_while(|l| *l != name).skip(1);
        lines
            .by_ref()
            .take_while(|l| !end.contains(l))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let Some(root) = root else {
        return Vec::new();
    };
    let tree = descendants(root, &parse_ps(&section("__PS__", &["__LSOF__", "__SS__"])));
    let sockets = if out.lines().any(|l| l == "__SS__") {
        parse_ss(&section("__SS__", &[]))
    } else {
        parse_lsof(&section("__LSOF__", &[]))
    };
    let mut ports: Vec<u16> = sockets
        .into_iter()
        .filter(|(pid, _)| tree.contains(pid))
        .map(|(_, port)| port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

struct Listen {
    pid: String,
    ports: Vec<u16>,
    at: Instant,
}

fn listens() -> &'static std::sync::Mutex<HashMap<String, Listen>> {
    static L: OnceLock<std::sync::Mutex<HashMap<String, Listen>>> = OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

async fn listening(st: &Shared, node: &str, ws: &str, pid: &str) -> Vec<u16> {
    let cached = listens()
        .lock()
        .unwrap()
        .get(ws)
        .filter(|l| l.pid == pid && (!l.ports.is_empty() || l.at.elapsed() < LISTEN_RETRY))
        .map(|l| l.ports.clone());
    if let Some(ports) = cached {
        return ports;
    }
    let out = st
        .transport(node)
        .exec(
            ExecSpec::new("bash")
                .arg("-lc")
                .arg(listen_script(&dev_dir(ws))),
        )
        .await
        .map(|o| o.stdout)
        .unwrap_or_default();
    let ports = listening_ports(&out);
    listens().lock().unwrap().insert(
        ws.to_owned(),
        Listen {
            pid: pid.to_owned(),
            ports: ports.clone(),
            at: Instant::now(),
        },
    );
    ports
}

fn forget_listen(ws: &str) {
    listens().lock().unwrap().remove(ws);
}

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

async fn ensure_tunnel(ws: &str, node: &str, remote_port: u16) -> Result<u16, String> {
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

async fn close_tunnel(ws: &str) {
    if let Some(mut t) = tunnels().lock().await.remove(ws) {
        let _ = t.child.start_kill();
    }
}

fn handed() -> &'static std::sync::Mutex<HashMap<String, (u16, &'static str)>> {
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

fn remember(ws: &str, v: &Value) {
    let mut map = handed().lock().unwrap();
    match handed_port(v) {
        Some(entry) => map.insert(ws.to_owned(), entry),
        None => map.remove(ws),
    };
}

async fn dev_status(st: &Shared, ws: &str) -> Value {
    let v = probe_dev(st, ws).await;
    remember(ws, &v);
    v
}

pub async fn loopback_ports(State(st): State<Shared>, headers: HeaderMap) -> Response {
    if crate::bots::caller_of(&headers).is_some() {
        return fail(StatusCode::FORBIDDEN, "bot 不能列出本机端口");
    }
    let known: Vec<String> = handed().lock().unwrap().keys().cloned().collect();
    for ws in &known {
        dev_status(&st, ws).await;
    }
    let mut ports: Vec<Value> = handed()
        .lock()
        .unwrap()
        .iter()
        .map(
            |(ws, (port, purpose))| json!({ "port": port, "workspace_id": ws, "purpose": purpose }),
        )
        .collect();
    ports.sort_by_key(|p| p["port"].as_u64());
    Json(json!({ "ports": ports })).into_response()
}

async fn probe_dev(st: &Shared, ws: &str) -> Value {
    let Some(p) = place(st, ws).await else {
        return json!({ "running": false, "reason": "没有这个工作区" });
    };
    let script = format!(
        r#"D={dir}
if [ -f "$D/pid" ] && kill -0 "$(cat "$D/pid")" 2>/dev/null; then
  M=$(stat -c %Y "$D/pid" 2>/dev/null || stat -f %m "$D/pid" 2>/dev/null || echo 0)
  echo "__RUNNING__ $(cat "$D/pid") $(( $(date +%s) - M ))"
else echo "__STOPPED__"; fi
[ -f "$D/log" ] && tail -c 24000 "$D/log""#,
        dir = dev_dir(ws)
    );
    let out = match st
        .transport(&p.node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await
    {
        Ok(o) => o.stdout,
        Err(e) => return json!({ "running": false, "reason": format!("连不上 {}：{e}", p.node) }),
    };
    let (head, log) = out.split_once('\n').unwrap_or((&out, ""));
    let running = running_of(head);
    let log = strip_ansi(log);
    let manual = preview_port(st, ws).await;
    let from_log = detect_url(&log);
    let ports = match (&manual, &from_log, &running) {
        (None, None, Some(r)) if r.age >= LISTEN_AFTER_SECS => {
            listening(st, &p.node, ws, &r.pid).await
        }
        _ => Vec::new(),
    };
    let mut v = json!({ "running": running.is_some(), "node": p.node, "log": log });
    if let Some(a) = choose_address(manual, from_log, &ports) {
        v["port"] = json!(a.port);
        v["path"] = json!(a.path);
        v["source"] = json!(a.source);
        if a.source == "listen" {
            v["ports"] = json!(ports);
        }
        if running.is_some() {
            let (port, path) = (a.port, a.path);
            let scheme = if a.https { "https" } else { "http" };
            if p.node == "local" {
                v["preview_url"] = json!(format!("{scheme}://localhost:{port}{path}"));
            } else {
                match ensure_tunnel(ws, &p.node, port).await {
                    Ok(lp) => {
                        v["preview_url"] = json!(format!("{scheme}://127.0.0.1:{lp}{path}"));
                        v["tunnel"] = json!(lp);
                    }
                    Err(e) => v["tunnel_error"] = json!(e),
                }
            }
        }
    }
    if running.is_none() {
        forget_listen(ws);
        close_tunnel(ws).await;
    }
    v
}

pub async fn dev_get(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    Json(dev_status(&st, &id).await).into_response()
}

pub async fn dev_start(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let cmd = load(&st, &id).await["dev"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if cmd.trim().is_empty() {
        return fail(StatusCode::CONFLICT, "还没配置 dev server 的启动命令");
    }
    let Some(p) = place(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个工作区");
    };

    let script = format!(
        r#"D={dir}; mkdir -p "$D" || exit 1
if [ -f "$D/pid" ] && kill -0 "$(cat "$D/pid")" 2>/dev/null; then echo "已经在跑了"; exit 0; fi
cat > "$D/run.sh" <<'BLAZAR_DEV_CMD'
cd {cwd} || exit 97
{cmd}
BLAZAR_DEV_CMD
: > "$D/log"
if command -v setsid >/dev/null 2>&1; then
  nohup setsid bash -l "$D/run.sh" >> "$D/log" 2>&1 < /dev/null &
elif command -v perl >/dev/null 2>&1; then
  nohup perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' bash -l "$D/run.sh" >> "$D/log" 2>&1 < /dev/null &
else
  nohup bash -l "$D/run.sh" >> "$D/log" 2>&1 < /dev/null &
fi
echo $! > "$D/pid"
echo started"#,
        dir = dev_dir(&id),
        cwd = cd_target(&p.path),
        cmd = cmd
            .lines()
            .filter(|l| l.trim() != "BLAZAR_DEV_CMD")
            .collect::<Vec<_>>()
            .join("\n"),
    );
    match st
        .transport(&p.node)
        .exec(
            ExecSpec::new("bash")
                .arg("-l")
                .arg("-s")
                .stdin(script.into_bytes()),
        )
        .await
    {
        Ok(o) if o.code == 0 => {
            changed(&st, &id);

            tokio::time::sleep(Duration::from_millis(1500)).await;
            Json(dev_status(&st, &id).await).into_response()
        }
        Ok(o) => fail(
            StatusCode::BAD_GATEWAY,
            format!("{}{}", o.stdout, o.stderr).trim().to_owned(),
        ),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

pub async fn dev_stop(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(p) = place(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个工作区");
    };
    let script = format!(
        r#"D={dir}; [ -f "$D/pid" ] || exit 0
P=$(cat "$D/pid")
kill -TERM -- "-$P" 2>/dev/null || kill -TERM "$P" 2>/dev/null
for i in 1 2 3 4 5 6; do kill -0 "$P" 2>/dev/null || break; sleep 0.5; done
kill -KILL -- "-$P" 2>/dev/null; kill -KILL "$P" 2>/dev/null
rm -f "$D/pid"; echo "[blazar] 已停止" >> "$D/log""#,
        dir = dev_dir(&id)
    );
    let _ = st
        .transport(&p.node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await;
    forget_listen(&id);
    close_tunnel(&id).await;
    changed(&st, &id);
    Json(dev_status(&st, &id).await).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ssh_targets_are_rejected_before_preview_tunnel_setup() {
        for node in ["-invalid", "", "Alice@-invalid", "host name"] {
            assert!(ensure_tunnel("test-workspace", node, 30000).await.is_err());
        }
    }

    #[test]
    fn dev_server_urls_are_found_in_real_world_logs() {
        let vite = "\u{1b}[32m  VITE v5.4.1\u{1b}[0m  ready in 312 ms\n\n  \u{1b}[32m➜\u{1b}[0m  Local:   \u{1b}[36mhttp://localhost:\u{1b}[1m5173\u{1b}[22m/\u{1b}[0m\n  ➜  Network: use --host to expose\n";
        assert_eq!(detect_url(vite), Some((5173, "/".into(), false)));
        assert_eq!(
            detect_url("- Local:        http://localhost:3000\n- Environments: .env"),
            Some((3000, "/".into(), false))
        );
        assert_eq!(
            detect_url("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ..."),
            Some((8000, "/".into(), false))
        );
        assert_eq!(
            detect_url("listening on https://127.0.0.1:8443/app."),
            Some((8443, "/app".into(), true))
        );

        assert_eq!(
            detect_url("http://localhost:3000 in use\nLocal: http://localhost:3001/"),
            Some((3001, "/".into(), false))
        );

        assert_eq!(
            detect_url("docs at https://vitejs.dev:443/guide and http://example.com:8080/"),
            None
        );
        assert_eq!(detect_url("no url here"), None);
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

    async fn ports(st: &Shared, headers: HeaderMap) -> (StatusCode, Value) {
        let r = loopback_ports(State(st.clone()), headers).await;
        let status = r.status();
        let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    async fn preview_fixture(dev: &str) -> (Shared, String, tempfile::TempDir) {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let st = crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
            crate::services::Services::Isolated,
        );
        let ws = uuid::Uuid::now_v7().to_string();
        for q in [
            "INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n-local', 'local', 'local', 'online', '0')".to_owned(),
            format!(
                "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('{ws}', 'n-local', 'pv', '{}', '0')",
                root.path().display()
            ),
            format!(
                "INSERT INTO workspace_scripts (workspace_id, setup, cleanup, dev, copy_files, updated_at)
                 VALUES ('{ws}', '', '', '{dev}', '', '0')"
            ),
        ] {
            sqlx::query(&q).execute(st.db.pool()).await.unwrap();
        }
        (st, ws, root)
    }

    async fn wait_for_address(st: &Shared, ws: &str) -> Value {
        for _ in 0..30 {
            let v = dev_status(st, ws).await;
            if v["preview_url"].is_string() {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("the preview never got an address");
    }

    #[test]
    fn listening_ports_come_from_the_dev_process_tree_on_macos() {
        let out = "__ROOT__ 500\n__PS__\n  500     1\n  501   500\n  502   501\n  777     1\n__LSOF__\n\
COMMAND   PID  USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME\n\
Python    502 craig    4u  IPv4 0xafaa59714fcb7f25      0t0  TCP 127.0.0.1:8765 (LISTEN)\n\
node      501 craig   22u  IPv6 0x5f6946e4331ab31a      0t0  TCP [::1]:5173 (LISTEN)\n\
node      501 craig   23u  IPv4 0x5f6946e4331ab31b      0t0  TCP *:24678 (LISTEN)\n\
node      501 craig   24u  IPv6 0x5f6946e4331ab31c      0t0  TCP *:24678 (LISTEN)\n\
rapportd  777 craig   16u  IPv4 0xae788ff549d6b72e      0t0  TCP *:52719 (LISTEN)\n";
        assert_eq!(listening_ports(out), vec![5173, 8765, 24678]);
        assert_eq!(
            listening_ports("__PS__\n1 0\n__LSOF__\n"),
            Vec::<u16>::new()
        );
    }

    #[test]
    fn listening_ports_come_from_the_dev_process_tree_on_linux() {
        let out = "__ROOT__ 41200\n__PS__\n41200 1\n41234 41200\n41301 41200\n41355 41301\n1000 1\n__SS__\n\
LISTEN 0      4096       127.0.0.1:8000       0.0.0.0:*    users:((\"python3\",pid=41234,fd=3))\n\
LISTEN 0      511                *:5173             *:*    users:((\"node\",pid=41301,fd=22))\n\
LISTEN 0      511          0.0.0.0:3000       0.0.0.0:*    users:((\"node\",pid=41301,fd=23),(\"node\",pid=41355,fd=23))\n\
LISTEN 0      128          0.0.0.0:22         0.0.0.0:*    users:((\"sshd\",pid=1000,fd=3))\n\
LISTEN 0      4096   [::ffff:127.0.0.1]:9229       *:*\n\
                                                             users:((\"node\",pid=41301,fd=19))\n\
LISTEN 0      128             [::]:22            [::]:*    users:((\"sshd\",pid=1000,fd=4))\n";
        assert_eq!(listening_ports(out), vec![3000, 5173, 8000, 9229]);
    }

    #[test]
    fn descendants_include_every_generation_but_not_siblings() {
        let tree = [
            (1, 0),
            (10, 1),
            (11, 10),
            (12, 11),
            (13, 11),
            (20, 1),
            (21, 20),
        ];
        assert_eq!(descendants(10, &tree), HashSet::from([10, 11, 12, 13]));
        assert_eq!(descendants(21, &tree), HashSet::from([21]));
        assert_eq!(descendants(99, &[(5, 99), (99, 5)]), HashSet::from([99, 5]));
    }

    #[test]
    fn a_manual_port_beats_the_log_and_the_log_beats_listening_ports() {
        let logged = Some((3000, "/app".to_owned(), true));
        assert_eq!(
            choose_address(Some(4000), logged.clone(), &[8000]),
            Some(Address {
                port: 4000,
                path: "/".into(),
                https: false,
                source: "manual"
            })
        );
        assert_eq!(
            choose_address(None, logged, &[8000]),
            Some(Address {
                port: 3000,
                path: "/app".into(),
                https: true,
                source: "log"
            })
        );
        assert_eq!(
            choose_address(None, None, &[8001, 8000, 35729]),
            Some(Address {
                port: 8000,
                path: "/".into(),
                https: false,
                source: "listen"
            })
        );
        assert_eq!(choose_address(None, None, &[]), None);
        assert_eq!(preview_port_of(None), Ok(None));
        assert_eq!(preview_port_of(Some(3000)), Ok(Some(3000)));
        assert!(preview_port_of(Some(0)).is_err());
        assert!(preview_port_of(Some(70000)).is_err());
        assert!(running_of("__STOPPED__").is_none());
        let r = running_of("__RUNNING__ 4242 7").unwrap();
        assert_eq!((r.pid.as_str(), r.age), ("4242", 7));
        assert_eq!(running_of("__RUNNING__ 4242").unwrap().age, u64::MAX);
    }

    #[tokio::test]
    async fn a_server_that_never_logs_its_address_is_found_by_its_listening_port() {
        let (st, ws, _root) =
            preview_fixture("exec python3 -m http.server 0 --bind 127.0.0.1").await;
        dev_start(State(st.clone()), Path(ws.clone())).await;
        let v = wait_for_address(&st, &ws).await;
        assert_eq!(v["source"], "listen");
        let port = v["port"].as_u64().unwrap();
        assert!(v["ports"].as_array().unwrap().contains(&json!(port)));
        assert_eq!(v["preview_url"], format!("http://localhost:{port}/"));
        let (_, list) = ports(&st, HeaderMap::new()).await;
        assert!(
            list["ports"]
                .as_array()
                .unwrap()
                .contains(&json!({ "port": port, "workspace_id": ws, "purpose": "preview" }))
        );
        dev_stop(State(st.clone()), Path(ws.clone())).await;
        let (_, after) = ports(&st, HeaderMap::new()).await;
        assert!(!after.to_string().contains(&ws));
    }

    #[tokio::test]
    async fn a_manual_preview_port_is_used_without_guessing() {
        let (st, ws, _root) = preview_fixture("exec sleep 30").await;
        let bad = put(
            State(st.clone()),
            Path(ws.clone()),
            Json(ScriptsBody {
                preview_port: Some(70000),
                ..ScriptsBody::default()
            }),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        put(
            State(st.clone()),
            Path(ws.clone()),
            Json(ScriptsBody {
                dev: "exec sleep 30".into(),
                preview_port: Some(4321),
                ..ScriptsBody::default()
            }),
        )
        .await;
        assert_eq!(load(&st, &ws).await["preview_port"], 4321);
        dev_start(State(st.clone()), Path(ws.clone())).await;
        let v = dev_status(&st, &ws).await;
        assert_eq!(v["source"], "manual");
        assert_eq!(v["preview_url"], "http://localhost:4321/");
        let (_, list) = ports(&st, HeaderMap::new()).await;
        assert!(
            list["ports"]
                .as_array()
                .unwrap()
                .contains(&json!({ "port": 4321, "workspace_id": ws, "purpose": "preview" }))
        );
        dev_stop(State(st.clone()), Path(ws.clone())).await;
    }

    #[tokio::test]
    async fn loopback_ports_list_running_previews_and_refuse_bots() {
        let (st, ws, _root) =
            preview_fixture("exec python3 -u -m http.server 0 --bind 127.0.0.1").await;
        let (_, before) = ports(&st, HeaderMap::new()).await;
        assert!(!before.to_string().contains(&ws));

        dev_start(State(st.clone()), Path(ws.clone())).await;
        let v = wait_for_address(&st, &ws).await;
        assert_eq!(v["source"], "log");
        let port = v["port"].as_u64().unwrap();

        let (status, list) = ports(&st, HeaderMap::new()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            list["ports"]
                .as_array()
                .unwrap()
                .contains(&json!({ "port": port, "workspace_id": ws, "purpose": "preview" }))
        );

        let mut bot = HeaderMap::new();
        bot.insert(
            crate::bots::CALLER_HEADER,
            json!({"bot": "b1", "handle": "lead", "role": "lead"})
                .to_string()
                .parse()
                .unwrap(),
        );
        let (status, body) = ports(&st, bot).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.get("ports").is_none());

        dev_stop(State(st.clone()), Path(ws.clone())).await;
        let (_, after) = ports(&st, HeaderMap::new()).await;
        assert!(!after.to_string().contains(&ws));
    }

    #[test]
    fn copy_lists_stay_inside_the_repo() {
        assert!(check_copy(".env\nconfig/*.local.json\n\n  .vscode/settings.json").is_ok());
        for bad in ["/etc/passwd", "../secrets", "a/../../b", "~/.ssh/id_rsa"] {
            assert!(check_copy(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn long_output_keeps_the_tail() {
        let s = "a".repeat(100) + "尾巴";
        let t = tail(&s, 10);
        assert!(t.ends_with("尾巴") && t.starts_with('…'));
        assert_eq!(tail("short", 10), "short");
    }
}
