use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};

use axum::Json;
use axum::extract::{Path, Query, State, WebSocketUpgrade, ws};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use chrono::{DateTime, Duration, Utc};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;
use crate::state::ServerEvent;

pub const PROVIDERS: &[&str] = &["claude", "codex"];

const SWITCH_AT: f64 = 0.9;

const BLOCKED_MINUTES: i64 = 60;

const STALE_HOURS: i64 = 5;

pub const TOKEN_FILE: &str = ".blazar-oauth-token";
const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

const QUOTA_MODEL: &str = "claude-haiku-4-5-20251001";
const QUOTA_FRESH_MINUTES: i64 = 5;

#[must_use]
pub fn env_key(provider: &str) -> Option<&'static str> {
    match provider {
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "codex" => Some("CODEX_HOME"),
        _ => None,
    }
}

fn default_id(provider: &str) -> String {
    format!("{provider}-default")
}

fn mode_key(provider: &str) -> String {
    format!("accounts.mode.{provider}")
}

fn shared_items(provider: &str) -> &'static [(&'static str, bool)] {
    match provider {
        "claude" => &[
            ("settings.json", false),
            ("keybindings.json", false),
            ("CLAUDE.md", false),
            ("skills", false),
            ("commands", false),
            ("agents", false),
            ("projects", true),
            ("history.jsonl", false),
        ],
        "codex" => &[
            ("config.toml", false),
            ("AGENTS.md", false),
            ("prompts", false),
            ("skills", false),
            ("rules", false),
            ("sessions", true),
            ("history.jsonl", false),
        ],
        _ => &[],
    }
}

pub(crate) fn default_home(provider: &str) -> Option<PathBuf> {
    if let Some(dir) = env_key(provider)
        .and_then(std::env::var_os)
        .filter(|v| !v.is_empty())
    {
        return Some(PathBuf::from(dir));
    }
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    Some(home.join(match provider {
        "claude" => ".claude",
        _ => ".codex",
    }))
}

#[cfg(unix)]
pub fn prepare_dir(
    provider: &str,
    dir: &FsPath,
    shared_from: Option<&FsPath>,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let Some(src) = shared_from.filter(|s| s.is_dir()) else {
        return Ok(());
    };
    for (name, ensure) in shared_items(provider) {
        let from = src.join(name);
        let to = dir.join(name);
        if to.symlink_metadata().is_ok() {
            continue;
        }
        if !from.exists() {
            if !ensure {
                continue;
            }
            std::fs::create_dir_all(&from)?;
        }
        std::os::unix::fs::symlink(&from, &to)?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn prepare_dir(
    _provider: &str,
    dir: &FsPath,
    _shared_from: Option<&FsPath>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    blazar_core_types::private_storage::protect_directory(dir)
}

fn root(st: &Shared) -> PathBuf {
    st.mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, FsPath::to_path_buf)
        .join("accounts")
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub config_dir: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,

    pub status: String,
    pub disabled: bool,
    pub checked_at: Option<String>,
    pub created_at: String,

    pub builtin: bool,

    pub kind: String,
}

impl Account {
    fn usable(&self) -> bool {
        !self.disabled && self.status != "logged_out"
    }

    fn env(&self) -> Option<(String, String)> {
        let key = env_key(&self.provider)?;
        self.config_dir.clone().map(|d| (key.to_owned(), d))
    }

    fn token_file(&self) -> Option<PathBuf> {
        (self.kind == "token")
            .then(|| {
                self.config_dir
                    .as_ref()
                    .map(|d| PathBuf::from(d).join(TOKEN_FILE))
            })
            .flatten()
    }
}

fn from_row(r: &sqlx::sqlite::SqliteRow) -> Account {
    let config_dir: Option<String> = r.try_get("config_dir").ok().flatten();
    Account {
        id: r.try_get("id").unwrap_or_default(),
        provider: r.try_get("provider").unwrap_or_default(),
        label: r.try_get("display_name").unwrap_or_default(),
        builtin: config_dir.is_none(),
        config_dir,
        email: r.try_get("email").ok().flatten(),
        plan: r.try_get("plan").ok().flatten(),
        status: r.try_get("status").unwrap_or_default(),
        disabled: r.try_get::<i64, _>("disabled").unwrap_or(0) != 0,
        checked_at: r.try_get("checked_at").ok().flatten(),
        created_at: r.try_get("created_at").unwrap_or_default(),
        kind: match r.try_get::<String, _>("auth_mode").as_deref() {
            Ok("token") => "token",
            _ => "login",
        }
        .to_owned(),
    }
}

pub async fn get(st: &Shared, id: &str) -> Option<Account> {
    sqlx::query("SELECT * FROM accounts WHERE id = ?1")
        .bind(id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| from_row(&r))
}

async fn all(st: &Shared, provider: Option<&str>) -> Vec<Account> {
    sqlx::query(
        "SELECT * FROM accounts WHERE (?1 IS NULL OR provider = ?1)
         ORDER BY provider, config_dir IS NOT NULL, created_at",
    )
    .bind(provider)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(from_row)
    .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Window {
    pub name: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
    pub observed_at: String,
}

impl Window {
    fn live(&self, now: DateTime<Utc>) -> bool {
        let at = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|t| t.with_timezone(&Utc))
        };
        match self.resets_at.as_deref().and_then(at) {
            Some(reset) => reset > now,
            None => {
                let ttl = if self.name == "blocked" {
                    Duration::minutes(BLOCKED_MINUTES)
                } else {
                    Duration::hours(STALE_HOURS)
                };
                at(&self.observed_at).is_some_and(|seen| seen + ttl > now)
            }
        }
    }
}

#[must_use]
pub fn score(windows: &[Window], now: DateTime<Utc>) -> f64 {
    windows
        .iter()
        .filter(|w| w.live(now))
        .map(|w| w.utilization)
        .fold(0.0, f64::max)
}

async fn windows_of(st: &Shared) -> HashMap<String, Vec<Window>> {
    let rows = sqlx::query(
        "SELECT account_id, window_name, utilization, resets_at, observed_at
         FROM rate_limit_snapshots ORDER BY window_name",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut out: HashMap<String, Vec<Window>> = HashMap::new();
    for r in rows {
        out.entry(r.try_get("account_id").unwrap_or_default())
            .or_default()
            .push(Window {
                name: r.try_get("window_name").unwrap_or_default(),
                utilization: r.try_get("utilization").unwrap_or(0.0),
                resets_at: r.try_get("resets_at").ok().flatten(),
                observed_at: r.try_get("observed_at").unwrap_or_default(),
            });
    }
    out
}

async fn running_of(st: &Shared) -> HashMap<String, i64> {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT account_id, COUNT(*) FROM sessions
         WHERE status = 'running' AND account_id IS NOT NULL GROUP BY account_id",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub score: f64,
    pub running: i64,
}

#[must_use]
pub fn best(cands: &[Candidate]) -> Option<&Candidate> {
    cands
        .iter()
        .min_by(|a, b| a.score.total_cmp(&b.score).then(a.running.cmp(&b.running)))
}

#[derive(Debug, Clone)]
pub struct Chosen {
    pub id: String,
    pub env: Option<(String, String)>,

    pub token_file: Option<(String, PathBuf)>,

    pub auto: bool,

    pub proxy: bool,
}

impl From<&Account> for Chosen {
    fn from(a: &Account) -> Self {
        Self {
            id: a.id.clone(),
            env: a.env(),
            token_file: a.token_file().map(|p| (TOKEN_ENV.to_owned(), p)),
            auto: false,
            proxy: false,
        }
    }
}

const MODEL_BLOCK_DAYS: i64 = 7;

#[must_use]
pub fn model_family(model: &str) -> String {
    let m = model.trim().to_ascii_lowercase();
    let m = m.split('[').next().unwrap_or_default();
    let m = m.strip_prefix("claude-").unwrap_or(m);
    m.split(|c: char| !c.is_ascii_alphabetic())
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_owned()
}

async fn model_blocks(st: &Shared) -> HashMap<String, Vec<(String, String)>> {
    let since = (Utc::now() - Duration::days(MODEL_BLOCK_DAYS)).to_rfc3339();
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (acc, model, at) in sqlx::query_as::<_, (String, String, String)>(
        "SELECT account_id, model, observed_at FROM account_model_blocks
         WHERE observed_at > ?1 ORDER BY model",
    )
    .bind(since)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    {
        out.entry(acc).or_default().push((model, at));
    }
    out
}

async fn mode_of(st: &Shared, provider: &str) -> String {
    crate::office::kv_get(st, &mode_key(provider))
        .await
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

pub async fn resolve(
    st: &Shared,
    provider: &str,
    run_node: &str,
    requested: Option<&str>,
    prior: Option<&str>,
    model: Option<&str>,
    exclude: &[String],
) -> Result<Option<Chosen>, String> {
    if env_key(provider).is_none() {
        return Ok(None);
    }
    let remote = run_node != "local";
    if remote && provider == "codex" {
        let chosen = pick(st, provider, false, requested, prior, model, exclude).await?;
        return Ok(chosen.map(|c| Chosen {
            env: None,
            token_file: None,
            proxy: true,
            ..c
        }));
    }
    if remote && provider != "claude" {
        return Ok(None);
    }
    let chosen = pick(st, provider, remote, requested, prior, model, exclude).await?;
    if !remote {
        return Ok(chosen);
    }
    match chosen {
        Some(c) if c.token_file.is_some() => Ok(Some(Chosen {
            env: None,
            token_file: None,
            proxy: true,
            ..c
        })),
        Some(c) => Err(format!(
            "「{}」是浏览器登录的账号，凭据没法带到 {run_node} 上。远端请选长期 token 账号（claude setup-token）",
            get(st, &c.id).await.map(|a| a.label).unwrap_or(c.id)
        )),
        None => Err(format!(
            "{run_node} 上运行 Claude Code 需要一个长期 token 账号"
        )),
    }
}

async fn pick(
    st: &Shared,
    provider: &str,
    remote: bool,
    requested: Option<&str>,
    prior: Option<&str>,
    model: Option<&str>,
    exclude: &[String],
) -> Result<Option<Chosen>, String> {
    let mode = match requested.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.to_owned(),
        None => mode_of(st, provider).await,
    };
    let fallback = || async {
        get(st, &default_id(provider))
            .await
            .map(|a| Chosen::from(&a))
    };
    match mode.as_str() {
        "" | "default" => Ok(fallback().await),
        "auto" => {
            let now = Utc::now();
            let windows = windows_of(st).await;
            let running = running_of(st).await;
            let blocks = model_blocks(st).await;
            let family = model.map(model_family).filter(|f| !f.is_empty());
            let can_run = |id: &str| {
                family.as_ref().is_none_or(|f| {
                    blocks
                        .get(id)
                        .is_none_or(|ms| ms.iter().all(|(m, _)| model_family(m) != *f))
                })
            };
            let cands: Vec<Candidate> = all(st, Some(provider))
                .await
                .into_iter()
                .filter(|a| {
                    a.usable() && !exclude.contains(&a.id) && (!remote || a.kind == "token")
                })
                .map(|a| Candidate {
                    score: windows.get(&a.id).map_or(0.0, |w| score(w, now)),
                    running: running.get(&a.id).copied().unwrap_or(0),
                    id: a.id,
                })
                .collect();
            let fit: Vec<Candidate> = cands
                .iter()
                .filter(|c| can_run(&c.id) && (exclude.is_empty() || c.score < 1.0))
                .cloned()
                .collect();
            let sticky = prior.and_then(|p| fit.iter().find(|c| c.id == p && c.score < SWITCH_AT));
            let pick = match sticky.or_else(|| best(&fit)) {
                Some(c) => Some(c.id.clone()),
                None if !exclude.is_empty() => {
                    return Err(
                        "没有别的账号能接手：都试过了、额度用完了，或者用不了这个模型".into(),
                    );
                }
                None => best(&cands).map(|c| c.id.clone()),
            };
            let chosen = match pick {
                Some(id) => get(st, &id).await.map(|a| Chosen::from(&a)),
                None => fallback().await,
            };
            Ok(chosen.map(|c| Chosen { auto: true, ..c }))
        }
        id => {
            let a = get(st, id)
                .await
                .ok_or("绑定的账号已经被删除，去「账号」页重新选一个")?;
            if a.provider != provider {
                return Err(format!("账号「{}」不是 {provider} 的账号", a.label));
            }
            if a.disabled {
                return Err(format!("账号「{}」已停用", a.label));
            }
            Ok(Some(Chosen::from(&a)))
        }
    }
}

async fn account_of_session(st: &Shared, sid: blazar_core_types::SessionId) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT account_id FROM sessions WHERE id = ?1")
        .bind(sid.to_string())
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .flatten()
}

async fn upsert_window(
    st: &Shared,
    account: &str,
    name: &str,
    util: f64,
    resets_at: Option<String>,
) {
    let _ = sqlx::query(
        "INSERT INTO rate_limit_snapshots (account_id, window_name, utilization, resets_at, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (account_id, window_name) DO UPDATE SET
           utilization = excluded.utilization, resets_at = excluded.resets_at,
           observed_at = excluded.observed_at",
    )
    .bind(account)
    .bind(name)
    .bind(util.clamp(0.0, 1.0))
    .bind(resets_at)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
}

pub async fn note_rate_limit(
    st: &Shared,
    sid: blazar_core_types::SessionId,
    rl: &blazar_core_types::RateLimit,
) {
    let Some(account) = account_of_session(st, sid).await else {
        return;
    };
    for w in &rl.windows {
        upsert_window(
            st,
            &account,
            &w.name,
            w.utilization,
            w.resets_at.map(|t| t.to_rfc3339()),
        )
        .await;
    }
    if rl.allowed {
        let _ = sqlx::query(
            "DELETE FROM rate_limit_snapshots WHERE account_id = ?1 AND window_name = 'blocked'",
        )
        .bind(&account)
        .execute(st.db.pool())
        .await;
    } else if !rl.windows.is_empty() && rl.windows.iter().all(|w| w.utilization < 1.0) {
        upsert_window(st, &account, "blocked", 1.0, None).await;
    }
    st.emit(ServerEvent::AccountsChanged);
}

async fn session_model(st: &Shared, sid: blazar_core_types::SessionId) -> Option<String> {
    let started: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_id = ?1
           AND payload LIKE '{\"type\":\"session_started\"%' ORDER BY seq DESC LIMIT 1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let request: Option<String> =
        sqlx::query_scalar::<_, Option<String>>("SELECT request FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .flatten();
    [started, request]
        .into_iter()
        .flatten()
        .filter_map(|p| serde_json::from_str::<Value>(&p).ok())
        .find_map(|v| v["model"].as_str().map(str::to_owned))
        .filter(|m| !m.trim().is_empty())
}

async fn note_model_block(
    st: &Shared,
    sid: blazar_core_types::SessionId,
    account: &str,
    why: &str,
) {
    let Some(model) = session_model(st, sid).await else {
        return;
    };
    let model = model
        .split('[')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let _ = sqlx::query(
        "INSERT INTO account_model_blocks (account_id, model, reason, observed_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (account_id, model) DO UPDATE SET reason = excluded.reason, observed_at = excluded.observed_at",
    )
    .bind(account)
    .bind(&model)
    .bind(why.chars().take(300).collect::<String>())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    st.emit(ServerEvent::AccountsChanged);
}

pub async fn on_run_finished(st: Shared, sid: blazar_core_types::SessionId, status: &'static str) {
    if let Ok(Some((Some(acc), rt))) = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT account_id, runtime_kind FROM sessions WHERE id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
        && rt == "codex"
        && let Some(a) = get(&st, &acc).await
    {
        let st = st.clone();
        tokio::spawn(async move {
            if let Err(e) = refresh_quota(&st, &a).await {
                tracing::info!(target: "blazar::accounts", "更新 Codex 额度失败：{e}");
            }
        });
    }
    if status != "failed" {
        return;
    }
    type Row = (
        String,
        Option<String>,
        i64,
        Option<String>,
        String,
        String,
        String,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT workspace_id, account_id, account_auto, request, COALESCE(thread_id, id), runtime_kind,
                COALESCE(node, 'local')
         FROM sessions WHERE id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let Some((ws, Some(account), 1, Some(raw), thread, runtime, node)) = row else {
        return;
    };
    let worked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND (
           payload LIKE '{\"type\":\"assistant_message\"%' OR payload LIKE '{\"type\":\"tool_use\"%'
           OR payload LIKE '{\"type\":\"thinking\"%')",
    )
    .bind(sid.to_string())
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(1);
    let midway = worked > 0;
    let why: String = sqlx::query_scalar::<_, String>(
        "SELECT payload FROM events WHERE session_id = ?1 AND (
           payload LIKE '{\"type\":\"finished\"%' OR payload LIKE '{\"type\":\"error\"%')
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| v["message"].as_str().map(str::to_owned))
    .unwrap_or_default();
    let class = blazar_core_types::FailureClass::classify(&why);
    if !model_only_limit(&why) && !class.should_switch_account() {
        return;
    }
    let Ok(req) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    let mut exclude: Vec<String> =
        serde_json::from_value(req["exclude_accounts"].clone()).unwrap_or_default();
    exclude.push(account.clone());
    let model = session_model(&st, sid).await;
    match resolve(
        &st,
        &runtime,
        &node,
        Some("auto"),
        None,
        model.as_deref(),
        &exclude,
    )
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(e) => {
            tracing::info!(target: "blazar::accounts", "{sid} 不自动换号重发：{e}");
            return;
        }
    }

    if !midway {
        let _ = sqlx::query("UPDATE sessions SET provider_session_id = NULL WHERE id = ?1")
            .bind(sid.to_string())
            .execute(st.db.pool())
            .await;
    }
    let req = retry_request(req, midway, &exclude, &thread);
    let Ok(parsed) = serde_json::from_value::<crate::api::PromptRequest>(req) else {
        return;
    };
    let from = get(&st, &account).await.map(|a| a.label).unwrap_or(account);
    match send_again(&st, &ws, parsed).await {
        Ok(Json(v)) if v["admitted"] != json!(false) => {
            let to = match v["account"].as_str() {
                Some(id) => get(&st, id).await.map(|a| a.label).unwrap_or_default(),
                None => String::new(),
            };
            let ws_name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = ?1")
                .bind(&ws)
                .fetch_optional(st.db.pool())
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            crate::inbox::push(
                &st,
                crate::inbox::Item {
                    kind: "rate_limit",
                    title: if midway {
                        format!("{ws_name} · 「{from}」中途停了，已换到「{to}」接着做")
                    } else {
                        format!("{ws_name} · 已从「{from}」换到「{to}」重发")
                    },
                    body: why.chars().take(200).collect(),
                    workspace_id: Some(ws),
                    thread_id: Some(thread),
                    session_id: v["session_id"].as_str().map(str::to_owned),
                    ref_id: None,
                },
            )
            .await;
        }
        Ok(Json(v)) => tracing::info!(target: "blazar::accounts", "换号重发没被接受：{v}"),
        Err(e) => tracing::warn!(target: "blazar::accounts", "换号重发失败：{}", e.message()),
    }
}

#[must_use]
pub fn retry_request(mut req: Value, midway: bool, exclude: &[String], thread: &str) -> Value {
    if midway {
        req["text"] = json!(CONTINUE_TEXT);
        req["images"] = json!([]);
        req["context_file"] = Value::Null;
        req["resume"] = json!(true);
        req["resume_at_last"] = json!(false);
    } else if req["resume"].as_bool() == Some(true) {
        req["resume_at_last"] = json!(true);
    }
    req["exclude_accounts"] = json!(exclude);
    req["account"] = json!("auto");
    req["retry_thread"] = json!(thread);
    req["resume_session"] = json!(thread);
    req["wait_secs"] = json!(0);
    req
}

pub const CONTINUE_TEXT: &str =
    "（上一个账号额度用尽，已换账号接着做）请从刚才中断的地方继续，把没做完的工作完成。";

fn send_again<'a>(
    st: &'a Shared,
    ws: &'a str,
    req: crate::api::PromptRequest,
) -> futures::future::BoxFuture<'a, Result<Json<Value>, crate::api::ApiError>> {
    Box::pin(crate::api::prompt(
        State(st.clone()),
        Path(ws.to_owned()),
        Json(req),
    ))
}

#[must_use]
pub fn model_only_limit(message: &str) -> bool {
    let t = message.to_ascii_lowercase();
    [
        "switch to another model",
        "requires usage credits",
        "requires extra usage",
    ]
    .iter()
    .any(|p| t.contains(p))
}

pub async fn note_failure(st: &Shared, sid: blazar_core_types::SessionId, message: &str) {
    use blazar_core_types::FailureClass;
    if model_only_limit(message) {
        if let Some(account) = account_of_session(st, sid).await {
            note_model_block(st, sid, &account, message).await;
        }
        return;
    }
    let class = FailureClass::classify(message);
    if !class.should_switch_account() {
        return;
    }
    let Some(account) = account_of_session(st, sid).await else {
        return;
    };
    if class == FailureClass::QuotaLimit {
        let now = Utc::now();
        let full = windows_of(st).await.get(&account).is_some_and(|ws| {
            ws.iter()
                .any(|w| w.live(now) && w.resets_at.is_some() && w.utilization >= 1.0)
        });
        if !full {
            upsert_window(st, &account, "blocked", 1.0, None).await;
        }
        st.emit(ServerEvent::AccountsChanged);
        return;
    }
    let Some(a) = get(st, &account).await else {
        return;
    };
    if a.kind == "token" {
        let _ =
            sqlx::query("UPDATE accounts SET status = 'logged_out', checked_at = ?2 WHERE id = ?1")
                .bind(&a.id)
                .bind(Utc::now().to_rfc3339())
                .execute(st.db.pool())
                .await;
        st.emit(ServerEvent::AccountsChanged);
    } else {
        check(st, &a).await;
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub logged_in: Option<bool>,
    pub email: Option<String>,
    pub plan: Option<String>,
}

#[must_use]
pub fn parse_claude_status(out: &str) -> Probe {
    let json = match (out.find('{'), out.rfind('}')) {
        (Some(a), Some(b)) if a < b => &out[a..=b],
        _ => return Probe::default(),
    };
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Probe::default();
    };
    let s = |k: &str| v[k].as_str().map(str::to_owned).filter(|x| !x.is_empty());
    let logged_in = v["loggedIn"].as_bool();
    Probe {
        logged_in,
        email: s("email").filter(|_| logged_in == Some(true)),
        plan: s("subscriptionType")
            .or_else(|| s("authMethod").filter(|m| m != "none" && m != "claude.ai"))
            .filter(|_| logged_in == Some(true)),
    }
}

#[must_use]
pub fn parse_codex_status(out: &str) -> Probe {
    let t = out.to_ascii_lowercase();
    if t.contains("not logged in") {
        return Probe {
            logged_in: Some(false),
            ..Probe::default()
        };
    }
    if !t.contains("logged in") {
        return Probe::default();
    }
    Probe {
        logged_in: Some(true),
        email: None,
        plan: t.contains("api key").then(|| "API key".into()),
    }
}

fn shell(provider: &str, cmd: &str) -> String {
    let program = match provider {
        "claude" => "claude",
        _ => "codex",
    };
    format!("{}\n{program} {cmd}", blazar_transport::PATH_PRELUDE)
}

pub async fn probe(a: &Account) -> Probe {
    let cmd = match a.provider.as_str() {
        "claude" => "auth status --json 2>&1",
        _ => "login status 2>&1",
    };
    let mut spec = ExecSpec::new("bash")
        .arg("-lc")
        .arg(shell(&a.provider, cmd));
    if let Some((k, v)) = a.env() {
        spec = spec.env(k, v);
    }
    if let Some(p) = a.token_file() {
        spec = spec.env_file(TOKEN_ENV, p);
    }
    let fut = LocalTransport.exec(spec);
    let Ok(Ok(out)) = tokio::time::timeout(std::time::Duration::from_secs(20), fut).await else {
        return Probe::default();
    };
    if out.code == 127 {
        return Probe::default();
    }
    match a.provider.as_str() {
        "claude" => parse_claude_status(&out.stdout),
        _ => parse_codex_status(&out.stdout),
    }
}

async fn check(st: &Shared, a: &Account) -> Option<Account> {
    if st.services == crate::services::Services::Isolated {
        return None;
    }
    let p = probe(a).await;
    let status = match p.logged_in {
        Some(true) => "ok",
        Some(false) => "logged_out",
        None => "unknown",
    };
    let _ = sqlx::query(
        "UPDATE accounts SET status = ?2, email = COALESCE(?3, email), plan = COALESCE(?4, plan),
                checked_at = ?5 WHERE id = ?1",
    )
    .bind(&a.id)
    .bind(status)
    .bind(&p.email)
    .bind(p.plan.as_ref().filter(|_| a.kind != "token"))
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    st.emit(ServerEvent::AccountsChanged);
    get(st, &a.id).await
}

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

pub async fn list(State(st): State<Shared>) -> Response {
    let now = Utc::now();
    let windows = windows_of(&st).await;
    let running = running_of(&st).await;
    let blocks = model_blocks(&st).await;
    let last: HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
        "SELECT account_id, MAX(created_at) FROM sessions WHERE account_id IS NOT NULL GROUP BY account_id",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .into_iter()
    .collect();
    let mut accounts = Vec::new();
    for a in all(&st, None).await {
        let w = windows.get(&a.id).cloned().unwrap_or_default();
        let mut v = serde_json::to_value(&a).unwrap_or_default();
        v["score"] = json!(score(&w, now));
        v["windows"] = json!(w.iter().filter(|x| x.live(now)).collect::<Vec<_>>());
        v["running"] = json!(running.get(&a.id).copied().unwrap_or(0));
        v["last_used_at"] = json!(last.get(&a.id));
        v["model_blocks"] = json!(
            blocks
                .get(&a.id)
                .map(|ms| ms
                    .iter()
                    .map(|(m, at)| json!({ "model": m, "observed_at": at }))
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        );
        accounts.push(v);
    }
    let mut modes = serde_json::Map::new();
    for p in PROVIDERS {
        modes.insert((*p).to_owned(), json!(mode_of(&st, p).await));
    }
    Json(json!({
        "accounts": accounts,
        "modes": modes,
        "root": root(&st).display().to_string(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct NewAccount {
    pub provider: String,
    pub label: String,
    #[serde(default)]
    pub token: Option<String>,
}

pub fn valid_token(s: &str) -> Result<String, String> {
    let t = s.trim();
    if !t.starts_with("sk-ant-oat") {
        return Err("不像是 claude setup-token 生成的 token（应以 sk-ant-oat 开头）".into());
    }
    if t.len() > 512
        || !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err("token 里有多余的字符，确认只复制了 token 本身".into());
    }
    Ok(t.to_owned())
}

fn write_token(path: &FsPath, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode(0o600);
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    blazar_core_types::private_storage::open(path, &opts)?.write_all(token.as_bytes())
}

fn valid_label(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() || s.chars().count() > 40 || s.chars().any(char::is_control) {
        return Err("名字 1–40 个字".into());
    }
    Ok(s.to_owned())
}

pub async fn create(State(st): State<Shared>, Json(req): Json<NewAccount>) -> Response {
    if !PROVIDERS.contains(&req.provider.as_str()) {
        return fail(
            StatusCode::BAD_REQUEST,
            format!("不支持的运行时 {}", req.provider),
        );
    }
    let label = match valid_label(&req.label) {
        Ok(l) => l,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let token = match req.token.as_deref().filter(|t| !t.trim().is_empty()) {
        None => None,
        Some(_) if req.provider != "claude" => {
            return fail(StatusCode::BAD_REQUEST, "只有 Claude Code 支持长期 token");
        }
        Some(t) => match valid_token(t) {
            Ok(t) => Some(t),
            Err(e) => return fail(StatusCode::BAD_REQUEST, e),
        },
    };
    let id = uuid::Uuid::now_v7().to_string();
    let dir = root(&st).join(&req.provider).join(&id);
    let shared = st.services.local(|| default_home(&req.provider)).flatten();
    let (provider, d) = (req.provider.clone(), dir.clone());
    let tok = token.clone();
    let made = tokio::task::spawn_blocking(move || {
        prepare_dir(&provider, &d, shared.as_deref())?;
        match tok {
            Some(t) => write_token(&d.join(TOKEN_FILE), &t),
            None => Ok(()),
        }
    })
    .await;
    if let Err(e) = made
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
    {
        let _ = std::fs::remove_dir_all(&dir);
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("创建账号目录失败: {e}"),
        );
    }
    let res = sqlx::query(
        "INSERT INTO accounts (id, provider, display_name, auth_mode, status, config_dir, created_at)
         VALUES (?1, ?2, ?3, ?6, 'logged_out', ?4, ?5)",
    )
    .bind(&id)
    .bind(&req.provider)
    .bind(&label)
    .bind(dir.display().to_string())
    .bind(Utc::now().to_rfc3339())
    .bind(if token.is_some() { "token" } else { "cli" })
    .execute(st.db.pool())
    .await;
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&dir);
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    st.emit(ServerEvent::AccountsChanged);
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, "账号写入后读不回来");
    };
    let a = if token.is_some() {
        check(&st, &a).await.unwrap_or(a)
    } else {
        a
    };
    Json(a).into_response()
}

#[derive(Deserialize)]
pub struct NewToken {
    pub token: String,
}

pub async fn set_token(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<NewToken>,
) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    };
    if a.provider != "claude" {
        return fail(StatusCode::CONFLICT, "只有 Claude Code 账号能用长期 token");
    }
    let token = match valid_token(&req.token) {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let (dir, adopt) = match a.config_dir.clone() {
        Some(d) => (PathBuf::from(d), false),
        None => (root(&st).join("claude").join(&a.id), true),
    };
    let shared = st.services.local(|| default_home("claude")).flatten();
    let (d, path) = (dir.clone(), dir.join(TOKEN_FILE));
    let written = tokio::task::spawn_blocking(move || {
        if adopt {
            prepare_dir("claude", &d, shared.as_deref())?;
        }
        write_token(&path, &token)
    })
    .await;
    match written {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("保存 token 失败: {e}"),
            );
        }
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    let _ = sqlx::query(
        "UPDATE accounts SET auth_mode = 'token', config_dir = COALESCE(config_dir, ?2), email = NULL
         WHERE id = ?1",
    )
    .bind(&id)
    .bind(dir.display().to_string())
        .execute(st.db.pool())
        .await;
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    };
    let a = check(&st, &a).await.unwrap_or(a);
    Json(a).into_response()
}

#[derive(Deserialize)]
pub struct Patch {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub disabled: Option<bool>,

    #[serde(default)]
    pub plan: Option<String>,
}

pub const PLANS: &[&str] = &["pro", "max5x", "max20x", "team", "enterprise"];

pub async fn update(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<Patch>,
) -> Response {
    if get(&st, &id).await.is_none() {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    }
    let label = match p.label.as_deref().map(valid_label).transpose() {
        Ok(l) => l,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    if let Some(plan) = p.plan.as_deref().filter(|x| !x.is_empty())
        && !PLANS.contains(&plan)
    {
        return fail(StatusCode::BAD_REQUEST, format!("未知订阅 {plan}"));
    }
    let _ = sqlx::query(
        "UPDATE accounts SET display_name = COALESCE(?2, display_name),
                disabled = COALESCE(?3, disabled),
                plan = CASE WHEN ?4 IS NULL THEN plan WHEN ?4 = '' THEN NULL ELSE ?4 END
         WHERE id = ?1",
    )
    .bind(&id)
    .bind(label)
    .bind(p.disabled.map(i64::from))
    .bind(p.plan.as_deref())
    .execute(st.db.pool())
    .await;
    st.emit(ServerEvent::AccountsChanged);
    get(&st, &id).await.map_or_else(
        || fail(StatusCode::NOT_FOUND, "没有这个账号"),
        |a| Json(a).into_response(),
    )
}

pub async fn remove(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    };
    let Some(dir) = a.config_dir.clone() else {
        return fail(StatusCode::CONFLICT, "默认登录不能删除，可以停用");
    };
    if running_of(&st).await.get(&id).copied().unwrap_or(0) > 0 {
        return fail(
            StatusCode::CONFLICT,
            "还有会话在用这个账号，等它结束或先中断",
        );
    }

    let logout = match a.provider.as_str() {
        "claude" => "auth logout",
        _ => "logout",
    };
    if let Some((k, v)) = a.env().filter(|_| a.kind == "login") {
        let spec = ExecSpec::new("bash")
            .arg("-lc")
            .arg(shell(&a.provider, &format!("{logout} >/dev/null 2>&1")))
            .env(k, v);
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            LocalTransport.exec(spec),
        )
        .await;
    }
    let dir = PathBuf::from(dir);
    if dir.starts_with(root(&st)) && dir.is_dir() {
        let d = dir.clone();
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(d)).await {
            tracing::warn!(target: "blazar::accounts", "删除账号目录 {} 失败: {e}", dir.display());
        }
    }
    let pool = st.db.pool();
    let _ = sqlx::query("DELETE FROM rate_limit_snapshots WHERE account_id = ?1")
        .bind(&id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE id = ?1")
        .bind(&id)
        .execute(pool)
        .await;
    let _ = sqlx::query("UPDATE agents SET account = NULL WHERE account = ?1")
        .bind(&id)
        .execute(pool)
        .await;
    if mode_of(&st, &a.provider).await == id {
        let _ = crate::office::kv_put(&st, &mode_key(&a.provider), &json!("")).await;
    }
    st.emit(ServerEvent::AccountsChanged);
    StatusCode::NO_CONTENT.into_response()
}

pub async fn check_one(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    };
    match check(&st, &a).await {
        Some(a) => Json(a).into_response(),
        None => fail(StatusCode::NOT_FOUND, "没有这个账号"),
    }
}

pub async fn check_all(State(st): State<Shared>) -> Response {
    let list = all(&st, None).await;
    let checks = list.iter().map(|a| check(&st, a));
    let n = futures::future::join_all(checks).await.len();

    let now = Utc::now();
    let windows = windows_of(&st).await;
    let stale = |a: &Account| {
        windows.get(&a.id).is_none_or(|ws| {
            ws.iter().all(|w| {
                DateTime::parse_from_rfc3339(&w.observed_at).is_ok_and(|t| {
                    t.with_timezone(&Utc) + Duration::minutes(QUOTA_FRESH_MINUTES) < now
                })
            })
        })
    };
    let due: Vec<Account> = all(&st, None)
        .await
        .into_iter()
        .filter(|a| can_query_quota(a) && !a.disabled && a.status != "logged_out" && stale(a))
        .collect();
    let refreshed = futures::future::join_all(due.iter().map(|a| refresh_quota(&st, a))).await;
    Json(json!({ "checked": n, "quota": refreshed.iter().filter(|r| r.is_ok()).count() }))
        .into_response()
}

pub async fn quota(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    };
    if !can_query_quota(&a) {
        return fail(
            StatusCode::CONFLICT,
            "浏览器登录的 Claude 账号查不了额度，跑一次会话就会更新",
        );
    }
    match refresh_quota(&st, &a).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Quota {
    pub windows: Vec<(String, f64, Option<String>)>,
    pub rejected: bool,
    pub reset: Option<String>,
}

fn window_name(key: &str) -> Option<String> {
    let (n, rest) = key.split_at(key.find(|c: char| !c.is_ascii_digit())?);
    if n.is_empty() {
        return None;
    }
    let (unit, scope) = rest.split_at(1);
    if !scope.is_empty() && !scope.starts_with('_') {
        return None;
    }
    let base = match (n, unit) {
        ("5", "h") => "five_hour".to_owned(),
        ("7", "d") => "seven_day".to_owned(),
        (n, "h") => format!("{n}_hour"),
        (n, "d") => format!("{n}_day"),
        _ => return None,
    };
    Some(format!("{base}{scope}"))
}

#[must_use]
pub fn parse_quota_headers(raw: &str) -> Quota {
    const P: &str = "anthropic-ratelimit-unified-";
    let mut h: HashMap<String, String> = HashMap::new();
    for line in raw.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            if let Some(rest) = k.strip_prefix(P) {
                h.insert(rest.to_owned(), v.trim().to_owned());
            }
        }
    }
    let at = |k: &str| {
        h.get(k)
            .and_then(|v| v.parse::<i64>().ok())
            .and_then(|t| DateTime::from_timestamp(t, 0))
            .map(|t| t.to_rfc3339())
    };
    let mut windows: Vec<(String, f64, Option<String>)> = h
        .iter()
        .filter_map(|(k, v)| {
            let key = k.strip_suffix("-utilization")?;
            let name = window_name(key)?;
            Some((name, v.parse().ok()?, at(&format!("{key}-reset"))))
        })
        .collect();
    windows.sort_by(|a, b| a.0.cmp(&b.0));
    Quota {
        windows,
        rejected: h.get("status").is_some_and(|s| s == "rejected"),
        reset: at("reset"),
    }
}

fn can_query_quota(a: &Account) -> bool {
    (a.provider == "claude" && a.kind == "token") || a.provider == "codex"
}

async fn refresh_quota(st: &Shared, a: &Account) -> Result<(), String> {
    match a.provider.as_str() {
        "codex" => refresh_codex_quota(st, a).await,
        _ => refresh_claude_quota(st, a).await,
    }
}

async fn store_quota(st: &Shared, a: &Account, q: &Quota) {
    let _ = sqlx::query("DELETE FROM rate_limit_snapshots WHERE account_id = ?1")
        .bind(&a.id)
        .execute(st.db.pool())
        .await;
    for (name, util, reset) in &q.windows {
        upsert_window(st, &a.id, name, *util, reset.clone()).await;
    }
    if q.rejected && q.windows.iter().all(|w| w.1 < 1.0) {
        upsert_window(st, &a.id, "blocked", 1.0, q.reset.clone()).await;
    }
    st.emit(ServerEvent::AccountsChanged);
}

fn codex_window_name(mins: i64) -> String {
    match mins {
        300 => "five_hour".into(),
        10080 => "seven_day".into(),
        m if m % 1440 == 0 => format!("{}_day", m / 1440),
        m if m % 60 == 0 => format!("{}_hour", m / 60),
        m => format!("{m}_min"),
    }
}

#[must_use]
pub fn parse_codex_rate_limits(v: &Value) -> (Quota, Option<String>) {
    let rl = &v["rateLimits"];
    let at = |t: &Value| {
        t.as_i64()
            .and_then(|t| DateTime::from_timestamp(t, 0))
            .map(|t| t.to_rfc3339())
    };
    let mut windows: Vec<(String, f64, Option<String>)> = ["primary", "secondary"]
        .iter()
        .filter_map(|k| {
            let w = rl.get(k).filter(|w| w.is_object())?;
            Some((
                codex_window_name(w["windowDurationMins"].as_i64()?),
                w["usedPercent"].as_f64()? / 100.0,
                at(&w["resetsAt"]),
            ))
        })
        .collect();
    windows.sort_by(|a, b| a.0.cmp(&b.0));
    let rejected =
        !rl["rateLimitReachedType"].is_null() || v["ordinaryUsageAllowed"] == json!(false);
    let reset = windows.iter().filter_map(|w| w.2.clone()).max();
    let plan = rl["planType"]
        .as_str()
        .map(str::to_owned)
        .filter(|p| !p.is_empty());
    (
        Quota {
            windows,
            rejected,
            reset,
        },
        plan,
    )
}

async fn refresh_codex_quota(st: &Shared, a: &Account) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut cmd = tokio::process::Command::new("bash");
    cmd.args(["-lc", &shell("codex", "app-server")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some((k, v)) = a.env() {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| format!("起不了 codex：{e}"))?;
    let mut stdin = child.stdin.take().ok_or("codex 没有 stdin")?;
    let stdout = child.stdout.take().ok_or("codex 没有 stdout")?;
    let hello = [
        json!({ "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "blazar", "version": env!("CARGO_PKG_VERSION") } } }),
        json!({ "method": "initialized" }),
        json!({ "id": 2, "method": "account/rateLimits/read" }),
    ];
    for m in hello {
        stdin
            .write_all(format!("{m}\n").as_bytes())
            .await
            .map_err(|e| format!("写给 codex 失败：{e}"))?;
    }
    let mut lines = BufReader::new(stdout).lines();
    let answer = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && v["id"] == json!(2)
            {
                return Some(v);
            }
        }
        None
    })
    .await;
    drop(stdin);
    let _ = child.kill().await;
    let v = answer
        .map_err(|_| "codex 查额度超时".to_owned())?
        .ok_or("codex 没有返回额度")?;
    if let Some(err) = v.get("error") {
        let msg = err["message"].as_str().unwrap_or("未知错误");
        if msg.to_ascii_lowercase().contains("not logged in") || msg.contains("auth") {
            check(st, a).await;
        }
        return Err(format!("codex 查额度失败：{msg}"));
    }
    let (q, plan) = parse_codex_rate_limits(&v["result"]);
    if let Some(plan) = plan {
        let _ = sqlx::query("UPDATE accounts SET plan = ?2 WHERE id = ?1")
            .bind(&a.id)
            .bind(plan)
            .execute(st.db.pool())
            .await;
    }
    if q.windows.is_empty() {
        st.emit(ServerEvent::AccountsChanged);
        return Err("codex 没有报告额度窗口".into());
    }
    store_quota(st, a, &q).await;
    Ok(())
}

async fn claude_curl(token: &str, args: &[&str]) -> Result<(String, String, String), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args([
        "-sS",
        "--proto",
        "=https",
        "--max-time",
        "20",
        "-w",
        "\nblazar-status:%{http_code}\n",
        "-H",
        "@-",
        "-H",
        "anthropic-beta: oauth-2025-04-20",
        "-H",
        "User-Agent: blazar-accounts",
    ])
    .args(args)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("起不了 curl：{e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin
            .write_all(format!("Authorization: Bearer {token}\n").as_bytes())
            .await;
        let _ = stdin.shutdown().await;
    }
    let out = tokio::time::timeout(std::time::Duration::from_secs(25), child.wait_with_output())
        .await
        .map_err(|_| "查询额度超时".to_owned())?
        .map_err(|e| e.to_string())?;
    let raw = String::from_utf8_lossy(&out.stdout).into_owned();
    let (body, code) = raw
        .rsplit_once("blazar-status:")
        .map_or((raw.as_str(), ""), |(b, c)| (b, c.trim()));
    Ok((
        code.to_owned(),
        body.to_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

async fn mark_logged_out(st: &Shared, a: &Account) {
    let _ = sqlx::query("UPDATE accounts SET status = 'logged_out', checked_at = ?2 WHERE id = ?1")
        .bind(&a.id)
        .bind(Utc::now().to_rfc3339())
        .execute(st.db.pool())
        .await;
    st.emit(ServerEvent::AccountsChanged);
}

async fn refresh_claude_quota(st: &Shared, a: &Account) -> Result<(), String> {
    let path = a.token_file().ok_or("这个账号没有长期 token")?;
    let token = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("读不到保存的 token：{e}"))?;
    let token = token.trim();
    if let Ok((code, body, _)) =
        claude_curl(token, &["https://api.anthropic.com/api/oauth/usage"]).await
    {
        if code == "401" {
            mark_logged_out(st, a).await;
            return Err("token 无效或已被吊销".into());
        }
        if code == "200"
            && let Ok(v) = serde_json::from_str::<Value>(&body)
        {
            let q = parse_oauth_usage(&v);
            if !q.windows.is_empty() {
                store_quota(st, a, &q).await;
                return Ok(());
            }
        }
    }
    let body = json!({
        "model": QUOTA_MODEL,
        "max_tokens": 1,
        "system": "You are Claude Code, Anthropic's official CLI for Claude.",
        "messages": [{ "role": "user", "content": "hi" }],
    })
    .to_string();
    let (code, raw, err) = claude_curl(
        token,
        &[
            "-D",
            "-",
            "-o",
            "/dev/null",
            "-H",
            "anthropic-version: 2023-06-01",
            "-H",
            "content-type: application/json",
            "-d",
            &body,
            "https://api.anthropic.com/v1/messages",
        ],
    )
    .await?;
    if code == "401" {
        mark_logged_out(st, a).await;
        return Err("token 无效或已被吊销".into());
    }
    let q = parse_quota_headers(&raw);
    if q.windows.is_empty() {
        return Err(format!(
            "没拿到额度信息（HTTP {code}）{}",
            err.trim().chars().take(160).collect::<String>()
        ));
    }
    store_quota(st, a, &q).await;
    Ok(())
}

#[must_use]
pub fn parse_oauth_usage(v: &Value) -> Quota {
    let mut windows: Vec<(String, f64, Option<String>)> = v
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| k.starts_with("five_hour") || k.starts_with("seven_day"))
        .filter_map(|(k, w)| {
            let util = w.get("utilization")?.as_f64()?;
            let reset = w
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Utc).to_rfc3339());
            Some((k.clone(), (util / 100.0).clamp(0.0, 1.0), reset))
        })
        .collect();
    windows.sort_by(|a, b| a.0.cmp(&b.0));
    Quota {
        rejected: windows.iter().any(|w| w.1 >= 1.0),
        windows,
        reset: None,
    }
}

#[derive(Deserialize)]
pub struct Mode {
    pub provider: String,
    pub mode: String,
}

pub async fn set_mode(State(st): State<Shared>, Json(m): Json<Mode>) -> Response {
    if !PROVIDERS.contains(&m.provider.as_str()) {
        return fail(
            StatusCode::BAD_REQUEST,
            format!("不支持的运行时 {}", m.provider),
        );
    }
    let mode = m.mode.trim();
    if !matches!(mode, "" | "auto") {
        match get(&st, mode).await {
            Some(a) if a.provider == m.provider => {}
            _ => return fail(StatusCode::BAD_REQUEST, "没有这个账号"),
        }
    }
    if let Err(e) = crate::office::kv_put(&st, &mode_key(&m.provider), &json!(mode)).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    let global = if global_enabled(&st).await && mode != "auto" {
        Some(sync_global(&st, &m.provider, mode).await)
    } else {
        None
    };
    st.emit(ServerEvent::AccountsChanged);
    match global {
        Some(Err(e)) => {
            Json(json!({ "ok": true, "global": false, "global_error": e })).into_response()
        }
        Some(Ok(())) => Json(json!({ "ok": true, "global": true })).into_response(),
        None => Json(json!({ "ok": true })).into_response(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuChoice {
    pub mode: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuProvider {
    pub provider: &'static str,
    pub current: String,
    pub choices: Vec<MenuChoice>,
}

pub async fn menu_state(st: &Shared) -> (bool, Vec<MenuProvider>) {
    let mut providers = Vec::new();
    for p in PROVIDERS {
        let mut choices = vec![MenuChoice {
            mode: "auto".into(),
            label: "自动轮换".into(),
        }];
        for a in all(st, Some(p)).await.into_iter().filter(|a| !a.disabled) {
            let label = match a
                .email
                .as_deref()
                .filter(|e| !e.is_empty() && *e != a.label)
            {
                Some(email) => format!("{} · {email}", a.label),
                None => a.label.clone(),
            };
            choices.push(MenuChoice {
                mode: if a.builtin { String::new() } else { a.id },
                label,
            });
        }
        if !choices.iter().any(|c| c.mode.is_empty()) {
            choices.insert(
                1,
                MenuChoice {
                    mode: String::new(),
                    label: "本机登录".into(),
                },
            );
        }
        providers.push(MenuProvider {
            provider: p,
            current: mode_of(st, p).await,
            choices,
        });
    }
    (global_enabled(st).await, providers)
}

async fn response_error(r: Response) -> Result<(), String> {
    if r.status().is_success() {
        return Ok(());
    }
    let body = axum::body::to_bytes(r.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    let v: Value = serde_json::from_slice(&body).unwrap_or_default();
    Err(v["error"].as_str().map_or_else(
        || String::from_utf8_lossy(&body).into_owned(),
        str::to_owned,
    ))
}

pub async fn choose(st: &Shared, provider: &str, mode: &str) -> Result<(), String> {
    let r = set_mode(
        State(st.clone()),
        Json(Mode {
            provider: provider.to_owned(),
            mode: mode.to_owned(),
        }),
    )
    .await;
    response_error(r).await
}

pub async fn set_global(st: &Shared, enabled: bool) -> Result<(), String> {
    response_error(global_put(State(st.clone()), Json(GlobalBody { enabled })).await).await
}

const GLOBAL_KEY: &str = "accounts.global_sync";

fn global_key(provider: &str) -> String {
    format!("accounts.global.{provider}")
}

async fn global_enabled(st: &Shared) -> bool {
    crate::office::kv_get(st, GLOBAL_KEY).await.as_bool() == Some(true)
}

async fn global_current(st: &Shared, provider: &str) -> Option<Account> {
    let id = crate::office::kv_get(st, &global_key(provider))
        .await
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if id.is_empty() {
        return None;
    }
    get(st, &id).await.filter(|a| !a.builtin)
}

async fn sync_global(st: &Shared, provider: &str, mode: &str) -> Result<(), String> {
    let target = if mode.is_empty() {
        None
    } else {
        get(st, mode).await.filter(|a| !a.builtin)
    };
    let prev = global_current(st, provider).await;
    if prev.as_ref().map(|a| &a.id) == target.as_ref().map(|a| &a.id) {
        return Ok(());
    }
    let backups = root(st).join("global-backup");
    let places = super::global_login::Places::from_env();
    let p2 = provider.to_owned();
    let (prev2, target2) = (prev.clone(), target.clone());
    tokio::task::spawn_blocking(move || match p2.as_str() {
        "claude" => {
            super::global_login::switch_claude(&places, &backups, prev2.as_ref(), target2.as_ref())
        }
        _ => super::global_login::switch_codex(&places, &backups, prev2.as_ref(), target2.as_ref()),
    })
    .await
    .map_err(|e| e.to_string())??;
    let id = target.map(|a| a.id).unwrap_or_default();
    crate::office::kv_put(st, &global_key(provider), &json!(id))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(target: "blazar::accounts", provider, account = %id, "全局登录已切换");
    Ok(())
}

pub async fn global_get(State(st): State<Shared>) -> Response {
    let mut cur = serde_json::Map::new();
    for p in PROVIDERS {
        cur.insert(
            (*p).into(),
            json!(global_current(&st, p).await.map(|a| a.id)),
        );
    }
    Json(json!({
        "enabled": global_enabled(&st).await,
        "current": cur,
        "keychain": cfg!(target_os = "macos"),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct GlobalBody {
    pub enabled: bool,
}

pub async fn global_put(State(st): State<Shared>, Json(b): Json<GlobalBody>) -> Response {
    let mut errors = Vec::new();
    for p in PROVIDERS {
        let mode = if b.enabled {
            mode_of(&st, p).await
        } else {
            String::new()
        };
        if mode == "auto" {
            continue;
        }
        if let Err(e) = sync_global(&st, p, &mode).await {
            errors.push(format!("{p}：{e}"));
        }
    }
    if errors.is_empty() || !b.enabled {
        let _ = crate::office::kv_put(&st, GLOBAL_KEY, &json!(b.enabled)).await;
    }
    st.emit(ServerEvent::AccountsChanged);
    if errors.is_empty() {
        Json(json!({ "ok": true })).into_response()
    } else {
        fail(StatusCode::CONFLICT, errors.join("；"))
    }
}

pub async fn clear_model_block(
    State(st): State<Shared>,
    Path((id, model)): Path<(String, String)>,
) -> Response {
    let _ = sqlx::query("DELETE FROM account_model_blocks WHERE account_id = ?1 AND model = ?2")
        .bind(&id)
        .bind(&model)
        .execute(st.db.pool())
        .await;
    st.emit(ServerEvent::AccountsChanged);
    StatusCode::NO_CONTENT.into_response()
}

fn login_ssh_args(node: &str, remote: &str) -> Result<Vec<String>, String> {
    blazar_transport::validate_ssh_target(node).map_err(|e| e.to_string())?;
    Ok(vec![
        "-tt".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=15".into(),
        "--".into(),
        node.into(),
        remote.into(),
    ])
}

pub async fn node_login_ws(
    ws: WebSocketUpgrade,
    State(st): State<Shared>,
    Path((node, runtime)): Path<(String, String)>,
    Query(q): Query<LoginQuery>,
) -> Response {
    if let Err(error) = blazar_transport::validate_ssh_target(&node) {
        return fail(StatusCode::BAD_REQUEST, error.to_string());
    }
    if runtime != "codex" {
        return fail(
            StatusCode::BAD_REQUEST,
            "远端只需要登录 Codex；Claude 用长期 token，经凭据代理转过去",
        );
    }
    let known: Option<String> = sqlx::query_scalar("SELECT name FROM nodes WHERE name = ?1")
        .bind(&node)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
    if known.is_none() || node == "local" {
        return fail(StatusCode::NOT_FOUND, "没有这台机器");
    }
    let exports: String = crate::proxy::node_net_env(&st, &node)
        .await
        .iter()
        .map(|(k, v)| format!("export {k}='{}'\n", v.replace('\'', "")))
        .collect();
    let script = format!("{exports}{}", shell("codex", "login --device-auth"));
    let remote = format!("bash -lc '{}'", script.replace('\'', r"'\''"));
    let home = directories::BaseDirs::new()
        .map_or_else(std::env::temp_dir, |b| b.home_dir().to_path_buf());
    let args = match login_ssh_args(&node, &remote) {
        Ok(args) => args,
        Err(error) => return fail(StatusCode::BAD_REQUEST, error),
    };
    let target = blazar_terminal::TerminalTarget::Command {
        program: "ssh".into(),
        args,
        env: Vec::new(),
        cwd: home.display().to_string(),
    };
    ws.on_upgrade(move |socket| install_then_login(st, node, socket, target, q))
}

async fn install_then_login(
    st: Shared,
    node: String,
    mut socket: ws::WebSocket,
    target: blazar_terminal::TerminalTarget,
    q: LoginQuery,
) {
    if crate::remote_cli::remote_version(&st, &node, "codex")
        .await
        .is_none()
    {
        let version = crate::remote_cli::target_version("codex")
            .await
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        let _ = socket
            .send(ws::Message::Text(
                format!(
                    "{node} 上还没有 Codex，正在把官方发布的 Codex{version} 装进 ~/.blazar …\r\n"
                )
                .into(),
            ))
            .await;
        match crate::remote_cli::install(&st, &node, "codex").await {
            Ok((_, msg)) => {
                let _ = socket
                    .send(ws::Message::Text(format!("{msg}\r\n\r\n").into()))
                    .await;
            }
            Err(e) => {
                let _ = socket
                    .send(ws::Message::Text(format!("安装失败：{e}\r\n").into()))
                    .await;
                let _ = socket.close().await;
                return;
            }
        }
    }
    pty_bridge(socket, target, q).await;
}

async fn pty_bridge(socket: ws::WebSocket, target: blazar_terminal::TerminalTarget, q: LoginQuery) {
    let (mut tx, mut rx) = socket.split();
    let session = match blazar_terminal::TerminalSession::open(&target, q.cols, q.rows) {
        Ok(s) => s,
        Err(err) => {
            let _ = tx
                .send(ws::Message::Text(
                    format!("\r\n打开终端失败: {err}\r\n").into(),
                ))
                .await;
            return;
        }
    };
    let (handle, mut output) = session.split();
    let pump = tokio::spawn(async move {
        while let Some(chunk) = output.recv().await {
            if tx.send(ws::Message::Binary(chunk.into())).await.is_err() {
                break;
            }
        }
        let _ = tx.close().await;
    });
    let input = async {
        while let Some(Ok(msg)) = rx.next().await {
            match msg {
                ws::Message::Text(t) => {
                    match serde_json::from_str::<crate::api::TerminalClientMsg>(&t) {
                        Ok(crate::api::TerminalClientMsg::Input { data }) => {
                            if handle.write(data.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        Ok(crate::api::TerminalClientMsg::Resize { cols, rows }) => {
                            let _ = handle.resize(cols, rows).await;
                        }
                        Err(_) => {}
                    }
                }
                ws::Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let stop = pump.abort_handle();
    tokio::select! {
        () = input => stop.abort(),
        _ = pump => {}
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    #[serde(default = "crate::api::default_cols")]
    pub cols: u16,
    #[serde(default = "crate::api::default_rows")]
    pub rows: u16,
}

pub async fn login_ws(
    ws: WebSocketUpgrade,
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<LoginQuery>,
) -> Response {
    let Some(a) = get(&st, &id).await else {
        return fail(StatusCode::NOT_FOUND, "没有这个账号");
    };
    if a.kind == "token" {
        return fail(
            StatusCode::CONFLICT,
            "这个账号用的是长期 token，需要时更换 token 即可",
        );
    }
    let cmd = match a.provider.as_str() {
        "claude" => "auth login",
        _ => "login",
    };
    let home = directories::BaseDirs::new()
        .map_or_else(std::env::temp_dir, |b| b.home_dir().to_path_buf());
    let target = blazar_terminal::TerminalTarget::Command {
        program: "bash".into(),
        args: vec!["-lc".into(), shell(&a.provider, cmd)],
        env: a.env().into_iter().collect(),
        cwd: home.display().to_string(),
    };
    ws.on_upgrade(move |socket| async move {
        let (mut tx, mut rx) = socket.split();
        let session = match blazar_terminal::TerminalSession::open(&target, q.cols, q.rows) {
            Ok(s) => s,
            Err(err) => {
                let _ = tx
                    .send(ws::Message::Text(
                        format!("\r\n打开终端失败: {err}\r\n").into(),
                    ))
                    .await;
                return;
            }
        };
        let (handle, mut output) = session.split();
        let pump = tokio::spawn(async move {
            while let Some(chunk) = output.recv().await {
                if tx.send(ws::Message::Binary(chunk.into())).await.is_err() {
                    break;
                }
            }
            let _ = tx.close().await;
        });
        let input = async {
            while let Some(Ok(msg)) = rx.next().await {
                match msg {
                    ws::Message::Text(t) => {
                        match serde_json::from_str::<crate::api::TerminalClientMsg>(&t) {
                            Ok(crate::api::TerminalClientMsg::Input { data }) => {
                                if handle.write(data.as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            Ok(crate::api::TerminalClientMsg::Resize { cols, rows }) => {
                                let _ = handle.resize(cols, rows).await;
                            }
                            Err(_) => {}
                        }
                    }
                    ws::Message::Close(_) => break,
                    _ => {}
                }
            }
        };
        let stop = pump.abort_handle();
        tokio::select! {
            () = input => stop.abort(),
            _ = pump => {}
        }
        if let Some(a) = get(&st, &a.id).await {
            check(&st, &a).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_targets_are_validated_before_login_commands() {
        for node in ["-invalid", "", "Alice@-invalid", "host name"] {
            assert!(login_ssh_args(node, "true").is_err());
        }
        let args = login_ssh_args("hub-host", "true").unwrap();
        let index = args.iter().position(|arg| arg == "hub-host").unwrap();
        assert_eq!(args[index - 1], "--");
    }

    fn w(name: &str, util: f64, resets: Option<DateTime<Utc>>, seen: DateTime<Utc>) -> Window {
        Window {
            name: name.into(),
            utilization: util,
            resets_at: resets.map(|t| t.to_rfc3339()),
            observed_at: seen.to_rfc3339(),
        }
    }

    #[test]
    fn score_ignores_windows_that_already_reset() {
        let now = Utc::now();
        let ws = [
            w(
                "five_hour",
                0.95,
                Some(now - Duration::minutes(1)),
                now - Duration::hours(2),
            ),
            w("seven_day", 0.4, Some(now + Duration::days(3)), now),
        ];
        assert!((score(&ws, now) - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn blocked_without_reset_time_expires() {
        let now = Utc::now();
        let fresh = [w("blocked", 1.0, None, now - Duration::minutes(10))];
        let old = [w(
            "blocked",
            1.0,
            None,
            now - Duration::minutes(BLOCKED_MINUTES + 1),
        )];
        assert!((score(&fresh, now) - 1.0).abs() < f64::EPSILON);
        assert!(score(&old, now).abs() < f64::EPSILON);
    }

    #[test]
    fn best_prefers_headroom_then_fewer_runs() {
        let c = |id: &str, score: f64, running: i64| Candidate {
            id: id.into(),
            score,
            running,
        };
        let list = [c("a", 0.7, 0), c("b", 0.2, 3), c("c", 0.2, 1)];
        assert_eq!(best(&list).unwrap().id, "c");
        assert!(best(&[]).is_none());
    }

    #[test]
    fn claude_status_is_read_from_cli_json() {
        let out = r#"some warning
{"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.c", "subscriptionType": "max"}"#;
        assert_eq!(
            parse_claude_status(out),
            Probe {
                logged_in: Some(true),
                email: Some("a@b.c".into()),
                plan: Some("max".into()),
            }
        );
        let out = r#"{"loggedIn": false, "authMethod": "none"}"#;
        assert_eq!(parse_claude_status(out).logged_in, Some(false));
        assert_eq!(parse_claude_status("command not found").logged_in, None);
    }

    #[test]
    fn codex_status_is_read_from_cli_text() {
        assert_eq!(parse_codex_status("Not logged in\n").logged_in, Some(false));
        let p = parse_codex_status("Logged in using ChatGPT\n");
        assert_eq!(p.logged_in, Some(true));
        assert_eq!(p.plan, None, "具体套餐等查额度时再填");
        assert_eq!(parse_codex_status("").logged_in, None);
    }

    #[test]
    fn only_setup_tokens_are_accepted() {
        assert_eq!(
            valid_token("  sk-ant-oat01-AbC_d-9\n").unwrap(),
            "sk-ant-oat01-AbC_d-9"
        );
        assert!(
            valid_token("sk-ant-api03-xyz").is_err(),
            "API key 不是订阅 token"
        );
        assert!(valid_token("sk-ant-oat01-abc def").is_err());
        assert!(valid_token("export CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-x").is_err());
    }

    #[test]
    fn switching_midway_resumes_the_same_conversation() {
        let orig = json!({ "text": "重构登录模块", "images": [{ "media_type": "image/png", "data": "AA==" }],
                           "resume": false, "model": "claude-fable-5-1", "context_file": "src/a.rs" });
        let r = retry_request(orig.clone(), true, &["a".into()], "t1");
        assert_eq!(r["text"], CONTINUE_TEXT);
        assert_eq!(r["images"], json!([]), "图片已经在上下文里了，不再发");
        assert_eq!(r["resume"], true, "接着同一个会话，上下文不变");
        assert_eq!(r["resume_at_last"], false, "从中断处接着，不回退到上一轮");
        assert_eq!(r["resume_session"], "t1");
        assert_eq!(r["model"], "claude-fable-5-1", "模型不变");
        assert_eq!(r["account"], "auto");
        assert_eq!(r["exclude_accounts"], json!(["a"]));

        let r = retry_request(orig, false, &["a".into(), "b".into()], "t1");
        assert_eq!(r["text"], "重构登录模块", "没开始干就原样重发");
        assert_eq!(r["images"].as_array().map(Vec::len), Some(1));
        assert_eq!(r["retry_thread"], "t1");
        assert_eq!(r["exclude_accounts"], json!(["a", "b"]));
    }

    #[test]
    fn codex_rate_limits_come_from_app_server() {
        let v = json!({
            "ordinaryUsageAllowed": true,
            "rateLimits": {
                "primary": { "usedPercent": 7, "windowDurationMins": 10080, "resetsAt": 1_791_163_562 },
                "secondary": { "usedPercent": 40, "windowDurationMins": 300, "resetsAt": 1_790_900_000 },
                "planType": "prolite", "rateLimitReachedType": null
            }
        });
        let (q, plan) = parse_codex_rate_limits(&v);
        let got: Vec<(&str, f64)> = q.windows.iter().map(|w| (w.0.as_str(), w.1)).collect();
        assert_eq!(got, [("five_hour", 0.4), ("seven_day", 0.07)]);
        assert_eq!(plan.as_deref(), Some("prolite"));
        assert!(!q.rejected);

        let v = json!({ "rateLimits": { "primary": { "usedPercent": 100, "windowDurationMins": 10080 },
                                        "secondary": null, "rateLimitReachedType": "primary" } });
        assert!(parse_codex_rate_limits(&v).0.rejected);
        assert_eq!(codex_window_name(1440), "1_day");
    }

    #[test]
    fn models_compare_by_family() {
        assert_eq!(model_family("claude-fable-5-1"), "fable");
        assert_eq!(model_family("fable[1m]"), "fable");
        assert_eq!(model_family("Opus"), "opus");
        assert_eq!(model_family("claude-opus-5-5[1m]"), "opus");
        assert_eq!(model_family(""), "");
    }

    #[test]
    fn model_specific_refusals_do_not_block_the_account() {
        assert!(model_only_limit(
            "Fable 5.1 requires usage credits. Switch to another model, or manage usage credits"
        ));
        assert!(!model_only_limit(
            "This request would exceed your account's rate limit. Please try again later."
        ));
    }

    #[test]
    fn oauth_usage_keeps_every_weekly_window() {
        let v = json!({
            "five_hour": { "utilization": 4.0, "resets_at": "2026-10-04T14:00:00+00:00" },
            "seven_day": { "utilization": 14.0, "resets_at": "2026-10-09T10:00:00Z" },
            "seven_day_fable": { "utilization": 37.5, "resets_at": null },
            "seven_day_opus": null,
            "extra_usage": { "is_enabled": false, "utilization": 50.0 },
        });
        let q = parse_oauth_usage(&v);
        let got: Vec<(&str, f64)> = q.windows.iter().map(|w| (w.0.as_str(), w.1)).collect();
        assert_eq!(
            got,
            [
                ("five_hour", 0.04),
                ("seven_day", 0.14),
                ("seven_day_fable", 0.375)
            ]
        );
        assert_eq!(q.windows[1].2.as_deref(), Some("2026-10-09T10:00:00+00:00"));
        assert!(!q.rejected);
    }

    #[test]
    fn quota_is_read_from_rate_limit_headers() {
        let raw = "HTTP/2 429 \r\n\
anthropic-ratelimit-unified-5h-status: allowed\r\n\
anthropic-ratelimit-unified-5h-utilization: 0.0\r\n\
anthropic-ratelimit-unified-5h-reset: 1790857800\r\n\
anthropic-ratelimit-unified-7d-utilization: 1.0\r\n\
anthropic-ratelimit-unified-7d-reset: 1790974800\r\n\
anthropic-ratelimit-unified-7d_opus-utilization: 0.25\r\n\
anthropic-ratelimit-unified-grace-5h-utilization: 0.5\r\n\
anthropic-ratelimit-unified-overage-period-monthly-utilization: 0.3\r\n\
anthropic-ratelimit-unified-status: rejected\r\n\
anthropic-ratelimit-unified-reset: 1790974800\r\n\
\nblazar-status:429\n";
        let q = parse_quota_headers(raw);
        let names: Vec<&str> = q.windows.iter().map(|w| w.0.as_str()).collect();
        assert_eq!(names, ["five_hour", "seven_day", "seven_day_opus"]);
        assert!((q.windows[1].1 - 1.0).abs() < f64::EPSILON);
        assert_eq!(
            q.windows[0].2.as_deref(),
            DateTime::from_timestamp(1_790_857_800, 0)
                .map(|t| t.to_rfc3339())
                .as_deref()
        );
        assert!(q.rejected);
        assert!(parse_quota_headers("HTTP/2 200\r\n").windows.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(TOKEN_FILE);
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_token(&path, "sk-ant-oat01-new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sk-ant-oat01-new");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn account_dir_links_shared_config_but_not_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".claude");
        std::fs::create_dir_all(home.join("skills")).unwrap();
        std::fs::write(home.join("settings.json"), "{}").unwrap();
        std::fs::write(home.join(".credentials.json"), "secret").unwrap();
        let dir = tmp.path().join("accounts/claude/x");

        prepare_dir("claude", &dir, Some(&home)).unwrap();
        prepare_dir("claude", &dir, Some(&home)).unwrap();

        for name in ["settings.json", "skills", "projects"] {
            assert!(
                dir.join(name).symlink_metadata().unwrap().is_symlink(),
                "{name}"
            );
        }
        assert!(
            home.join("projects").is_dir(),
            "会话历史目录要先建出来，账号之间才能续接"
        );
        assert!(!dir.join(".credentials.json").exists());
        assert!(!dir.join("CLAUDE.md").exists());

        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            home.join("settings.json").exists(),
            "删账号不能删到共享的原文件"
        );
    }
}
