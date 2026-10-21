use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use chrono::Utc;
use serde_json::Value;

use crate::api::Shared;
use crate::state::ServerEvent;

use super::{Account, TOKEN_ENV, get};

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

pub(super) async fn check(st: &Shared, a: &Account) -> Option<Account> {
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
    fn codex_status_is_read_from_cli_text() {
        assert_eq!(parse_codex_status("Not logged in\n").logged_in, Some(false));
        let p = parse_codex_status("Logged in using ChatGPT\n");
        assert_eq!(p.logged_in, Some(true));
        assert_eq!(p.plan, None, "具体套餐等查额度时再填");
        assert_eq!(parse_codex_status("").logged_in, None);
    }
}
