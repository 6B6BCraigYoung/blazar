pub mod connect;
pub mod obsidian;

use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use tokio::sync::Mutex;

use crate::api::Shared;

pub static HUB_URL: OnceLock<String> = OnceLock::new();

pub const MCP_SUBCOMMAND: &str = "__mcp-office";

const MAX_OUT: usize = 200 * 1024;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

pub(crate) async fn kv_get(st: &Shared, key: &str) -> Value {
    sqlx::query_scalar::<_, String>("SELECT value FROM kv_settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or(Value::Null)
}

pub(crate) async fn kv_put(st: &Shared, key: &str, v: &Value) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO kv_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(v.to_string())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub async fn prefs(st: &Shared) -> Value {
    let saved = kv_get(st, "prefs").await;
    json!({
        "git": {
            "branch_prefix": saved["git"]["branch_prefix"].as_str().unwrap_or("blazar/"),
            "pr_draft": saved["git"]["pr_draft"].as_bool().unwrap_or(false),
            "ai_draft": saved["git"]["ai_draft"].as_bool().unwrap_or(true),
        },
        "inbox": {
            "muted": saved["inbox"]["muted"].as_array().cloned().unwrap_or_default(),
        },
    })
}

fn valid_branch_prefix(p: &str) -> bool {
    p.len() <= 40
        && !p.starts_with(['-', '/', '.'])
        && !p.contains("..")
        && !p.contains("//")
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
}

pub async fn get_prefs(State(st): State<Shared>) -> Response {
    Json(prefs(&st).await).into_response()
}

pub async fn put_prefs(State(st): State<Shared>, Json(b): Json<Value>) -> Response {
    let mut cur = prefs(&st).await;
    if let Some(p) = b["git"]["branch_prefix"].as_str() {
        let p = p.trim();
        if !valid_branch_prefix(p) {
            return fail(
                StatusCode::BAD_REQUEST,
                "分支前缀只能用字母、数字、- _ / .，且不能以 - / . 开头",
            );
        }
        cur["git"]["branch_prefix"] = json!(p);
    }
    for k in ["pr_draft", "ai_draft"] {
        if let Some(v) = b["git"][k].as_bool() {
            cur["git"][k] = json!(v);
        }
    }
    if let Some(m) = b["inbox"]["muted"].as_array() {
        let kinds: Vec<&str> = m
            .iter()
            .filter_map(Value::as_str)
            .filter(|k| crate::inbox::KINDS.contains(k))
            .collect();
        cur["inbox"]["muted"] = json!(kinds);
    }
    match kv_put(&st, "prefs", &cur).await {
        Ok(()) => Json(cur).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn about(State(st): State<Shared>) -> Response {
    let n = |sql: &'static str| {
        let pool = st.db.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(&pool)
                .await
                .unwrap_or(0)
        }
    };
    let db_bytes =
        n("SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()").await;
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "hub_url": HUB_URL.get(),

        "engine": {
            "name": "EasyTier",
            "version": blazar_netmesh::engine::ENGINE_VERSION,
            "license": "LGPL-3.0",
            "source": format!("https://github.com/EasyTier/EasyTier/tree/v{}", blazar_netmesh::engine::ENGINE_VERSION),
        },
        "db_bytes": db_bytes,
        "counts": {
            "workspaces": n("SELECT COUNT(*) FROM workspaces").await,
            "sessions": n("SELECT COUNT(*) FROM sessions").await,
            "events": n("SELECT COUNT(*) FROM events").await,
            "tasks": n("SELECT COUNT(*) FROM tasks").await,
            "skills": n("SELECT COUNT(*) FROM skills").await,
            "inbox": n("SELECT COUNT(*) FROM inbox").await,
        },
    }))
    .into_response()
}

#[derive(Clone)]
pub(crate) struct Bin {
    pub path: String,

    pub dir: String,
}

const BIN_DIR_ENV: &str = "BLAZAR_OFFICE_BIN_DIR";

pub(crate) async fn find_bin(name: &str) -> Option<Bin> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    if let Some(dir) = std::env::var_os(BIN_DIR_ENV) {
        let dir = std::path::PathBuf::from(dir);
        let path = dir.join(name);
        return path.is_file().then(|| Bin {
            path: path.display().to_string(),
            dir: dir.display().to_string(),
        });
    }
    let out = tokio::process::Command::new("bash")
        .args(["-lc", &format!("command -v {name}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if path.is_empty() || !std::path::Path::new(&path).is_file() {
        return None;
    }
    let dir = std::path::Path::new(&path).parent()?.display().to_string();
    Some(Bin { path, dir })
}

pub(crate) async fn lark_bin() -> Option<Bin> {
    static BIN: OnceLock<Mutex<Option<Bin>>> = OnceLock::new();
    if std::env::var_os(BIN_DIR_ENV).is_some() {
        return find_bin("lark-cli").await;
    }
    let mut g = BIN.get_or_init(|| Mutex::new(None)).lock().await;
    if !g
        .as_ref()
        .is_some_and(|b| std::path::Path::new(&b.path).is_file())
    {
        *g = find_bin("lark-cli").await;
    }
    g.clone()
}

pub(crate) async fn lark_configured() -> Option<bool> {
    let bin = lark_bin().await?;
    let status = tokio::process::Command::new(&bin.path)
        .args(["config", "show"])
        .env(
            "PATH",
            format!("{}:{}", bin.dir, std::env::var("PATH").unwrap_or_default()),
        )
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    tokio::time::timeout(Duration::from_secs(10), status)
        .await
        .ok()?
        .ok()
        .map(|s| s.success())
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn lark(args: &[String], stdin: Option<String>, secs: u64) -> Result<Ran, String> {
    let bin = lark_bin().await.ok_or("这台机器上没找到 lark-cli")?;
    let path = format!("{}:{}", bin.dir, std::env::var("PATH").unwrap_or_default());
    let mut cmd = tokio::process::Command::new(&bin.path);
    cmd.args(args)
        .env("PATH", path)
        .current_dir(std::env::temp_dir())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("起不了 lark-cli：{e}"))?;
    if let (Some(text), Some(mut w)) = (stdin, child.stdin.take()) {
        use tokio::io::AsyncWriteExt;
        let _ = w.write_all(text.as_bytes()).await;
        let _ = w.shutdown().await;
    }
    let out = tokio::time::timeout(Duration::from_secs(secs), child.wait_with_output())
        .await
        .map_err(|_| format!("lark-cli {secs} 秒没跑完，放弃了"))?
        .map_err(|e| e.to_string())?;
    let cut = |b: &[u8]| {
        String::from_utf8_lossy(b)
            .chars()
            .take(MAX_OUT)
            .collect::<String>()
    };
    Ok(Ran {
        code: out.status.code().unwrap_or(-1),
        stdout: cut(&out.stdout),
        stderr: cut(&out.stderr),
    })
}

fn sv(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn redact_status(v: &Value) -> Value {
    let ident = |k: &str| {
        let i = &v["identities"][k];
        json!({ "available": i["available"].as_bool().unwrap_or(false), "status": i["status"].as_str().unwrap_or("") ,
                "expires_at": i["expiresAt"], "message": i["message"].as_str().unwrap_or("") })
    };
    json!({ "brand": v["brand"].as_str().unwrap_or(""), "default_as": v["defaultAs"].as_str().unwrap_or(""),
            "user": ident("user"), "bot": ident("bot") })
}

pub async fn lark_settings(st: &Shared) -> Value {
    let s = kv_get(st, "office.lark").await;
    json!({
        "agent_access": s["agent_access"].as_str().filter(|a| matches!(*a, "read" | "write")).unwrap_or("off"),
        "notify": {
            "enabled": s["notify"]["enabled"].as_bool().unwrap_or(false),
            "target": s["notify"]["target"].as_str().unwrap_or(""),
            "as": s["notify"]["as"].as_str().filter(|a| matches!(*a, "user" | "bot")).unwrap_or(""),
            "kinds": s["notify"]["kinds"].as_array().cloned()
                .unwrap_or_else(|| vec![json!("run_failed"), json!("approval"), json!("question"), json!("autopilot_paused")]),
        },
    })
}

fn valid_target(t: &str) -> bool {
    (t.starts_with("oc_") || t.starts_with("ou_"))
        && t.len() > 3
        && t.len() <= 80
        && t[3..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub async fn lark_status(State(st): State<Shared>) -> Response {
    let settings = lark_settings(&st).await;
    let Some(bin) = lark_bin().await else {
        return Json(json!({ "installed": false, "settings": settings,
            "can_install": find_bin("npx").await.is_some(),
            "install": "npx @larksuite/cli@latest install", "login": "lark-cli config init --new && lark-cli auth login --recommend" }))
        .into_response();
    };
    let version = lark(&sv(&["--version"]), None, 10)
        .await
        .map(|r| r.stdout.trim().replace("lark-cli version ", ""))
        .unwrap_or_default();
    let status = lark(&sv(&["auth", "status"]), None, 15)
        .await
        .ok()
        .and_then(|r| serde_json::from_str::<Value>(&r.stdout).ok())
        .map_or(Value::Null, |v| redact_status(&v));

    let says_unconfigured = ["user", "bot"].iter().any(|k| {
        status[k]["message"]
            .as_str()
            .is_some_and(|m| m.contains("not configured"))
    });
    let configured = lark_configured().await.unwrap_or(false) && !says_unconfigured;

    let local = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|h| h.join(".claude/skills"))
        .and_then(|d| std::fs::read_dir(d).ok())
        .map_or(0, |rd| {
            rd.filter_map(Result::ok)
                .filter(|e| {
                    e.file_name().to_string_lossy().starts_with("lark-")
                        && e.path().join("SKILL.md").is_file()
                })
                .count()
        });
    let imported: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE name LIKE 'lark-%'")
        .fetch_one(st.db.pool())
        .await
        .unwrap_or(0);
    Json(json!({ "installed": true, "version": version, "path_dir": bin.dir, "auth": status, "settings": settings,
                 "configured": configured,
                 "skills_local": local, "skills_imported": imported,
                 "init": "lark-cli config init", "login": "lark-cli auth login --recommend" }))
    .into_response()
}

pub async fn put_lark_settings(State(st): State<Shared>, Json(b): Json<Value>) -> Response {
    let mut cur = lark_settings(&st).await;
    if let Some(a) = b["agent_access"].as_str() {
        if !matches!(a, "off" | "read" | "write") {
            return fail(
                StatusCode::BAD_REQUEST,
                "agent_access 只能是 off / read / write",
            );
        }
        cur["agent_access"] = json!(a);
    }
    let n = &b["notify"];
    if let Some(v) = n["enabled"].as_bool() {
        cur["notify"]["enabled"] = json!(v);
    }
    if let Some(t) = n["target"].as_str() {
        let t = t.trim();
        if !t.is_empty() && !valid_target(t) {
            return fail(
                StatusCode::BAD_REQUEST,
                "发给谁：填群的 chat_id（oc_ 开头）或个人的 open_id（ou_ 开头）",
            );
        }
        cur["notify"]["target"] = json!(t);
    }
    if let Some(a) = n["as"].as_str() {
        cur["notify"]["as"] = json!(if matches!(a, "user" | "bot") { a } else { "" });
    }
    if let Some(k) = n["kinds"].as_array() {
        cur["notify"]["kinds"] = json!(
            k.iter()
                .filter_map(Value::as_str)
                .filter(|k| crate::inbox::KINDS.contains(k))
                .collect::<Vec<_>>()
        );
    }
    if cur["notify"]["enabled"] == json!(true)
        && cur["notify"]["target"].as_str().unwrap_or("").is_empty()
    {
        return fail(StatusCode::BAD_REQUEST, "先填发给谁，再打开转发");
    }
    match kv_put(&st, "office.lark", &cur).await {
        Ok(()) => Json(cur).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

fn command_path(args: &[String]) -> Vec<String> {
    args.iter()
        .take_while(|a| !a.starts_with('-'))
        .cloned()
        .collect()
}

fn static_risk(args: &[String]) -> Result<Option<&'static str>, String> {
    if args.is_empty()
        || args.len() > 60
        || args.iter().any(|a| a.len() > 20_000 || a.contains('\0'))
    {
        return Err("参数不对".into());
    }
    if args.iter().any(|a| matches!(a.as_str(), "--yes" | "-y")) {
        return Err("带 --yes 的高风险操作不能由 agent 代劳：需要的话请你自己在终端里执行".into());
    }

    let help = args.iter().any(|a| matches!(a.as_str(), "--help" | "-h"));
    match args[0].as_str() {
        "auth" | "config" | "profile" | "install" | "update" | "upgrade" | "completion"
        | "uninstall" => Err(format!(
            "`lark-cli {}` 管的是登录与配置，不给 agent 用",
            args[0]
        )),

        "event" => Err("事件订阅是长驻进程，这里不支持".into()),
        "schema" | "skills" | "help" | "--help" | "--version" => Ok(Some("read")),
        _ if help => Ok(Some("read")),
        "api" => Ok(Some(
            if args.get(1).is_some_and(|m| m.eq_ignore_ascii_case("GET")) {
                "read"
            } else {
                "write"
            },
        )),
        _ => Ok(None),
    }
}

fn risk_from_help(help: &str) -> Option<&'static str> {
    let line = help
        .lines()
        .find_map(|l| l.trim().strip_prefix("Risk:"))?
        .trim()
        .to_lowercase();
    Some(if line.starts_with("read") {
        "read"
    } else if line.starts_with("high") {
        "high-risk-write"
    } else {
        "write"
    })
}

#[derive(Deserialize)]
pub struct RunBody {
    pub args: Vec<String>,
}

async fn log_call(st: &Shared, args: &[String], risk: &str, verdict: &str, exit: Option<i32>) {
    let command: String = command_path(args).join(" ").chars().take(120).collect();
    let _ = sqlx::query(
        "INSERT INTO office_calls (id, app, command, risk, verdict, exit_code, created_at) VALUES (?1, 'lark', ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(if command.is_empty() { "（空）".to_owned() } else { command })
    .bind(risk)
    .bind(verdict)
    .bind(exit)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
}

pub async fn lark_run(State(st): State<Shared>, Json(b): Json<RunBody>) -> Response {
    let access = lark_settings(&st).await["agent_access"]
        .as_str()
        .unwrap_or("off")
        .to_owned();
    if access == "off" {
        return fail(
            StatusCode::FORBIDDEN,
            "「让智能体用飞书」没有打开（侧栏「办公」→ 飞书）",
        );
    }
    let risk = match static_risk(&b.args) {
        Err(e) => {
            log_call(&st, &b.args, "denied", "refused", None).await;
            return fail(StatusCode::FORBIDDEN, e);
        }
        Ok(Some(r)) => r,
        Ok(None) => {
            let mut probe = command_path(&b.args);
            probe.push("--help".into());
            match lark(&probe, None, 15).await {
                Ok(r) => risk_from_help(&format!("{}{}", r.stdout, r.stderr)).unwrap_or("write"),
                Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
            }
        }
    };
    let dry = b.args.iter().any(|a| a == "--dry-run");
    if risk == "high-risk-write" && !dry {
        log_call(&st, &b.args, risk, "refused", None).await;
        return fail(
            StatusCode::FORBIDDEN,
            "这是高风险写操作（删除、移除成员、批量变更这一类）：不能由 agent 代劳，请你自己在终端里执行",
        );
    }
    if risk == "write" && access != "write" && !dry {
        log_call(&st, &b.args, risk, "refused", None).await;
        return fail(
            StatusCode::FORBIDDEN,
            "现在是「只读」：这条命令会改动飞书里的内容。加 --dry-run 可以先看它会发什么请求；要真执行，到「办公 → 飞书」里改成「读写」",
        );
    }
    match lark(&b.args, None, 90).await {
        Ok(r) => {
            log_call(
                &st,
                &b.args,
                risk,
                if dry { "dry_run" } else { "allowed" },
                Some(r.code),
            )
            .await;
            Json(json!({ "exit_code": r.code, "risk": risk, "stdout": r.stdout, "stderr": r.stderr }))
                .into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

pub async fn lark_calls(State(st): State<Shared>) -> Response {
    let rows = sqlx::query(
        "SELECT command, risk, verdict, exit_code, created_at FROM office_calls WHERE app = 'lark' ORDER BY created_at DESC LIMIT 60",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    Json(
        rows.iter()
            .map(|r| {
                json!({ "command": r.try_get::<String, _>("command").unwrap_or_default(),
                        "risk": r.try_get::<String, _>("risk").unwrap_or_default(),
                        "verdict": r.try_get::<String, _>("verdict").unwrap_or_default(),
                        "exit_code": r.try_get::<Option<i64>, _>("exit_code").ok().flatten(),
                        "created_at": r.try_get::<String, _>("created_at").unwrap_or_default() })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

pub async fn github_status() -> Response {
    let Some(gh) = find_bin("gh").await else {
        return Json(json!({ "installed": false, "authed": false, "version": "",
            "can_install": find_bin("brew").await.is_some(),
            "install": "brew install gh", "login": "gh auth login" }))
        .into_response();
    };
    let run = |args: &'static [&'static str], keep_stdout: bool| {
        let mut cmd = tokio::process::Command::new(&gh.path);
        cmd.args(args)
            .env(
                "PATH",
                format!("{}:{}", gh.dir, std::env::var("PATH").unwrap_or_default()),
            )
            .stdin(Stdio::null())
            .stdout(if keep_stdout {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::null())
            .kill_on_drop(true);
        async move {
            tokio::time::timeout(Duration::from_secs(15), cmd.output())
                .await
                .ok()?
                .ok()
        }
    };
    let version = run(&["--version"], true)
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let authed = run(&["auth", "status"], false)
        .await
        .is_some_and(|o| o.status.success());
    Json(json!({
        "installed": true,
        "version": version.trim_start_matches("gh version ").split_whitespace().next().unwrap_or(""),
        "authed": authed,
        "install": "brew install gh", "login": "gh auth login",
    }))
    .into_response()
}

const KIND_TEXT: &[(&str, &str)] = &[
    ("run_done", "✅ 跑完了"),
    ("run_failed", "❌ 没跑成"),
    ("approval", "⚠️ 等你裁决"),
    ("question", "💬 在问你"),
    ("autopilot_paused", "⏸ 自动化暂停"),
    ("rate_limit", "📉 额度告警"),
];

fn notify_args(target: &str, as_: &str, markdown: &str, key: &str, dry: bool) -> Vec<String> {
    let mut a = sv(&["im", "+messages-send"]);
    a.push(if target.starts_with("ou_") {
        "--user-id".into()
    } else {
        "--chat-id".into()
    });
    a.push(target.to_owned());
    a.extend([
        "--markdown".to_owned(),
        markdown.to_owned(),
        "--idempotency-key".to_owned(),
        key.chars().take(50).collect(),
    ]);
    if matches!(as_, "user" | "bot") {
        a.extend(["--as".to_owned(), as_.to_owned()]);
    }
    if dry {
        a.push("--dry-run".into());
    }
    a
}

pub async fn forward(st: Shared, id: String, kind: &'static str, title: String, body: String) {
    let s = lark_settings(&st).await;
    let n = &s["notify"];
    let target = n["target"].as_str().unwrap_or("");
    if n["enabled"] != json!(true)
        || !valid_target(target)
        || !n["kinds"]
            .as_array()
            .is_some_and(|k| k.iter().any(|x| x == kind))
    {
        return;
    }
    let head = KIND_TEXT
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(kind, |(_, t)| t);
    let text = format!(
        "**{head}**　{title}\n{}",
        body.chars().take(400).collect::<String>()
    );
    match lark(
        &notify_args(
            target,
            n["as"].as_str().unwrap_or(""),
            text.trim(),
            &format!("blz-{id}"),
            false,
        ),
        None,
        30,
    )
    .await
    {
        Ok(r) if r.code == 0 => {}
        Ok(r) => {
            tracing::warn!(target: "blazar::office", "转发到飞书失败（{}）：{}", r.code, r.stderr.chars().take(200).collect::<String>())
        }
        Err(e) => tracing::warn!(target: "blazar::office", "转发到飞书失败：{e}"),
    }
}

#[derive(Deserialize, Default)]
pub struct TestBody {
    #[serde(default)]
    pub dry_run: bool,
}

pub async fn test_notify(State(st): State<Shared>, body: Option<Json<TestBody>>) -> Response {
    let dry = body.is_some_and(|b| b.dry_run);
    let s = lark_settings(&st).await;
    let target = s["notify"]["target"].as_str().unwrap_or("").to_owned();
    if !valid_target(&target) {
        return fail(StatusCode::BAD_REQUEST, "先填发给谁");
    }
    let key = format!("blz-test-{}", Utc::now().timestamp());
    let args = notify_args(
        &target,
        s["notify"]["as"].as_str().unwrap_or(""),
        "**Blazar** 测试消息：提醒转发已经接通。",
        &key,
        dry,
    );
    match lark(&args, None, 30).await {
        Ok(r) => Json(json!({ "ok": r.code == 0, "dry_run": dry,
            "detail": if r.code == 0 && !dry { String::new() } else { format!("{}{}", r.stdout, r.stderr).chars().take(1200).collect() } })).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

pub async fn task_doc(
    State(st): State<Shared>,
    Path(id): Path<String>,
    body: Option<Json<TestBody>>,
) -> Response {
    let dry = body.is_some_and(|b| b.dry_run);
    let Ok(Some(t)) =
        sqlx::query("SELECT number, title, description, status FROM tasks WHERE id = ?1")
            .bind(&id)
            .fetch_optional(st.db.pool())
            .await
    else {
        return fail(StatusCode::NOT_FOUND, "没有这个任务");
    };
    let title: String = t.try_get("title").unwrap_or_default();
    let number: i64 = t.try_get("number").unwrap_or(0);
    let comments = sqlx::query(
        "SELECT author, body, created_at FROM task_comments WHERE task_id = ?1 ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut md = format!(
        "> BLZ-{number} · {}\n\n{}\n",
        t.try_get::<String, _>("status").unwrap_or_default(),
        t.try_get::<String, _>("description").unwrap_or_default()
    );
    for c in &comments {
        let who = match c
            .try_get::<String, _>("author")
            .unwrap_or_default()
            .as_str()
        {
            "agent" => "智能体",
            "user" => "我",
            _ => "系统",
        };
        md.push_str(&format!(
            "\n## {who} · {}\n\n{}\n",
            c.try_get::<String, _>("created_at")
                .unwrap_or_default()
                .chars()
                .take(16)
                .collect::<String>()
                .replace('T', " "),
            c.try_get::<String, _>("body").unwrap_or_default()
        ));
    }
    let mut args = sv(&["docs", "+create", "--doc-format", "markdown", "--title"]);
    args.push(format!("BLZ-{number} {title}"));
    args.extend(sv(&["--content", "-"]));
    if dry {
        args.push("--dry-run".into());
    }
    match lark(&args, Some(md), 60).await {
        Ok(r) => {
            let v: Value = serde_json::from_str(&r.stdout).unwrap_or(Value::Null);
            let url = ["/data/url", "/data/document/url", "/data/doc_url", "/url"]
                .iter()
                .find_map(|p| v.pointer(p).and_then(Value::as_str));
            Json(json!({ "ok": r.code == 0, "dry_run": dry, "url": url,
                "detail": if r.code == 0 && url.is_some() { String::new() } else { format!("{}{}", r.stdout, r.stderr).chars().take(1200).collect() } })).into_response()
        }
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

pub async fn mcp_spec(st: &Shared) -> Option<blazar_runtime::McpServerSpec> {
    if lark_settings(st).await["agent_access"] == "off" || lark_bin().await.is_none() {
        return None;
    }
    Some(blazar_runtime::McpServerSpec {
        name: "blazar-office".into(),
        command: std::env::current_exe().ok()?.display().to_string(),
        args: vec![
            MCP_SUBCOMMAND.into(),
            "--hub".into(),
            HUB_URL.get()?.clone(),
        ],
        ..Default::default()
    })
}

const TOOL_DESC: &str = "在用户本机执行飞书 / Lark 的官方命令行 lark-cli（消息、文档、多维表格、表格、日历、邮件、任务、会议纪要、知识库……）。\
args 是传给 lark-cli 的参数数组，不含 `lark-cli` 本身，例如 [\"calendar\",\"+agenda\"]、[\"im\",\"+messages-send\",\"--chat-id\",\"oc_xxx\",\"--text\",\"hi\"]。\
不熟悉某个业务域时先用 [\"<域>\",\"--help\"] 或 [\"skills\",\"read\",\"lark-im\"] 看用法；任何写操作都可以加 --dry-run 先看会发什么请求。\
登录 / 配置类命令、--yes 的高风险操作不可用；用户可能只开了只读。";

pub async fn serve_mcp(argv: &[String]) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let hub = argv
        .windows(2)
        .find(|w| w[0] == "--hub")
        .map(|w| w[1].clone())
        .ok_or_else(|| anyhow::anyhow!("用法: {MCP_SUBCOMMAND} --hub <url>"))?;
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = req["id"].clone();
        let result = match req["method"].as_str().unwrap_or_default() {
            "initialize" => {
                json!({ "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),
                "capabilities": { "tools": {} }, "serverInfo": { "name": "blazar-office", "version": env!("CARGO_PKG_VERSION") } })
            }
            "tools/list" => json!({ "tools": [{ "name": "lark", "description": TOOL_DESC,
                "inputSchema": { "type": "object", "required": ["args"], "properties": { "args": { "type": "array", "items": { "type": "string" }, "description": "lark-cli 的参数" } } } }] }),
            "tools/call" => {
                let args = req["params"]["arguments"]["args"].clone();
                let (text, err) =
                    match post_json(&hub, "/api/office/lark/run", &json!({ "args": args })).await {
                        Ok((200, v)) => (
                            format!(
                                "exit={} risk={}\n{}{}",
                                v["exit_code"],
                                v["risk"].as_str().unwrap_or(""),
                                v["stdout"].as_str().unwrap_or(""),
                                v["stderr"].as_str().unwrap_or("")
                            ),
                            v["exit_code"] != json!(0),
                        ),
                        Ok((_, v)) => (v["error"].as_str().unwrap_or("被拒绝了").to_owned(), true),
                        Err(e) => (format!("连不上 Blazar：{e}"), true),
                    };
                json!({ "content": [{ "type": "text", "text": text }], "isError": err })
            }
            _ if id.is_null() => continue,
            _ => json!({}),
        };
        out.write_all(
            format!(
                "{}\n",
                json!({ "jsonrpc": "2.0", "id": id, "result": result })
            )
            .as_bytes(),
        )
        .await?;
        out.flush().await?;
    }
    Ok(())
}

async fn post_json(hub: &str, path: &str, body: &Value) -> anyhow::Result<(u16, Value)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let host = hub
        .trim_end_matches('/')
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("hub 地址要以 http:// 开头"))?;
    let mut s = tokio::net::TcpStream::connect(host).await?;
    let payload = body.to_string();
    s.write_all(format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}", payload.len()).as_bytes()).await?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await?;
    let text = String::from_utf8_lossy(&raw);
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("不是合法的 HTTP 响应"))?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let (mut acc, mut r) = (String::new(), rest);
        while let Some((size, tail)) = r.split_once("\r\n") {
            let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
                break;
            };
            if n == 0 || tail.len() < n {
                break;
            }
            acc.push_str(&tail[..n]);
            r = tail[n..].trim_start_matches("\r\n");
        }
        acc
    } else {
        rest.to_owned()
    };
    Ok((status, serde_json::from_str(&body).unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        sv(v)
    }

    #[test]
    fn account_details_never_leave_the_status_call() {
        let raw = json!({ "appId": "cli_a1b2", "brand": "feishu", "defaultAs": "user", "identities": {
            "user": { "available": true, "status": "ok", "openId": "ou_secret", "userName": "张三", "scope": "im:message", "expiresAt": "2026-10-01T00:00:00Z", "message": "" },
            "bot": { "available": false, "status": "missing", "message": "not configured" } } });
        let r = redact_status(&raw);
        assert_eq!(r["user"]["available"], true);
        assert_eq!(r["bot"]["status"], "missing");
        let text = r.to_string();
        for leak in ["ou_secret", "张三", "cli_a1b2", "im:message"] {
            assert!(!text.contains(leak), "{leak}");
        }
    }

    #[test]
    fn login_config_and_yes_are_never_available_to_agents() {
        for bad in [
            a(&["auth", "login"]),
            a(&["config", "init"]),
            a(&["update"]),
            a(&["event", "consume", "x"]),
            a(&["im", "+chat-members-remove", "--yes"]),
            a(&["drive", "files", "delete", "-y"]),
            a(&[]),
        ] {
            assert!(static_risk(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            static_risk(&a(&["schema", "im.messages.list"])).unwrap(),
            Some("read")
        );
        assert_eq!(
            static_risk(&a(&["skills", "read", "lark-im"])).unwrap(),
            Some("read")
        );
        assert_eq!(
            static_risk(&a(&["api", "GET", "/open-apis/calendar/v4/calendars"])).unwrap(),
            Some("read")
        );
        assert_eq!(
            static_risk(&a(&["api", "POST", "/open-apis/im/v1/messages"])).unwrap(),
            Some("write")
        );
        assert_eq!(static_risk(&a(&["calendar", "+agenda"])).unwrap(), None);

        assert_eq!(
            static_risk(&a(&["calendar", "--help"])).unwrap(),
            Some("read")
        );
        assert_eq!(
            static_risk(&a(&["im", "+messages-send", "-h"])).unwrap(),
            Some("read")
        );
        assert!(static_risk(&a(&["auth", "--help"])).is_err());

        assert!(static_risk(&a(&["install", "--help"])).is_err());
        assert!(static_risk(&a(&["install", "-h"])).is_err());
        assert!(static_risk(&a(&["profile", "use", "other"])).is_err());
    }

    #[test]
    fn risk_is_read_from_the_commands_own_help() {
        assert_eq!(
            risk_from_help("Create a doc\n\nRisk: write\n\nUsage:"),
            Some("write")
        );
        assert_eq!(risk_from_help("x\nRisk: read\n"), Some("read"));
        assert_eq!(
            risk_from_help("x\n  Risk: high-risk-write\n"),
            Some("high-risk-write")
        );
        assert_eq!(risk_from_help("no such line"), None);
        assert_eq!(
            command_path(&a(&[
                "mail",
                "user_mailbox.messages",
                "list",
                "--user-mailbox-id",
                "me"
            ])),
            a(&["mail", "user_mailbox.messages", "list"])
        );
    }

    #[test]
    fn notify_targets_and_args() {
        assert!(valid_target("oc_abc123") && valid_target("ou_x-y_z"));
        for bad in ["", "oc_", "abc", "oc_a b", "oc_$(id)", "--chat-id"] {
            assert!(!valid_target(bad), "{bad}");
        }
        let g = notify_args("oc_abc", "bot", "**hi**", "blz-1", true);
        assert_eq!(
            &g[..4],
            &a(&["im", "+messages-send", "--chat-id", "oc_abc"])[..]
        );
        assert!(g.contains(&"--dry-run".to_owned()) && g.windows(2).any(|w| w == ["--as", "bot"]));
        assert_eq!(notify_args("ou_me", "", "x", "k", false)[2], "--user-id");
    }

    #[test]
    fn branch_prefixes_stay_valid_git_refs() {
        for ok in ["blazar/", "me/", "feat-", "a.b/c_", ""] {
            assert!(valid_branch_prefix(ok), "{ok}");
        }
        for bad in ["-x/", "/x", "a..b/", "a//b", "a b/", ".hidden/", "x~1/"] {
            assert!(!valid_branch_prefix(bad), "{bad}");
        }
    }
}
