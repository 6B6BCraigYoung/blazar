use std::path::PathBuf;
use std::time::Duration;

use aes_gcm::aead::generic_array::typenum::U16;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::aes::Aes256;
use aes_gcm::{AesGcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use reqwest::header::CONTENT_TYPE;
use serde_json::{Value, json};

use crate::api::Shared;
use crate::services::Services;

use super::Account;
use super::quota::{Quota, mark_logged_out, store_quota};

const TIMEOUT: Duration = Duration::from_secs(20);

const DEVIN_HOST: &str = "https://server.codeium.com";

const FACTORY_HOST: &str = "https://api.factory.ai";

const FACTORY_KEYCHAIN: [&str; 2] = ["Factory CLI", "auth-encryption-key-security-cli"];

type Window = (String, f64, Option<String>);

fn home() -> Result<PathBuf, String> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or_else(|| "找不到用户目录".to_owned())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

async fn fetch(req: reqwest::RequestBuilder, who: &str) -> Result<(u16, Value), String> {
    let r = req
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("连不上 {who}：{}", e.without_url()))?;
    let code = r.status().as_u16();
    let body = r
        .bytes()
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or_default();
    Ok((code, body))
}

fn time_of(v: &Value) -> Option<DateTime<Utc>> {
    if let Some(t) = v
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
    {
        return Some(t.with_timezone(&Utc));
    }
    let n = v
        .as_i64()
        .or_else(|| v.as_str()?.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)?;
    if n > 1_000_000_000_000 {
        DateTime::from_timestamp_millis(n)
    } else {
        DateTime::from_timestamp(n, 0)
    }
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

async fn settle(st: &Shared, a: &Account, q: &Quota, plan: Option<String>, email: Option<String>) {
    let _ = sqlx::query(
        "UPDATE accounts SET status = 'ok', plan = COALESCE(?2, plan), email = COALESCE(?3, email),
                checked_at = ?4 WHERE id = ?1",
    )
    .bind(&a.id)
    .bind(plan)
    .bind(email)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    store_quota(st, a, q).await;
}

#[must_use]
pub fn parse_devin_credentials(text: &str) -> Option<(String, String)> {
    let get = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key)
                .then(|| v.trim().trim_matches('"').to_owned())
                .filter(|v| !v.is_empty())
        })
    };
    let host = get("api_server_url")
        .filter(|u| u.starts_with("https://"))
        .unwrap_or_else(|| DEVIN_HOST.to_owned());
    Some((
        get("windsurf_api_key")?,
        host.trim_end_matches('/').to_owned(),
    ))
}

#[derive(Debug, Default, PartialEq)]
pub struct Seat {
    pub quota: Quota,
    pub plan: Option<String>,
    pub email: Option<String>,
}

#[must_use]
pub fn parse_devin_status(v: &Value) -> Seat {
    let user = &v["userStatus"];
    let plan = &user["planStatus"];
    let info = &plan["planInfo"];
    let window = |name: &str, left: &str, reset: &str, hidden: &str| -> Option<Window> {
        if info[hidden] == json!(true) {
            return None;
        }
        let reset = time_of(&plan[reset]);
        let left = plan[left].as_f64().or(reset.map(|_| 0.0))?;
        Some((
            name.to_owned(),
            (1.0 - left / 100.0).clamp(0.0, 1.0),
            reset.map(|t| t.to_rfc3339()),
        ))
    };
    let windows: Vec<Window> = [
        window(
            "one_day",
            "dailyQuotaRemainingPercent",
            "dailyQuotaResetAtUnix",
            "hideDailyQuota",
        ),
        window(
            "seven_day",
            "weeklyQuotaRemainingPercent",
            "weeklyQuotaResetAtUnix",
            "hideWeeklyQuota",
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    Seat {
        quota: Quota {
            rejected: windows.iter().any(|w| w.1 >= 1.0),
            windows,
            reset: None,
        },
        plan: text(&info["planName"]).map(|p| p.to_ascii_lowercase()),
        email: text(&user["email"]),
    }
}

async fn devin_status() -> Result<(u16, Value), String> {
    let path = home()?.join(".local/share/devin/credentials.toml");
    let saved = tokio::fs::read_to_string(&path)
        .await
        .map_err(|_| "Devin 还没有登录，在终端里运行 devin auth login".to_owned())?;
    let (key, host) = parse_devin_credentials(&saved).ok_or("Devin 的登录凭据里没有可用的令牌")?;
    let version = env!("CARGO_PKG_VERSION");
    let body = json!({ "metadata": {
        "apiKey": key, "ideName": "devin", "ideVersion": version,
        "extensionName": "devin", "extensionVersion": version, "locale": "en",
    } });
    let req = client()?
        .post(format!(
            "{host}/exa.seat_management_pb.SeatManagementService/GetUserStatus"
        ))
        .header(CONTENT_TYPE, "application/json")
        .header("Connect-Protocol-Version", "1")
        .body(body.to_string());
    fetch(req, "Devin").await
}

pub(super) async fn refresh_devin(st: &Shared, a: &Account) -> Result<(), String> {
    if st.services == Services::Isolated {
        return Err("这个实例不联网查额度".into());
    }
    let (code, v) = devin_status().await?;
    if code == 401 {
        mark_logged_out(st, a).await;
        return Err("Devin 的登录已失效，在终端里运行 devin auth login".into());
    }
    if code != 200 {
        return Err(format!("Devin 查额度失败（HTTP {code}）"));
    }
    let seat = parse_devin_status(&v);
    settle(st, a, &seat.quota, seat.plan, seat.email).await;
    if seat.quota.windows.is_empty() {
        return Err("Devin 没有报告额度窗口".into());
    }
    Ok(())
}

#[must_use]
pub fn open_droid_login(sealed: &str, key: &str) -> Option<String> {
    let mut parts = sealed.trim().split(':').map(|p| STANDARD.decode(p).ok());
    let (iv, tag, mut data) = (parts.next()??, parts.next()??, parts.next()??);
    let key = STANDARD.decode(key.trim()).ok()?;
    if iv.len() != 16 {
        return None;
    }
    data.extend_from_slice(&tag);
    let plain = AesGcm::<Aes256, U16>::new_from_slice(&key)
        .ok()?
        .decrypt(Nonce::<U16>::from_slice(&iv), data.as_slice())
        .ok()?;
    text(&serde_json::from_slice::<Value>(&plain).ok()?["access_token"])
}

async fn droid_token() -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Err("目前只能在 macOS 上读取 Factory Droid 的登录".into());
    }
    let path = home()?.join(".factory/auth.v2.loginkeychain");
    let sealed = tokio::fs::read_to_string(&path)
        .await
        .map_err(|_| "Factory Droid 还没有登录，在终端里运行 droid 完成登录".to_owned())?;
    let [service, account] = FACTORY_KEYCHAIN;
    let asked = tokio::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(TIMEOUT, asked)
        .await
        .map_err(|_| "钥匙串没有在时限内放行 Factory Droid 的登录密钥".to_owned())?
        .map_err(|e| format!("读不了钥匙串：{e}"))?;
    if !out.status.success() {
        return Err("钥匙串里没有 Factory Droid 的登录密钥".into());
    }
    open_droid_login(&sealed, &String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "解不开 Factory Droid 的登录凭据".to_owned())
}

#[must_use]
pub fn parse_droid_limits(v: &Value, now: DateTime<Utc>) -> Option<Quota> {
    if v["usesTokenRateLimitsBilling"] != json!(true) {
        return None;
    }
    let pool = v["limits"].get("standard")?;
    let windows: Vec<Window> = [
        ("five_hour", "fiveHour"),
        ("seven_day", "weekly"),
        ("monthly", "monthly"),
    ]
    .iter()
    .filter_map(|(name, key)| {
        let w = pool.get(key)?;
        let used = w["usedPercent"].as_f64()?;
        let left = w["secondsRemaining"]
            .as_f64()
            .filter(|s| *s > 0.0)
            .and_then(|s| Duration::try_from_secs_f64(s).ok())
            .and_then(|d| chrono::Duration::from_std(d).ok());
        let reset = left
            .map(|d| now + d)
            .or_else(|| time_of(&w["windowEnd"]).filter(|t| *t > now));
        Some((
            (*name).to_owned(),
            (used / 100.0).clamp(0.0, 1.0),
            reset.map(|t| t.to_rfc3339()),
        ))
    })
    .collect();
    (!windows.is_empty()).then(|| Quota {
        rejected: windows.iter().any(|w| w.1 >= 1.0),
        windows,
        reset: None,
    })
}

#[must_use]
pub fn parse_droid_usage(v: &Value) -> Quota {
    let usage = &v["usage"];
    let reset = time_of(&usage["endDate"]).map(|t| t.to_rfc3339());
    let windows: Vec<Window> = [("monthly", "standard"), ("monthly_premium", "premium")]
        .iter()
        .filter_map(|(name, key)| {
            let pool = usage.get(key)?;
            let total = pool["totalAllowance"].as_f64().filter(|t| *t > 0.0)?;
            let used = pool["usedRatio"]
                .as_f64()
                .or_else(|| Some(pool["orgTotalTokensUsed"].as_f64()? / total))?;
            Some(((*name).to_owned(), used.clamp(0.0, 1.0), reset.clone()))
        })
        .collect();
    Quota {
        rejected: windows.iter().any(|w| w.1 >= 1.0),
        windows,
        reset: None,
    }
}

#[must_use]
pub fn parse_droid_plan(me: &Value) -> Option<String> {
    let sub = &me["organization"]["subscription"];
    text(&sub["orbSubscription"]["plan"]["name"])
        .or_else(|| text(&sub["factoryTier"]))
        .map(|p| p.to_ascii_lowercase())
}

async fn droid_quota() -> Result<(Quota, Option<String>), String> {
    let token = droid_token().await?;
    let client = client()?;
    let get = |url: String| {
        let req = client
            .get(url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .header("x-factory-client", "web-app")
            .header("Origin", "https://app.factory.ai")
            .header("Referer", "https://app.factory.ai/");
        fetch(req, "Factory")
    };
    let (code, me) = get(format!("{FACTORY_HOST}/api/app/auth/me")).await?;
    if code == 401 || code == 403 {
        return Err("Factory Droid 的登录令牌过期了，在终端里运行一次 droid 就会续上".into());
    }
    if code != 200 {
        return Err(format!("Factory 查额度失败（HTTP {code}）"));
    }
    let plan = parse_droid_plan(&me);
    let (code, limits) = get(format!("{FACTORY_HOST}/api/billing/limits")).await?;
    let quota = match Some(&limits)
        .filter(|_| code == 200)
        .and_then(|v| parse_droid_limits(v, Utc::now()))
    {
        Some(q) => q,
        None => {
            let user = me["userProfile"]["id"].as_str().unwrap_or_default();
            let url = reqwest::Url::parse_with_params(
                &format!("{FACTORY_HOST}/api/organization/subscription/usage"),
                [("useCache", "true"), ("userId", user)],
            )
            .map_err(|e| e.to_string())?;
            let (code, usage) = get(url.to_string()).await?;
            if code != 200 {
                return Err(format!("Factory 查额度失败（HTTP {code}）"));
            }
            parse_droid_usage(&usage)
        }
    };
    Ok((quota, plan))
}

pub(super) async fn refresh_droid(st: &Shared, a: &Account) -> Result<(), String> {
    if st.services == Services::Isolated {
        return Err("这个实例不联网查额度".into());
    }
    let (quota, plan) = droid_quota().await?;
    settle(st, a, &quota, plan, None).await;
    if quota.windows.is_empty() {
        return Err("Factory 这个账号没有可显示的额度".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devin_credentials_give_the_token_and_an_https_host() {
        let saved = "windsurf_api_key = \"devin-session-token$abc\"\napi_server_url = \"https://server.example.com/\"\n";
        assert_eq!(
            parse_devin_credentials(saved),
            Some((
                "devin-session-token$abc".into(),
                "https://server.example.com".into()
            ))
        );
        let plain = "windsurf_api_key = \"k\"\napi_server_url = \"http://evil.example.com\"\n";
        assert_eq!(
            parse_devin_credentials(plain).map(|c| c.1),
            Some(DEVIN_HOST.to_owned()),
            "令牌不发给非 https 的地址"
        );
        assert_eq!(parse_devin_credentials("devin_api_url = \"x\"\n"), None);
    }

    #[test]
    fn devin_quota_is_read_as_used_share() {
        let v = json!({ "userStatus": { "email": "a@b.c", "planStatus": {
            "planInfo": { "planName": "Max", "hideDailyQuota": true },
            "dailyQuotaRemainingPercent": 100,
            "weeklyQuotaRemainingPercent": 99,
            "dailyQuotaResetAtUnix": "1794729600",
            "weeklyQuotaResetAtUnix": "1794729600",
        } } });
        let seat = parse_devin_status(&v);
        assert_eq!(seat.plan.as_deref(), Some("max"));
        assert_eq!(seat.email.as_deref(), Some("a@b.c"));
        assert_eq!(
            seat.quota.windows.len(),
            1,
            "Devin 自己隐藏的每日窗口不显示"
        );
        let (name, used, reset) = &seat.quota.windows[0];
        assert_eq!(name, "seven_day");
        assert!((used - 0.01).abs() < 1e-9);
        assert_eq!(
            reset.as_deref(),
            DateTime::from_timestamp(1_794_729_600, 0)
                .map(|t| t.to_rfc3339())
                .as_deref()
        );
        assert!(!seat.quota.rejected);
    }

    #[test]
    fn a_devin_window_without_a_percentage_is_used_up() {
        let v = json!({ "userStatus": { "planStatus": {
            "planInfo": {},
            "dailyQuotaRemainingPercent": 40,
            "dailyQuotaResetAtUnix": "1794729600",
            "weeklyQuotaResetAtUnix": "1794729600",
        } } });
        let q = parse_devin_status(&v).quota;
        assert_eq!(q.windows[0].0, "one_day");
        assert!((q.windows[0].1 - 0.6).abs() < 1e-9);
        assert!((q.windows[1].1 - 1.0).abs() < f64::EPSILON);
        assert!(q.rejected);
        assert!(parse_devin_status(&json!({})).quota.windows.is_empty());
    }

    #[test]
    fn droid_login_opens_with_the_keychain_key() {
        let key = [7u8; 32];
        let iv = [9u8; 16];
        let mut sealed = AesGcm::<Aes256, U16>::new_from_slice(&key)
            .unwrap()
            .encrypt(
                Nonce::<U16>::from_slice(&iv),
                br#"{"access_token":"tok","refresh_token":"r"}"#.as_slice(),
            )
            .unwrap();
        let tag = sealed.split_off(sealed.len() - 16);
        let file = format!(
            "{}:{}:{}",
            STANDARD.encode(iv),
            STANDARD.encode(tag),
            STANDARD.encode(sealed)
        );
        let good = STANDARD.encode(key);
        assert_eq!(
            open_droid_login(&file, &format!("{good}\n")).as_deref(),
            Some("tok")
        );
        assert_eq!(open_droid_login(&file, &STANDARD.encode([8u8; 32])), None);
        assert_eq!(open_droid_login("not:sealed", &good), None);
    }

    #[test]
    fn droid_rate_limits_become_three_windows() {
        let now = Utc::now();
        let v = json!({ "usesTokenRateLimitsBilling": true, "limits": { "standard": {
            "fiveHour": { "usedPercent": 42.0, "secondsRemaining": 3600 },
            "weekly": { "usedPercent": 100, "windowEnd": (now + chrono::Duration::days(2)).timestamp_millis() },
            "monthly": { "usedPercent": 5 },
        } } });
        let q = parse_droid_limits(&v, now).unwrap();
        let names: Vec<&str> = q.windows.iter().map(|w| w.0.as_str()).collect();
        assert_eq!(names, ["five_hour", "seven_day", "monthly"]);
        assert!((q.windows[0].1 - 0.42).abs() < 1e-9);
        assert_eq!(
            q.windows[0].2.as_deref(),
            Some((now + chrono::Duration::hours(1)).to_rfc3339().as_str())
        );
        assert!(q.windows[1].2.is_some());
        assert_eq!(q.windows[2].2, None);
        assert!(q.rejected);
        let off = json!({ "usesTokenRateLimitsBilling": false, "hasActiveSubscription": false });
        assert_eq!(parse_droid_limits(&off, now), None);
    }

    #[test]
    fn droid_token_allowance_is_the_fallback() {
        let v = json!({ "usage": { "startDate": 1_794_640_578_511_i64, "endDate": null,
            "standard": { "orgTotalTokensUsed": 5_000_000, "totalAllowance": 20_000_000, "usedRatio": 0.25 },
            "premium": { "totalAllowance": 0, "usedRatio": 0 },
        } });
        let q = parse_droid_usage(&v);
        assert_eq!(q.windows, vec![("monthly".to_owned(), 0.25, None)]);
        assert!(parse_droid_usage(&json!({})).windows.is_empty());
        let me = json!({ "organization": { "subscription": { "factoryTier": "team", "orbSubscription": null } } });
        assert_eq!(parse_droid_plan(&me).as_deref(), Some("team"));
    }
}
