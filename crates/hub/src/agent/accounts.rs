use std::path::{Path as FsPath, PathBuf};

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use sqlx::Row;

use crate::api::Shared;

mod failover;
mod login;
mod manage;
mod mode;
mod probe;
mod quota;
mod selection;

pub use failover::{
    CONTINUE_TEXT, model_only_limit, note_failure, note_rate_limit, on_run_finished, retry_request,
};
pub use login::{LoginQuery, login_ws, node_login_ws};
pub use manage::{
    NewAccount, NewToken, PLANS, Patch, check_all, check_one, clear_model_block, create, list,
    remove, set_token, update, valid_token,
};
pub use mode::{
    GlobalBody, MenuChoice, MenuProvider, Mode, choose, global_get, global_put, menu_state,
    set_global, set_mode,
};
pub use probe::{
    FRESH_HOURS, Probe, check_on_start, fresh, parse_claude_status, parse_codex_status, probe,
};
pub use quota::{
    Quota, parse_codex_rate_limits, parse_oauth_usage, parse_quota_headers, quota, quota_all,
};
pub use selection::{Candidate, Chosen, Window, best, model_family, resolve, score};

pub const PROVIDERS: &[&str] = &["claude", "codex"];

pub const TOKEN_FILE: &str = ".blazar-oauth-token";
const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

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

    pub linked: Option<String>,
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

fn linked_from(dir: &str) -> Option<String> {
    std::fs::read_link(PathBuf::from(dir).join(TOKEN_FILE))
        .ok()
        .map(|p| p.display().to_string())
}

fn from_row(r: &sqlx::sqlite::SqliteRow) -> Account {
    let config_dir: Option<String> = r.try_get("config_dir").ok().flatten();
    let kind = match r.try_get::<String, _>("auth_mode").as_deref() {
        Ok("token") => "token",
        _ => "login",
    };
    Account {
        linked: config_dir
            .as_deref()
            .filter(|_| kind == "token")
            .and_then(linked_from),
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
        kind: kind.to_owned(),
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

async fn mode_of(st: &Shared, provider: &str) -> String {
    crate::office::kv_get(st, &mode_key(provider))
        .await
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

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
