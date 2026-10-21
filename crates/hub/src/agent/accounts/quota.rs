use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::api::Shared;
use crate::state::ServerEvent;

use super::failover::upsert_window;
use super::probe::{check, shell};
use super::{Account, all, fail, get};

const QUOTA_MODEL: &str = "claude-haiku-4-5-20251001";

pub async fn quota_all(State(st): State<Shared>) -> Response {
    let due: Vec<Account> = all(&st, None)
        .await
        .into_iter()
        .filter(|a| can_query_quota(a) && !a.disabled && a.status != "logged_out")
        .collect();
    let results = futures::future::join_all(due.iter().map(|a| refresh_quota(&st, a))).await;
    let failed: Vec<Value> = due
        .iter()
        .zip(&results)
        .filter_map(|(a, r)| {
            r.as_ref()
                .err()
                .map(|e| json!({ "id": a.id, "label": a.label, "error": e }))
        })
        .collect();
    Json(json!({
        "refreshed": results.iter().filter(|r| r.is_ok()).count(),
        "total": due.len(),
        "failed": failed,
    }))
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

pub(super) fn can_query_quota(a: &Account) -> bool {
    (a.provider == "claude" && a.kind == "token") || a.provider == "codex"
}

pub(super) async fn refresh_quota(st: &Shared, a: &Account) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
