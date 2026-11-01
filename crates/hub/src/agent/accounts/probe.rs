use std::path::PathBuf;

use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::api::Shared;
use crate::state::ServerEvent;

use super::quota::{can_query_quota, refresh_quota};
use super::{Account, TOKEN_ENV, default_home, get};

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

pub(super) fn shell(provider: &str, cmd: &str) -> String {
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

pub const FRESH_HOURS: i64 = 1;

#[must_use]
pub fn fresh(checked_at: Option<&str>, now: chrono::DateTime<Utc>) -> bool {
    checked_at
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| t.with_timezone(&Utc) + chrono::Duration::hours(FRESH_HOURS) > now)
}

pub(super) async fn recheck(st: &Shared, only_stale: bool, provider: Option<&str>) -> usize {
    if st.services == crate::services::Services::Isolated {
        return 0;
    }
    let now = Utc::now();
    let due: Vec<Account> = super::all(st, provider)
        .await
        .into_iter()
        .filter(|a| !a.disabled && (!only_stale || !fresh(a.checked_at.as_deref(), now)))
        .collect();
    let n = due.len();
    futures::future::join_all(due.iter().map(|a| check(st, a))).await;
    n
}

pub async fn check_on_start(st: Shared) {
    let n = recheck(&st, false, None).await;
    if n > 0 {
        tracing::info!(target: "blazar::accounts", "启动时重新检查了 {n} 个账号的登录状态");
    }
}

fn credential_files(a: &Account) -> Vec<PathBuf> {
    let home = a
        .config_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| default_home(&a.provider));
    let mut files: Vec<PathBuf> = a.token_file().into_iter().collect();
    if let Some(home) = home {
        files.push(home.join(match a.provider.as_str() {
            "claude" => ".credentials.json",
            _ => "auth.json",
        }));
    }
    files
}

fn keychain_service(a: &Account) -> Option<String> {
    if !cfg!(target_os = "macos") || a.provider != "claude" || a.kind == "token" {
        return None;
    }
    Some(match &a.config_dir {
        None => "Claude Code-credentials".to_owned(),
        Some(dir) => {
            let hash: String = Sha256::digest(dir.as_bytes())
                .iter()
                .take(4)
                .map(|b| format!("{b:02x}"))
                .collect();
            format!("Claude Code-credentials-{hash}")
        }
    })
}

#[must_use]
pub fn parse_keychain_mdat(out: &str) -> Option<DateTime<Utc>> {
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("\"mdat\""))?;
    let quoted = line.rsplit_once('=')?.1;
    let digits: String = quoted
        .split_once('"')?
        .1
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    NaiveDateTime::parse_from_str(&digits, "%Y%m%d%H%M%S")
        .ok()
        .map(|t| t.and_utc())
}

async fn credentials_changed_at(a: &Account) -> Option<DateTime<Utc>> {
    let mut latest = credential_files(a)
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok()?.modified().ok())
        .map(DateTime::<Utc>::from)
        .max();
    if let Some(service) = keychain_service(a)
        && let Ok(out) = tokio::process::Command::new("security")
            .args(["find-generic-password", "-s", &service])
            .output()
            .await
    {
        latest = latest.max(parse_keychain_mdat(&String::from_utf8_lossy(&out.stdout)));
    }
    latest
}

async fn signed_in_again(st: &Shared, a: &Account) -> bool {
    let Some(since) = a
        .checked_at
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
    else {
        return true;
    };
    if credentials_changed_at(a)
        .await
        .is_none_or(|t| t <= since.with_timezone(&Utc))
    {
        return false;
    }
    !can_query_quota(a) || refresh_quota(st, a).await.is_ok()
}

pub(super) async fn check(st: &Shared, a: &Account) -> Option<Account> {
    check_as(st, a, false).await
}

pub(super) async fn check_after_login(st: &Shared, a: &Account) -> Option<Account> {
    check_as(st, a, true).await
}

async fn check_as(st: &Shared, a: &Account, just_signed_in: bool) -> Option<Account> {
    if st.services == crate::services::Services::Isolated {
        return None;
    }
    let p = probe(a).await;
    if p.logged_in == Some(true)
        && a.status == "logged_out"
        && !just_signed_in
        && !signed_in_again(st, a).await
    {
        return get(st, &a.id).await;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_check_counts_as_fresh_for_one_hour() {
        let now = Utc::now();
        let at = |m: i64| (now - chrono::Duration::minutes(m)).to_rfc3339();
        assert!(fresh(Some(&at(50)), now));
        assert!(!fresh(Some(&at(61)), now));
        assert!(!fresh(None, now));
        assert!(!fresh(Some("2026-10-13"), now));
    }

    #[test]
    fn keychain_change_time_is_read_without_the_secret() {
        let out = r#"keychain: "/Users/u/Library/Keychains/login.keychain-db"
attributes:
    "cdat"<timedate>=0x32303236303930313130303030305A00  "20260901100000Z\000"
    "mdat"<timedate>=0x32303236313033303132333435305A00  "20261030123450Z\000"
    "svce"<blob>="Claude Code-credentials""#;
        assert_eq!(
            parse_keychain_mdat(out).map(|t| t.to_rfc3339()),
            Some("2026-10-30T12:34:50+00:00".into())
        );
        assert_eq!(parse_keychain_mdat("security: not found"), None);
    }

    #[tokio::test]
    async fn a_rewritten_token_file_counts_as_signing_in_again() {
        let dir = tempfile::tempdir().unwrap();
        let a = Account {
            id: "t".into(),
            provider: "claude".into(),
            label: "t".into(),
            config_dir: Some(dir.path().display().to_string()),
            email: None,
            plan: None,
            status: "logged_out".into(),
            disabled: false,
            checked_at: None,
            created_at: String::new(),
            builtin: false,
            kind: "token".into(),
            linked: None,
        };
        assert_eq!(credentials_changed_at(&a).await, None);
        let before = Utc::now() - chrono::Duration::seconds(5);
        std::fs::write(dir.path().join(super::super::TOKEN_FILE), "x").unwrap();
        let changed = credentials_changed_at(&a).await.unwrap();
        assert!(changed > before);
        assert!(changed < Utc::now() + chrono::Duration::seconds(5));
    }

    #[test]
    fn codex_status_is_read_from_cli_text() {
        assert_eq!(parse_codex_status("Not logged in\n").logged_in, Some(false));
        let p = parse_codex_status("Logged in using ChatGPT\n");
        assert_eq!(p.logged_in, Some(true));
        assert_eq!(p.plan, None, "具体套餐等查额度时再填");
        assert_eq!(parse_codex_status("").logged_in, None);
    }
}
