use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::oneshot;

use crate::office;

const MAX_LINES: usize = 200;
const MAX_LINE: usize = 400;

const KEEP_JOBS: usize = 12;
const QR_MAX: u64 = 300 * 1024;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

#[derive(Clone, Serialize)]
pub struct JobView {
    pub id: String,
    pub app: &'static str,
    pub step: &'static str,

    pub status: &'static str,
    pub message: String,
    pub lines: Vec<String>,

    pub url: Option<String>,

    pub url_trusted: bool,

    pub qr: Option<String>,

    pub code: Option<String>,
    pub exit_code: Option<i32>,
    pub started_at: String,
}

struct Job {
    seq: u64,
    view: JobView,
    cancel: Option<oneshot::Sender<()>>,
    terms: [Term; 2],

    forward: bool,
    want_url: bool,
    want_code: bool,

    hidden: Vec<String>,
}

fn jobs() -> MutexGuard<'static, HashMap<String, Job>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Job>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn latest(app: &str) -> Option<JobView> {
    jobs()
        .values()
        .filter(|j| j.view.app == app)
        .max_by_key(|j| j.seq)
        .map(|j| j.view.clone())
}

enum Outcome {
    Exit(i32),
    Cancelled,
    TimedOut,
    Failed(String),
}

#[derive(Clone, Copy, Default)]
enum Esc {
    #[default]
    None,
    Start,
    Csi,
    Osc,
}

#[derive(Default)]
struct Term {
    raw: Vec<u8>,
    cur: String,
    esc: Esc,
}

impl Term {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.raw.extend_from_slice(bytes);
        let mut text = String::new();
        loop {
            match std::str::from_utf8(&self.raw) {
                Ok(s) => {
                    text.push_str(s);
                    self.raw.clear();
                    break;
                }
                Err(e) => {
                    let ok = e.valid_up_to();
                    text.push_str(std::str::from_utf8(&self.raw[..ok]).unwrap_or_default());
                    match e.error_len() {
                        Some(bad) => {
                            text.push('\u{fffd}');
                            self.raw.drain(..ok + bad);
                        }

                        None => {
                            self.raw.drain(..ok);
                            break;
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for c in text.chars() {
            match (self.esc, c) {
                (Esc::None, '\x1b') => self.esc = Esc::Start,
                (Esc::None, '\n' | '\r') => out.push(std::mem::take(&mut self.cur)),
                (Esc::None, '\t') => self.cur.push(' '),
                (Esc::None, c) if c.is_control() => {}
                (Esc::None, c) => {
                    if self.cur.len() < MAX_LINE * 8 {
                        self.cur.push(c);
                    }
                }
                (Esc::Start, '[') => self.esc = Esc::Csi,
                (Esc::Start, ']') => self.esc = Esc::Osc,
                (Esc::Start, _) => self.esc = Esc::None,
                (Esc::Csi, '\x40'..='\x7e') => {
                    if c == 'G' {
                        out.push(std::mem::take(&mut self.cur));
                    }
                    self.esc = Esc::None;
                }
                (Esc::Csi, _) => {}
                (Esc::Osc, '\x07') => self.esc = Esc::None,
                (Esc::Osc, '\x1b') => self.esc = Esc::Start,
                (Esc::Osc, _) => {}
            }
        }
        out
    }

    fn flush(&mut self) -> Option<String> {
        self.raw.clear();
        (!self.cur.is_empty()).then(|| std::mem::take(&mut self.cur))
    }
}

fn tidy(seg: &str) -> String {
    seg.trim()
        .trim_start_matches(['│', '◒', '◐', '◓', '◑', ' '])
        .chars()
        .take(MAX_LINE)
        .collect()
}

fn same_activity(a: &str, b: &str) -> bool {
    let core = |s: &str| s.trim_end_matches(['.', '…', ' ']).to_owned();
    core(a) == core(b)
}

fn deep_find<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    match v {
        Value::Object(m) => keys
            .iter()
            .find_map(|k| m.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
            .or_else(|| m.values().find_map(|c| deep_find(c, keys))),
        Value::Array(a) => a.iter().find_map(|c| deep_find(c, keys)),
        _ => None,
    }
}

const URL_KEYS: &[&str] = &[
    "verification_uri_complete",
    "verification_url",
    "console_url",
];

fn find_url(line: &str) -> Option<String> {
    if line.trim_start().starts_with('{')
        && let Ok(v) = serde_json::from_str::<Value>(line)
    {
        return deep_find(&v, URL_KEYS)
            .filter(|u| u.starts_with("https://"))
            .map(str::to_owned);
    }
    let at = line.find("https://")?;
    let url: &str = line[at..]
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'))
        .next()?;
    let url = url.trim_end_matches(['。', '，', '）', ')', ',', ';']);
    (url.len() > "https://".len() && url.len() <= 2000).then(|| url.to_owned())
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?.to_lowercase();
    (!host.is_empty()).then_some(host)
}

fn trusted(app: &str, url: &str) -> bool {
    let domains: &[&str] = match app {
        "lark" => &["feishu.cn", "larksuite.com", "larkoffice.com", "feishu.net"],
        "github" => &["github.com"],
        _ => &[],
    };
    url_host(url).is_some_and(|h| {
        domains
            .iter()
            .any(|d| h == *d || h.ends_with(&format!(".{d}")))
    })
}

fn find_code(line: &str) -> Option<String> {
    if !line.to_lowercase().contains("code") {
        return None;
    }
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find(|w| {
            let b = w.as_bytes();
            b.len() == 9
                && b[4] == b'-'
                && b.iter()
                    .enumerate()
                    .all(|(i, c)| i == 4 || c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        .map(str::to_owned)
}

pub(crate) fn redact(line: &str, url: Option<&str>) -> String {
    let low = line.to_lowercase();

    if low.contains("logged in as") || low.contains("logged in to") {
        return "✓ 已登录".into();
    }
    if low.contains("logged out of") {
        return "✓ 已退出登录".into();
    }
    let line = match url {
        Some(u) if line.contains(u) => line.replace(u, "〔链接见上方〕"),
        _ => line.to_owned(),
    };
    const IDS: &[&str] = &[
        "cli_",
        "ou_",
        "on_",
        "oc_",
        "gho_",
        "ghp_",
        "ghu_",
        "ghs_",
        "github_pat_",
    ];
    let mask = |w: &str| -> Option<String> {
        if let Some(p) = IDS
            .iter()
            .find(|p| w.starts_with(**p) && w.len() >= p.len() + 6)
        {
            return Some(format!("{p}…"));
        }

        if (w.starts_with("u-") || w.starts_with("t-")) && w.len() >= 20 {
            return Some(format!("{}…", &w[..2]));
        }
        let (local, domain) = w.split_once('@')?;
        (!local.is_empty() && domain.contains('.')).then(|| "…@…".to_owned())
    };
    let is_word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '@' | '.' | '+');
    let mut out = String::with_capacity(line.len());
    let mut word = String::new();
    for c in line.chars().chain(std::iter::once('\n')) {
        if is_word(c) {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            out.push_str(&mask(&word).unwrap_or_else(|| word.clone()));
            word.clear();
        }
        if c != '\n' {
            out.push(c);
        }
    }
    out
}

struct Spec {
    program: String,
    args: Vec<String>,

    dir: String,
    envs: Vec<(&'static str, &'static str)>,
    timeout: Duration,
}

impl Spec {
    fn command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.args)
            .env(
                "PATH",
                format!("{}:{}", self.dir, std::env::var("PATH").unwrap_or_default()),
            )
            .env("NO_COLOR", "1")
            .envs(self.envs.iter().copied())
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(unix)]
        cmd.process_group(0);
        cmd
    }
}

fn push_bytes(id: &str, stream: usize, bytes: &[u8], flush: bool) -> Option<String> {
    let mut g = jobs();
    let job = g.get_mut(id)?;
    let mut segs = job.terms[stream].feed(bytes);
    if flush {
        segs.extend(job.terms[stream].flush());
    }
    let mut found = None;
    for seg in segs {
        let line = tidy(&seg);
        if line.is_empty() {
            continue;
        }
        if job.want_url
            && job.view.url.is_none()
            && let Some(u) = find_url(&line)
        {
            job.view.url_trusted = trusted(job.view.app, &u);
            job.view.url = Some(u.clone());
            found = Some(u);
        }
        if job.want_code && job.view.code.is_none() {
            job.view.code = find_code(&line);
        }
        let shown = redact(&line, job.view.url.as_deref());
        let list = if job.forward {
            &mut job.view.lines
        } else {
            &mut job.hidden
        };
        match list.last_mut() {
            Some(last) if same_activity(last, &shown) => *last = shown,
            _ => {
                list.push(shown);
                if list.len() > MAX_LINES {
                    list.remove(0);
                }
            }
        }
    }
    found
}

async fn read_loop(id: String, stream: usize, mut r: impl AsyncRead + Unpin, qr: bool) {
    let mut buf = [0u8; 4096];
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if let Some(url) = push_bytes(&id, stream, &buf[..n], false)
            && qr
        {
            tokio::spawn(make_qr(id.clone(), url));
        }
    }
    if let Some(url) = push_bytes(&id, stream, &[], true)
        && qr
    {
        tokio::spawn(make_qr(id.clone(), url));
    }
}

async fn stop(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    if tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

async fn drive(id: &str, spec: &Spec, cancel: &mut oneshot::Receiver<()>, qr: bool) -> Outcome {
    let mut child = match spec.command().spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(format!("起不来：{e}")),
    };
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(tokio::spawn(read_loop(id.to_owned(), 0, out, qr)));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(tokio::spawn(read_loop(id.to_owned(), 1, err, qr)));
    }
    let outcome = tokio::select! {
        r = child.wait() => match r {
            Ok(s) => Outcome::Exit(s.code().unwrap_or(-1)),
            Err(e) => Outcome::Failed(e.to_string()),
        },
        _ = &mut *cancel => { stop(&mut child).await; Outcome::Cancelled }
        () = tokio::time::sleep(spec.timeout) => { stop(&mut child).await; Outcome::TimedOut }
    };

    for r in readers {
        let abort = r.abort_handle();
        if tokio::time::timeout(Duration::from_secs(2), r)
            .await
            .is_err()
        {
            abort.abort();
        }
    }
    outcome
}

async fn make_qr(id: String, url: String) {
    let Some(bin) = office::lark_bin().await else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("blazar-qr-{id}"));
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return;
    }
    let spec = Spec {
        program: bin.path,
        args: [
            "auth", "qrcode", &url, "--output", "qr.png", "--size", "240",
        ]
        .map(str::to_owned)
        .to_vec(),
        dir: bin.dir,
        envs: vec![],
        timeout: Duration::from_secs(15),
    };
    let mut cmd = spec.command();
    cmd.current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let ok = matches!(
        tokio::time::timeout(spec.timeout, cmd.status()).await,
        Ok(Ok(s)) if s.success()
    );
    let png = dir.join("qr.png");
    let small = ok
        && tokio::fs::metadata(&png)
            .await
            .is_ok_and(|m| m.len() <= QR_MAX);
    let bytes = if small {
        tokio::fs::read(&png).await.ok()
    } else {
        None
    };
    let _ = tokio::fs::remove_dir_all(&dir).await;
    if let Some(bytes) = bytes.filter(|b| b.starts_with(b"\x89PNG"))
        && let Some(job) = jobs().get_mut(&id)
        && job.view.status == "running"
    {
        job.view.qr = Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
}

async fn lark_login(
    id: &str,
    bin: &office::Bin,
    scope: Vec<String>,
    cancel: &mut oneshot::Receiver<()>,
) -> Outcome {
    let mut args = vec!["auth".to_owned(), "login".to_owned()];
    args.extend(scope);
    args.extend(["--no-wait".to_owned(), "--json".to_owned()]);
    let first = Spec {
        program: bin.path.clone(),
        args,
        dir: bin.dir.clone(),
        envs: vec![],
        timeout: Duration::from_secs(60),
    };
    let out = tokio::select! {
        r = tokio::time::timeout(first.timeout, first.command().output()) => r,
        _ = &mut *cancel => return Outcome::Cancelled,
    };
    let out = match out {
        Err(_) => return Outcome::TimedOut,
        Ok(Err(e)) => return Outcome::Failed(format!("起不来：{e}")),
        Ok(Ok(o)) => o,
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed = stdout
        .find('{')
        .and_then(|at| serde_json::from_str::<Value>(stdout[at..].trim()).ok());
    let found = parsed.as_ref().and_then(|v| {
        let url = deep_find(v, URL_KEYS)?;
        let code = deep_find(v, &["device_code"])?;
        Some((url.to_owned(), code.to_owned()))
    });
    let Some((url, device_code)) = found else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let text = if stderr.trim().is_empty() && !stdout.contains("device_code") {
            stdout.as_ref()
        } else {
            stderr.as_ref()
        };
        if let Some(job) = jobs().get_mut(id) {
            job.hidden = text
                .lines()
                .map(|l| redact(&tidy(l), None))
                .filter(|l| !l.is_empty())
                .collect();
        }
        return Outcome::Exit(out.status.code().filter(|c| *c != 0).unwrap_or(1));
    };
    if !url.starts_with("https://") {
        return Outcome::Failed("lark-cli 给的授权链接不是 https".into());
    }
    let expires = parsed
        .as_ref()
        .and_then(|v| deep_num(v, "expires_in"))
        .unwrap_or(600)
        .clamp(60, 1800);
    if let Some(job) = jobs().get_mut(id) {
        job.view.url_trusted = trusted("lark", &url);
        job.view.url = Some(url.clone());
    }
    tokio::spawn(make_qr(id.to_owned(), url));
    let second = Spec {
        program: bin.path.clone(),
        args: vec![
            "auth".into(),
            "login".into(),
            "--device-code".into(),
            device_code,
        ],
        dir: bin.dir.clone(),
        envs: vec![],
        timeout: Duration::from_secs(expires + 30),
    };
    drive(id, &second, cancel, false).await
}

fn deep_num(v: &Value, key: &str) -> Option<u64> {
    match v {
        Value::Object(m) => m
            .get(key)
            .and_then(Value::as_u64)
            .or_else(|| m.values().find_map(|c| deep_num(c, key))),
        _ => None,
    }
}

fn finish(id: &str, outcome: &Outcome) {
    let mut g = jobs();
    if let Some(job) = g.get_mut(id) {
        let step = job.view.step;
        let (status, message) = match outcome {
            Outcome::Exit(0) => (
                "done",
                match step {
                    "install" => "装好了",
                    "init" => "应用已创建，并绑定到这台机器的 lark-cli",
                    "login" => "已登录",
                    "logout" => "已退出登录",
                    _ => "完成",
                }
                .to_owned(),
            ),
            Outcome::Exit(code) => ("failed", format!("没成功（退出码 {code}）")),
            Outcome::Cancelled => ("cancelled", "已取消".to_owned()),
            Outcome::TimedOut => (
                "failed",
                "等太久了，已经停掉。链接有时效，重新点一次就行".to_owned(),
            ),
            Outcome::Failed(e) => ("failed", e.clone()),
        };
        job.view.status = status;
        job.view.message = message;
        job.view.exit_code = match outcome {
            Outcome::Exit(c) => Some(*c),
            _ => None,
        };
        if !job.forward && status == "failed" {
            let from = job.hidden.len().saturating_sub(8);
            job.view.lines = job.hidden.split_off(from);
        }
        job.hidden.clear();
        job.cancel = None;

        job.view.qr = None;
        job.view.code = None;
        tracing::info!(app = job.view.app, step, status, "办公软件连接任务结束");
    }
    let mut done: Vec<(u64, String)> = g
        .iter()
        .filter(|(_, j)| j.view.status != "running")
        .map(|(k, j)| (j.seq, k.clone()))
        .collect();
    done.sort_unstable();
    let drop_n = done.len().saturating_sub(KEEP_JOBS);
    for (_, k) in done.into_iter().take(drop_n) {
        g.remove(&k);
    }
}

#[derive(Deserialize)]
pub struct StartBody {
    pub step: String,

    #[serde(default)]
    pub brand: Option<String>,

    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub domains: Vec<String>,

    #[serde(default)]
    pub force: bool,
}

enum Plan {
    Run {
        spec: Spec,
        forward: bool,
        want_url: bool,
        want_code: bool,
    },
    LarkLogin {
        bin: office::Bin,
        scope: Vec<String>,
    },
}

fn sv(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn login_scope(mode: &str, domains: &[String]) -> Result<Vec<String>, String> {
    match mode {
        "recommend" => Ok(sv(&["--recommend"])),
        "all" => Ok(sv(&["--domain", "all"])),
        "domains" => {
            let ok = !domains.is_empty()
                && domains.len() <= 30
                && domains.iter().all(|d| {
                    (2..=20).contains(&d.len()) && d.chars().all(|c| c.is_ascii_lowercase())
                });
            if !ok {
                return Err("业务域不对：至少选一个，只能是小写字母".into());
            }
            Ok(vec!["--domain".into(), domains.join(",")])
        }
        _ => Err("scope 只能是 recommend / all / domains".into()),
    }
}

async fn plan(app: &str, b: &StartBody) -> Result<(&'static str, Plan), Response> {
    let bad = |m: &str| fail(StatusCode::BAD_REQUEST, m);
    let run = |step, spec, forward, want_url, want_code| {
        Ok((
            step,
            Plan::Run {
                spec,
                forward,
                want_url,
                want_code,
            },
        ))
    };
    let mins = |m: u64| Duration::from_secs(m * 60);
    match (app, b.step.as_str()) {
        ("lark", "install") => {
            let npx = office::find_bin("npx").await.ok_or_else(|| {
                bad("这台机器上没找到 npx。lark-cli 是个 npm 包，先装 Node.js（nodejs.org），再回来点安装")
            })?;
            let spec = Spec {
                program: npx.path,
                args: sv(&["-y", "@larksuite/cli@latest", "install"]),
                dir: npx.dir,
                envs: vec![],
                timeout: mins(10),
            };
            run("install", spec, true, false, false)
        }
        ("lark", step @ ("init" | "login" | "logout")) => {
            let bin = office::lark_bin()
                .await
                .ok_or_else(|| bad("还没装 lark-cli，先点「安装」"))?;
            match step {
                "init" => {
                    let brand = match b.brand.as_deref().unwrap_or("feishu") {
                        "feishu" => "feishu",
                        "lark" => "lark",
                        _ => return Err(bad("brand 只能是 feishu / lark")),
                    };
                    if !b.force && office::lark_configured().await == Some(true) {
                        return Err(fail(
                            StatusCode::CONFLICT,
                            "这台机器的 lark-cli 已经配置过应用了。重新创建会换掉现在的配置，登录也要重来",
                        ));
                    }
                    let spec = Spec {
                        program: bin.path,
                        args: sv(&["config", "init", "--new", "--brand", brand]),
                        dir: bin.dir,
                        envs: vec![],
                        timeout: mins(15),
                    };
                    run("init", spec, false, true, false)
                }
                "login" => {
                    let scope = login_scope(b.scope.as_deref().unwrap_or("recommend"), &b.domains)
                        .map_err(|e| bad(&e))?;
                    Ok(("login", Plan::LarkLogin { bin, scope }))
                }
                _ => {
                    let spec = Spec {
                        program: bin.path,
                        args: sv(&["auth", "logout", "--json"]),
                        dir: bin.dir,
                        envs: vec![],
                        timeout: mins(1),
                    };
                    run("logout", spec, false, false, false)
                }
            }
        }
        ("github", "install") => {
            let brew = office::find_bin("brew").await.ok_or_else(|| {
                bad("这台机器上没有 Homebrew。照 cli.github.com 上的说明装 gh，再回来登录")
            })?;
            let spec = Spec {
                program: brew.path,
                args: sv(&["install", "gh"]),
                dir: brew.dir,
                envs: vec![("HOMEBREW_NO_AUTO_UPDATE", "1"), ("NONINTERACTIVE", "1")],
                timeout: mins(15),
            };
            run("install", spec, true, false, false)
        }
        ("github", step @ ("login" | "logout")) => {
            let gh = office::find_bin("gh")
                .await
                .ok_or_else(|| bad("还没装 gh，先点「安装」"))?;
            let login = step == "login";
            let spec = Spec {
                program: gh.path,
                args: if login {
                    sv(&[
                        "auth",
                        "login",
                        "--hostname",
                        "github.com",
                        "--git-protocol",
                        "https",
                        "--web",
                    ])
                } else {
                    sv(&["auth", "logout", "--hostname", "github.com"])
                },
                dir: gh.dir,
                envs: vec![("GH_PROMPT_DISABLED", "1")],
                timeout: if login { mins(15) } else { mins(1) },
            };
            run(
                if login { "login" } else { "logout" },
                spec,
                false,
                login,
                login,
            )
        }
        ("lark" | "github", _) => Err(bad("没有这一步")),
        _ => Err(fail(StatusCode::NOT_FOUND, "没有这个软件")),
    }
}

fn app_key(app: &str) -> Option<&'static str> {
    match app {
        "lark" => Some("lark"),
        "github" => Some("github"),
        _ => None,
    }
}

pub async fn start(Path(app): Path<String>, Json(b): Json<StartBody>) -> Response {
    let Some(app) = app_key(&app) else {
        return fail(StatusCode::NOT_FOUND, "没有这个软件");
    };
    if let Some(j) = latest(app).filter(|j| j.status == "running") {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "上一步还在进行中，先等它完成或者取消", "job": j })),
        )
            .into_response();
    }
    let (step, plan) = match plan(app, &b).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let id = uuid::Uuid::now_v7().to_string();
    let (tx, mut rx) = oneshot::channel();
    let (forward, want_url, want_code) = match &plan {
        Plan::Run {
            forward,
            want_url,
            want_code,
            ..
        } => (*forward, *want_url, *want_code),
        Plan::LarkLogin { .. } => (false, false, false),
    };
    let view = JobView {
        id: id.clone(),
        app,
        step,
        status: "running",
        message: String::new(),
        lines: vec![],
        url: None,
        url_trusted: false,
        qr: None,
        code: None,
        exit_code: None,
        started_at: Utc::now().to_rfc3339(),
    };
    {
        let mut g = jobs();

        if g.values()
            .any(|j| j.view.app == app && j.view.status == "running")
        {
            return fail(StatusCode::CONFLICT, "上一步还在进行中");
        }
        let seq = g.values().map(|j| j.seq).max().unwrap_or(0) + 1;
        g.insert(
            id.clone(),
            Job {
                seq,
                view: view.clone(),
                cancel: Some(tx),
                terms: [Term::default(), Term::default()],
                forward,
                want_url,
                want_code,
                hidden: vec![],
            },
        );
    }
    tracing::info!(app, step, "办公软件连接任务开始");
    tokio::spawn(async move {
        let outcome = match plan {
            Plan::Run { spec, want_url, .. } => {
                drive(&id, &spec, &mut rx, want_url && app == "lark").await
            }
            Plan::LarkLogin { bin, scope } => lark_login(&id, &bin, scope, &mut rx).await,
        };
        finish(&id, &outcome);
    });
    Json(view).into_response()
}

pub async fn job(Path(app): Path<String>) -> Response {
    Json(latest(&app)).into_response()
}

pub async fn cancel(Path(app): Path<String>) -> Response {
    let mut g = jobs();
    let running = g
        .values_mut()
        .find(|j| j.view.app == app && j.view.status == "running");
    match running.and_then(|j| j.cancel.take()) {
        Some(tx) => {
            let _ = tx.send(());
            Json(json!({ "ok": true })).into_response()
        }
        None => fail(StatusCode::NOT_FOUND, "没有在进行中的步骤"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(chunks: &[&[u8]]) -> Vec<String> {
        let mut t = Term::default();
        let mut out: Vec<String> = Vec::new();
        let mut push = |seg: String| {
            let l = tidy(&seg);
            if l.is_empty() {
                return;
            }
            match out.last_mut() {
                Some(last) if same_activity(last, &l) => *last = l,
                _ => out.push(l),
            }
        };
        for c in chunks {
            t.feed(c).into_iter().for_each(&mut push);
        }
        t.flush().into_iter().for_each(&mut push);
        out
    }

    #[test]
    fn spinner_frames_collapse_into_one_line() {
        let raw = "Setting up Feishu/Lark CLI...\n\x1b[?25l│\n◒  Installing skills\x1b[1G\x1b[J◐  Installing skills\x1b[1G\x1b[J◓  Installing skills.\x1b[1G\x1b[J◑  Installing skills..\x1b[1G\x1b[J◇  Skills installed\n\x1b[?25hTo complete setup, run interactively:\n";
        assert_eq!(
            lines_of(&[raw.as_bytes()]),
            [
                "Setting up Feishu/Lark CLI...",
                "Installing skills..",
                "◇  Skills installed",
                "To complete setup, run interactively:"
            ]
        );
    }

    #[test]
    fn multibyte_chars_and_escapes_split_across_chunks_survive() {
        let raw = "◒  装技能\x1b[1G\x1b[J◇  好了\n".as_bytes();
        for cut in 1..raw.len() {
            let got = lines_of(&[&raw[..cut], &raw[cut..]]);
            assert_eq!(got, ["装技能", "◇  好了"], "切在 {cut}");
        }

        assert_eq!(
            lines_of(&[b"ok\n\xff\xfe\nnext\n"]),
            ["ok", "\u{fffd}\u{fffd}", "next"]
        );
    }

    #[test]
    fn finds_the_link_in_json_and_in_plain_text() {
        assert_eq!(
            find_url(r#"{"ok":true,"data":{"verification_url":"https://open.feishu.cn/v?c=1&d=2","device_code":"x"}}"#).as_deref(),
            Some("https://open.feishu.cn/v?c=1&d=2")
        );

        assert_eq!(
            find_url(r#"{"verification_url":"https://a.feishu.cn/1","verification_uri_complete":"https://a.feishu.cn/2"}"#).as_deref(),
            Some("https://a.feishu.cn/2")
        );
        assert_eq!(
            find_url("请在浏览器打开：https://open.feishu.cn/app/new?x=1。").as_deref(),
            Some("https://open.feishu.cn/app/new?x=1")
        );
        assert_eq!(
            find_url(
                "Open this URL to continue in your web browser: https://github.com/login/device"
            )
            .as_deref(),
            Some("https://github.com/login/device")
        );
        assert_eq!(find_url("http://insecure.example/ 不算"), None);
        assert_eq!(
            find_url(r#"{"verification_url":"javascript:alert(1)"}"#),
            None
        );
        assert_eq!(find_url("https://"), None);
    }

    #[test]
    fn only_the_apps_own_domains_are_trusted() {
        assert!(trusted("lark", "https://open.feishu.cn/x"));
        assert!(trusted("lark", "https://accounts.larksuite.com:443/x?y"));
        assert!(trusted("github", "https://github.com/login/device"));
        assert!(!trusted("lark", "https://feishu.cn.evil.example/x"));
        assert!(!trusted("lark", "https://open.feishu.cn@evil.example/x"));
        assert!(!trusted("lark", "https://evilfeishu.cn/x"));
        assert!(!trusted(
            "github",
            "https://github.com.evil.example/login/device"
        ));
        assert!(!trusted("other", "https://github.com/"));
    }

    #[test]
    fn finds_the_github_one_time_code() {
        assert_eq!(
            find_code("! First copy your one-time code: AB12-CD34").as_deref(),
            Some("AB12-CD34")
        );
        assert_eq!(find_code("one-time code: ab12-cd34"), None);
        assert_eq!(find_code("nothing here ABCD-1234"), None);
    }

    #[test]
    fn redaction_hides_ids_tokens_emails_and_account_lines() {
        assert_eq!(
            redact("App ID: cli_a1b2c3d4e5f6 (owner ou_9f8e7d6c5b4a)", None),
            "App ID: cli_… (owner ou_…)"
        );
        assert_eq!(
            redact("app_id=cli_a1b2c3d4e5f6&x=1", None),
            "app_id=cli_…&x=1"
        );
        assert_eq!(redact("已登录：someone@example.com", None), "已登录：…@…");
        assert_eq!(redact("✓ Logged in as octocat", None), "✓ 已登录");
        assert_eq!(
            redact("✓ Logged out of github.com account octocat", None),
            "✓ 已退出登录"
        );
        assert_eq!(
            redact("token u-abcdefghijklmnopqrstuvwxyz ok", None),
            "token u-… ok"
        );
        assert_eq!(redact("gho_abcdefghijklmnop", None), "gho_…");

        assert_eq!(
            redact("run cli_ or u-turn, on_ time", None),
            "run cli_ or u-turn, on_ time"
        );

        assert_eq!(
            redact(
                "打开 https://open.feishu.cn/v?app=cli_a1b2c3d4e5f6 完成授权",
                Some("https://open.feishu.cn/v?app=cli_a1b2c3d4e5f6")
            ),
            "打开 〔链接见上方〕 完成授权"
        );
    }

    #[test]
    fn login_scope_only_takes_enumerated_shapes() {
        assert_eq!(login_scope("recommend", &[]).unwrap(), ["--recommend"]);
        assert_eq!(login_scope("all", &[]).unwrap(), ["--domain", "all"]);
        assert_eq!(
            login_scope("domains", &sv(&["docs", "im"])).unwrap(),
            ["--domain", "docs,im"]
        );
        assert!(login_scope("domains", &[]).is_err());
        assert!(login_scope("domains", &sv(&["docs,--yes"])).is_err());
        assert!(login_scope("domains", &sv(&["--device-code"])).is_err());
        assert!(login_scope("domains", &sv(&["Docs"])).is_err());
        assert!(login_scope("everything", &[]).is_err());
    }

    fn register(id: &str, forward: bool, want_url: bool) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        jobs().insert(
            id.to_owned(),
            Job {
                seq: 0,
                view: JobView {
                    id: id.to_owned(),
                    app: "lark",
                    step: "init",
                    status: "running",
                    message: String::new(),
                    lines: vec![],
                    url: None,
                    url_trusted: false,
                    qr: None,
                    code: None,
                    exit_code: None,
                    started_at: String::new(),
                },
                cancel: Some(tx),
                terms: [Term::default(), Term::default()],
                forward,
                want_url,
                want_code: false,
                hidden: vec![],
            },
        );
        rx
    }

    fn sh(script: &str, secs: u64) -> Spec {
        Spec {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            dir: "/usr/bin".into(),
            envs: vec![],
            timeout: Duration::from_secs(secs),
        }
    }

    #[tokio::test]
    async fn a_quiet_step_keeps_its_output_to_itself_unless_it_fails() {
        let id = "t-quiet-ok";
        let mut rx = register(id, false, true);
        let spec = sh(
            "echo 'open https://open.feishu.cn/new?x=1'; echo '创建成功：张三 cli_a1b2c3d4e5f6'",
            10,
        );
        let out = drive(id, &spec, &mut rx, false).await;
        finish(id, &out);
        let v = latest_by_id(id);
        assert_eq!(v.status, "done");
        assert_eq!(v.url.as_deref(), Some("https://open.feishu.cn/new?x=1"));
        assert!(v.url_trusted);
        assert!(v.lines.is_empty(), "{:?}", v.lines);

        let id = "t-quiet-bad";
        let mut rx = register(id, false, true);
        let spec = sh("echo 'app cli_a1b2c3d4e5f6 is disabled' >&2; exit 3", 10);
        let out = drive(id, &spec, &mut rx, false).await;
        finish(id, &out);
        let v = latest_by_id(id);
        assert_eq!((v.status, v.exit_code), ("failed", Some(3)));
        assert_eq!(v.lines, ["app cli_… is disabled"]);
    }

    fn latest_by_id(id: &str) -> JobView {
        jobs().get(id).map(|j| j.view.clone()).expect("job")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_stops_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let id = "t-cancel";
        let mut rx = register(id, true, false);
        let tx = jobs().get_mut(id).and_then(|j| j.cancel.take()).unwrap();

        let spec = sh(
            &format!("(echo $$ > '{}'; exec sleep 60) & wait", pidfile.display()),
            60,
        );
        let t = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            let _ = tx.send(());
        });
        let started = std::time::Instant::now();
        let out = drive(id, &spec, &mut rx, false).await;
        t.await.unwrap();
        finish(id, &out);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(latest_by_id(id).status, "cancelled");
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "孙进程还活着");
    }

    #[tokio::test]
    async fn a_step_that_overruns_is_stopped() {
        let id = "t-timeout";
        let mut rx = register(id, true, false);
        let out = drive(id, &sh("sleep 30", 1), &mut rx, false).await;
        finish(id, &out);
        let v = latest_by_id(id);
        assert_eq!(v.status, "failed");
        assert!(v.message.contains("等太久"));
    }
}
