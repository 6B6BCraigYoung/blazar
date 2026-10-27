use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::api::Shared;
use crate::state::ServerEvent;

use super::probe::{check, shell};
use super::quota::{can_query_quota, refresh_quota};
use super::selection::{model_blocks, running_of, score, windows_of};
use super::{
    Account, PROVIDERS, TOKEN_FILE, all, default_home, fail, get, mode_key, mode_of, prepare_dir,
    root,
};

const QUOTA_FRESH_MINUTES: i64 = 5;

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
        v["stale"] = json!(!super::probe::fresh(a.checked_at.as_deref(), now));
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
    #[serde(default)]
    pub token_file: Option<String>,
}

fn valid_token_file(s: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(s.trim());
    if !path.is_absolute() || path.file_name().and_then(|n| n.to_str()) != Some(TOKEN_FILE) {
        return Err(format!("只能引用别的 Blazar 账号目录里的 {TOKEN_FILE}"));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读不了要引用的 token：{e}"))?;
    valid_token(&text)
        .map_err(|_| "要引用的文件里不是 claude setup-token 生成的 token".to_owned())?;
    Ok(path)
}

fn link_token(dir: &FsPath, target: &FsPath) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, dir.join(TOKEN_FILE))
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, target);
        Err(std::io::Error::other("这个平台不支持引用别处的 token"))
    }
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
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(std::io::Error::other(
            "这个账号引用的是别处的 token，要换就去源头那个 hub 改",
        ));
    }
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
    let link = match req.token_file.as_deref().filter(|t| !t.trim().is_empty()) {
        None => None,
        Some(_) if req.provider != "claude" => {
            return fail(StatusCode::BAD_REQUEST, "只有 Claude Code 支持长期 token");
        }
        Some(_) if token.is_some() => {
            return fail(StatusCode::BAD_REQUEST, "token 和 token_file 只能给一个");
        }
        Some(p) => match valid_token_file(p) {
            Ok(p) => Some(p),
            Err(e) => return fail(StatusCode::BAD_REQUEST, e),
        },
    };
    let id = uuid::Uuid::now_v7().to_string();
    let dir = root(&st).join(&req.provider).join(&id);
    let shared = st.services.local(|| default_home(&req.provider)).flatten();
    let (provider, d) = (req.provider.clone(), dir.clone());
    let tok = token.clone();
    let target = link.clone();
    let made = tokio::task::spawn_blocking(move || {
        prepare_dir(&provider, &d, shared.as_deref())?;
        match (tok, target) {
            (Some(t), _) => write_token(&d.join(TOKEN_FILE), &t),
            (None, Some(p)) => link_token(&d, &p),
            (None, None) => Ok(()),
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
    .bind(if token.is_some() || link.is_some() {
        "token"
    } else {
        "cli"
    })
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
    let a = if token.is_some() || link.is_some() {
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
    if a.linked.is_some() {
        return fail(
            StatusCode::CONFLICT,
            "这个账号引用的是别处的 token，要换就去源头那个 hub 改",
        );
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
        return fail(StatusCode::CONFLICT, "默认账号不能删除，可以停用");
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(unix)]
    #[test]
    fn a_linked_token_is_read_through_but_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("host-account");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join(TOKEN_FILE), "sk-ant-oat01-host\n").unwrap();
        let mine = tmp.path().join("mine");
        std::fs::create_dir_all(&mine).unwrap();
        let target = valid_token_file(&source.join(TOKEN_FILE).display().to_string()).unwrap();
        link_token(&mine, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(mine.join(TOKEN_FILE))
                .unwrap()
                .trim(),
            "sk-ant-oat01-host"
        );
        assert_eq!(
            super::super::linked_from(&mine.display().to_string()).as_deref(),
            Some(target.display().to_string().as_str())
        );
        assert!(write_token(&mine.join(TOKEN_FILE), "sk-ant-oat01-new").is_err());
        assert_eq!(
            std::fs::read_to_string(source.join(TOKEN_FILE)).unwrap(),
            "sk-ant-oat01-host\n"
        );
        assert!(valid_token_file("relative/.blazar-oauth-token").is_err());
        assert!(valid_token_file(&tmp.path().join("other.txt").display().to_string()).is_err());
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
}
