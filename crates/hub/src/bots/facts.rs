use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{Caller, opt, s};
use crate::api::Shared;
use crate::state::ServerEvent;

pub const KINDS: &[&str] = &["run", "gpu", "node", "dataset", "custom"];
const HISTORY_KEEP: i64 = 200;
const REPORT_TOKEN_KEY: &str = "facts.report_token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub key: String,
    pub kind: String,
    pub owner_bot: Option<String>,
    pub owner_handle: Option<String>,
    pub value: Value,
    pub status: String,
    pub ttl_secs: Option<i64>,
    pub updated_at: String,
    pub created_at: String,
}

#[must_use]
pub fn valid_key(k: &str) -> bool {
    (1..=200).contains(&k.len())
        && !k.starts_with('/')
        && !k.ends_with('/')
        && !k.contains("//")
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b':' | b'-'))
}

fn fact_of(r: &SqliteRow) -> Fact {
    Fact {
        key: s(r, "key"),
        kind: s(r, "kind"),
        owner_bot: opt(r, "owner_bot"),
        owner_handle: opt(r, "owner_handle"),
        value: serde_json::from_str(&s(r, "value")).unwrap_or(Value::Null),
        status: s(r, "status"),
        ttl_secs: r.try_get("ttl_secs").ok().flatten(),
        updated_at: s(r, "updated_at"),
        created_at: s(r, "created_at"),
    }
}

const SELECT: &str =
    "SELECT f.*, b.handle AS owner_handle FROM facts f LEFT JOIN bots b ON b.id = f.owner_bot";

pub async fn get(st: &Shared, key: &str) -> Option<Fact> {
    sqlx::query(&format!("{SELECT} WHERE f.key = ?1"))
        .bind(key)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| fact_of(&r))
}

pub async fn list(st: &Shared, prefix: &str, limit: i64) -> Vec<Fact> {
    sqlx::query(&format!(
        "{SELECT} WHERE f.key LIKE ?1 ESCAPE '\\' ORDER BY f.key LIMIT ?2"
    ))
    .bind(format!(
        "{}%",
        prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    ))
    .bind(limit.clamp(1, 2000))
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(fact_of)
    .collect()
}

#[derive(Debug, Default, Deserialize)]
pub struct Put {
    #[serde(default)]
    pub key: Option<String>,
    pub value: Value,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

pub async fn put(st: &Shared, key: &str, p: Put, by: Option<&Caller>) -> Result<Fact, String> {
    if !valid_key(key) {
        return Err("Fact keys look like run/q01 or gpu/gpu-1/0, up to 200 characters".into());
    }
    if p.value.to_string().len() > 64 * 1024 {
        return Err("Fact values are limited to 64KB".into());
    }
    let existing = get(st, key).await;
    let ttl = match p.ttl_secs {
        Some(t) if t <= 0 => None,
        Some(t) => Some(t.min(30 * 86_400)),
        None => existing.as_ref().and_then(|f| f.ttl_secs),
    };
    let kind = p
        .kind
        .or_else(|| existing.as_ref().map(|f| f.kind.clone()))
        .unwrap_or_else(|| {
            key.split('/')
                .next()
                .filter(|k| KINDS.contains(k))
                .unwrap_or("custom")
                .to_owned()
        });
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!("Unknown kind {kind}; use {}", KINDS.join(" / ")));
    }
    let owner = match (&existing, by) {
        (Some(f), Some(c)) => {
            if let Some(o) = &f.owner_bot
                && o != &c.bot
            {
                return Err(format!(
                    "{key} is owned by bot:{}; don't change it directly, mail it instead",
                    f.owner_handle.as_deref().unwrap_or("?")
                ));
            }
            Some(c.bot.clone())
        }
        (Some(f), None) => f.owner_bot.clone(),
        (None, Some(c)) => Some(c.bot.clone()),
        (None, None) => None,
    };
    let now = Utc::now().to_rfc3339();
    let value = p.value.to_string();
    sqlx::query(
        "INSERT INTO facts (key, kind, owner_bot, value, status, ttl_secs, updated_at, created_at)
         VALUES (?1, ?2, ?3, ?4, 'ok', ?5, ?6, ?6)
         ON CONFLICT (key) DO UPDATE SET kind = excluded.kind, owner_bot = excluded.owner_bot,
            value = excluded.value, status = 'ok', ttl_secs = excluded.ttl_secs, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(&kind)
    .bind(&owner)
    .bind(&value)
    .bind(ttl)
    .bind(&now)
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    if existing.as_ref().is_none_or(|f| f.value != p.value) {
        let _ = sqlx::query("INSERT INTO fact_history (key, value, at) VALUES (?1, ?2, ?3)")
            .bind(key)
            .bind(&value)
            .bind(&now)
            .execute(st.db.pool())
            .await;
        let _ = sqlx::query(
            "DELETE FROM fact_history WHERE key = ?1 AND at NOT IN
             (SELECT at FROM fact_history WHERE key = ?1 ORDER BY at DESC LIMIT ?2)",
        )
        .bind(key)
        .bind(HISTORY_KEEP)
        .execute(st.db.pool())
        .await;
    }
    st.emit(ServerEvent::FactsChanged);
    get(st, key)
        .await
        .ok_or_else(|| "Couldn't read the fact back".into())
}

pub async fn remove(st: &Shared, key: &str, by: Option<&Caller>) -> Result<(), String> {
    let f = get(st, key).await.ok_or("No such fact")?;
    if let (Some(c), Some(o)) = (by, &f.owner_bot)
        && o != &c.bot
        && !c.is_lead()
    {
        return Err(format!(
            "{key} is owned by bot:{}",
            f.owner_handle.as_deref().unwrap_or("?")
        ));
    }
    sqlx::query("DELETE FROM facts WHERE key = ?1")
        .bind(key)
        .execute(st.db.pool())
        .await
        .map_err(|e| e.to_string())?;
    st.emit(ServerEvent::FactsChanged);
    Ok(())
}

pub async fn history(st: &Shared, key: &str, limit: i64) -> Vec<Value> {
    sqlx::query("SELECT value, at FROM fact_history WHERE key = ?1 ORDER BY at DESC LIMIT ?2")
        .bind(key)
        .bind(limit.clamp(1, HISTORY_KEEP))
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| {
            json!({
                "value": serde_json::from_str::<Value>(&s(r, "value")).unwrap_or(Value::Null),
                "at": s(r, "at"),
            })
        })
        .collect()
}

#[must_use]
pub fn staleness(ttl_secs: i64, age_secs: i64) -> &'static str {
    if ttl_secs <= 0 {
        "ok"
    } else if age_secs > ttl_secs * 5 {
        "lost"
    } else if age_secs > ttl_secs * 2 {
        "stale"
    } else {
        "ok"
    }
}

pub async fn sweep(st: &Shared, notify: bool) {
    let now = Utc::now();
    let rows = sqlx::query(&format!("{SELECT} WHERE f.ttl_secs IS NOT NULL"))
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
    let mut changed = false;
    for r in &rows {
        let f = fact_of(r);
        let Some(ttl) = f.ttl_secs else {
            continue;
        };
        let age = chrono::DateTime::parse_from_rfc3339(&f.updated_at)
            .map(|t| (now - t.with_timezone(&Utc)).num_seconds())
            .unwrap_or(0);
        let next = staleness(ttl, age);
        if next == f.status {
            continue;
        }
        let _ = sqlx::query("UPDATE facts SET status = ?2 WHERE key = ?1")
            .bind(&f.key)
            .bind(next)
            .execute(st.db.pool())
            .await;
        changed = true;
        if next == "lost"
            && notify
            && let Some(owner) = &f.owner_bot
        {
            let _ = super::send(
                    st,
                    owner,
                    super::NewMail {
                        body: format!(
                            "Your fact {} hasn't been updated for {} seconds, more than 5x its heartbeat interval, so it was marked lost. Check whether the underlying task is still running, then update or delete the fact.",
                            f.key, age
                        ),
                        subject: Some(format!("Fact {} lost", f.key)),
                        from_kind: "rule".into(),
                        from_ref: "fact_stale".into(),
                        ..super::NewMail::default()
                    },
                )
                .await;
        }
    }
    if changed {
        st.emit(ServerEvent::FactsChanged);
    }
}

pub async fn report_token(st: &Shared, rotate: bool) -> Result<String, String> {
    let saved = crate::office::kv_get(st, REPORT_TOKEN_KEY).await;
    if let Some(t) = saved.as_str().filter(|t| !t.is_empty() && !rotate) {
        return Ok(t.to_owned());
    }
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|e| format!("生成令牌失败：{e}"))?;
    let token = format!(
        "bzf_{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    crate::office::kv_put(st, REPORT_TOKEN_KEY, &json!(token)).await?;
    Ok(token)
}

pub async fn report_token_matches(st: &Shared, candidate: &str) -> bool {
    let saved = crate::office::kv_get(st, REPORT_TOKEN_KEY).await;
    let Some(t) = saved.as_str().filter(|t| !t.is_empty()) else {
        return false;
    };
    t.len() == candidate.len()
        && t.bytes()
            .zip(candidate.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_path_like() {
        assert!(valid_key("run/q01"));
        assert!(valid_key("gpu/gpu-1/0"));
        assert!(valid_key("node:robot-1.online"));
        assert!(!valid_key("/run"));
        assert!(!valid_key("run/"));
        assert!(!valid_key("a//b"));
        assert!(!valid_key("has space"));
        assert!(!valid_key(""));
    }

    #[test]
    fn staleness_follows_the_two_and_five_multiples() {
        assert_eq!(staleness(60, 30), "ok");
        assert_eq!(staleness(60, 120), "ok");
        assert_eq!(staleness(60, 121), "stale");
        assert_eq!(staleness(60, 300), "stale");
        assert_eq!(staleness(60, 301), "lost");
        assert_eq!(staleness(0, 999), "ok");
    }
}
