use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;

use super::{Account, TOKEN_ENV, all, default_id, env_key, get, mode_of};

pub const DEFAULT_THRESHOLD: f64 = 0.85;

const SHORT_WEIGHT: f64 = 0.6;

const LONG_WEIGHT: f64 = 0.4;

pub const RUN_PENALTY: f64 = 0.03;

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

fn long_window(name: &str) -> bool {
    name.starts_with("seven_day") || name.contains("week")
}

#[must_use]
pub fn peak(windows: &[Window], now: DateTime<Utc>) -> f64 {
    windows
        .iter()
        .filter(|w| w.live(now))
        .map(|w| w.utilization)
        .fold(0.0, f64::max)
}

#[must_use]
pub fn score(windows: &[Window], now: DateTime<Utc>) -> f64 {
    let top = peak(windows, now);
    if top >= 1.0 {
        return 1.0;
    }
    let max_of = |long: bool| {
        windows
            .iter()
            .filter(|w| w.live(now) && w.name != "blocked" && long_window(&w.name) == long)
            .map(|w| w.utilization)
            .fold(0.0, f64::max)
    };
    SHORT_WEIGHT * max_of(false) + LONG_WEIGHT * max_of(true)
}

#[must_use]
pub fn threshold_of(saved: &Value) -> f64 {
    saved["quota"]["threshold"]
        .as_f64()
        .filter(|t| (0.05..=1.0).contains(t))
        .unwrap_or(DEFAULT_THRESHOLD)
}

pub fn merge(cur: &mut Value, body: &Value) -> Result<(), String> {
    if let Some(t) = body["quota"].get("threshold") {
        match t.as_f64().filter(|t| (0.05..=1.0).contains(t)) {
            Some(t) => cur["quota"]["threshold"] = json!(t),
            None => return Err("额度阈值要在 5% 到 100% 之间".into()),
        }
    }
    Ok(())
}

pub async fn threshold(st: &Shared) -> f64 {
    threshold_of(&crate::office::kv_get(st, "prefs").await)
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
    pub peak: f64,
    pub running: i64,
}

impl Candidate {
    #[must_use]
    pub fn load(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let running = self.running.max(0) as f64;
        self.score + RUN_PENALTY * running
    }
}

#[must_use]
pub fn best(cands: &[Candidate]) -> Option<&Candidate> {
    cands.iter().min_by(|a, b| {
        a.load()
            .total_cmp(&b.load())
            .then(a.running.cmp(&b.running))
    })
}

#[must_use]
pub fn spread(cands: &[Candidate], threshold: f64) -> Option<&Candidate> {
    let under: Vec<Candidate> = cands
        .iter()
        .filter(|c| c.peak < threshold)
        .cloned()
        .collect();
    let id = best(&under).or_else(|| best(cands))?.id.clone();
    cands.iter().find(|c| c.id == id)
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
            "\"{}\" is a browser-login account, and its credentials can't be carried to {run_node}. For remote machines, pick a long-lived token account (claude setup-token)",
            get(st, &c.id).await.map(|a| a.label).unwrap_or(c.id)
        )),
        None => Err(format!(
            "Running Claude Code on {run_node} requires a long-lived token account"
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
            super::probe::recheck(st, true, Some(provider)).await;
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
                    peak: windows.get(&a.id).map_or(0.0, |w| peak(w, now)),
                    running: running.get(&a.id).copied().unwrap_or(0),
                    id: a.id,
                })
                .collect();
            let fit: Vec<Candidate> = cands
                .iter()
                .filter(|c| can_run(&c.id) && (exclude.is_empty() || c.score < 1.0))
                .cloned()
                .collect();
            let limit = threshold(st).await;
            let sticky = prior.and_then(|p| fit.iter().find(|c| c.id == p && c.peak < limit));
            let pick = match sticky.or_else(|| spread(&fit, limit)) {
                Some(c) => Some(c.id.clone()),
                None if !exclude.is_empty() => {
                    return Err(
                        "No other account can take over: all were tried, hit their usage limit, or can't use this model".into(),
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
                .ok_or("The bound account was deleted; pick another one on the Accounts page")?;
            if a.provider != provider {
                return Err(format!(
                    "Account \"{}\" belongs to {}, not {provider}. Pick a {provider} account or Auto-balance in the agent menu, then send again",
                    a.label, a.provider
                ));
            }
            if a.disabled {
                return Err(format!("Account \"{}\" is disabled", a.label));
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
        assert!((score(&ws, now) - LONG_WEIGHT * 0.4).abs() < 1e-9);
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
        let list = [cand("a", 0.7, 0), cand("b", 0.2, 3), cand("c", 0.2, 1)];
        assert_eq!(best(&list).unwrap().id, "c");
        assert!(best(&[]).is_none());
    }

    fn cand(id: &str, score: f64, running: i64) -> Candidate {
        Candidate {
            id: id.into(),
            score,
            peak: score,
            running,
        }
    }

    #[test]
    fn score_blends_both_windows() {
        let now = Utc::now();
        let later = Some(now + Duration::hours(3));
        let short_heavy = [
            w("five_hour", 0.8, later, now),
            w("seven_day", 0.1, later, now),
        ];
        let long_heavy = [
            w("five_hour", 0.1, later, now),
            w("seven_day", 0.8, later, now),
        ];
        let s = score(&short_heavy, now);
        let l = score(&long_heavy, now);
        assert!((s - (0.6 * 0.8 + 0.4 * 0.1)).abs() < 1e-9);
        assert!((l - (0.6 * 0.1 + 0.4 * 0.8)).abs() < 1e-9);
        assert!(s > l);
        let both = [
            w("five_hour", 0.5, later, now),
            w("seven_day", 0.5, later, now),
        ];
        let one = [
            w("five_hour", 0.5, later, now),
            w("seven_day", 0.0, later, now),
        ];
        assert!(score(&both, now) > score(&one, now));
        let full = [
            w("five_hour", 1.0, later, now),
            w("seven_day", 0.2, later, now),
        ];
        assert!((score(&full, now) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn accounts_over_the_threshold_get_no_new_sessions() {
        let mut hot = cand("hot", 0.3, 0);
        hot.peak = 0.9;
        let list = [hot, cand("cool", 0.5, 0)];
        assert_eq!(spread(&list, DEFAULT_THRESHOLD).unwrap().id, "cool");
        assert_eq!(spread(&list, 0.95).unwrap().id, "hot");
    }

    #[test]
    fn when_every_account_is_hot_the_lowest_still_runs() {
        let mut a = cand("a", 0.9, 0);
        a.peak = 0.95;
        let mut b = cand("b", 0.7, 0);
        b.peak = 0.88;
        assert_eq!(spread(&[a, b], DEFAULT_THRESHOLD).unwrap().id, "b");
        assert!(spread(&[], DEFAULT_THRESHOLD).is_none());
    }

    #[test]
    fn running_sessions_add_a_light_penalty() {
        let list = [cand("busy", 0.30, 4), cand("idle", 0.35, 0)];
        assert_eq!(best(&list).unwrap().id, "idle");
        let list = [cand("busy", 0.10, 1), cand("idle", 0.40, 0)];
        assert_eq!(best(&list).unwrap().id, "busy");
    }

    #[test]
    fn threshold_reads_from_settings() {
        assert!((threshold_of(&json!({})) - DEFAULT_THRESHOLD).abs() < f64::EPSILON);
        assert!(
            (threshold_of(&json!({ "quota": { "threshold": 0.7 } })) - 0.7).abs() < f64::EPSILON
        );
        let mut cur = json!({});
        assert!(merge(&mut cur, &json!({ "quota": { "threshold": 1.5 } })).is_err());
        merge(&mut cur, &json!({ "quota": { "threshold": 0.6 } })).unwrap();
        assert_eq!(cur["quota"]["threshold"], 0.6);
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
