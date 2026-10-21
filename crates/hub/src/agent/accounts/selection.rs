use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::Row;

use crate::api::Shared;

use super::{Account, TOKEN_ENV, all, default_id, env_key, get, mode_of};

const SWITCH_AT: f64 = 0.9;

const BLOCKED_MINUTES: i64 = 60;

const STALE_HOURS: i64 = 5;

#[derive(Debug, Clone, Serialize)]
pub struct Window {
    pub name: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
    pub observed_at: String,
}

impl Window {
    pub(super) fn live(&self, now: DateTime<Utc>) -> bool {
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

pub(super) async fn windows_of(st: &Shared) -> HashMap<String, Vec<Window>> {
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

pub(super) async fn running_of(st: &Shared) -> HashMap<String, i64> {
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

pub(super) async fn model_blocks(st: &Shared) -> HashMap<String, Vec<(String, String)>> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn models_compare_by_family() {
        assert_eq!(model_family("claude-fable-5-1"), "fable");
        assert_eq!(model_family("fable[1m]"), "fable");
        assert_eq!(model_family("Opus"), "opus");
        assert_eq!(model_family("claude-opus-5-5[1m]"), "opus");
        assert_eq!(model_family(""), "");
    }
}
