use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;

#[derive(Deserialize, Default)]
pub struct RangeQuery {
    #[serde(default)]
    pub days: Option<i64>,

    #[serde(default)]
    pub tz: Option<i64>,
}

pub async fn overview(State(st): State<Shared>, Query(q): Query<RangeQuery>) -> Response {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let tz = q.tz.unwrap_or(0).clamp(-14 * 60, 14 * 60);
    let shift = format!("{tz:+} minutes");
    let since = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let pool = st.db.pool();

    let daily = sqlx::query(
        "SELECT substr(datetime(created_at, ?2), 1, 10) AS day, COUNT(*) AS runs,
                SUM(status = 'done') AS done, SUM(status = 'failed') AS failed, SUM(status = 'interrupted') AS interrupted
         FROM sessions WHERE created_at >= ?1 GROUP BY day ORDER BY day",
    )
    .bind(&since)
    .bind(&shift)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let cost_daily: Vec<(String, f64)> = sqlx::query_as(
        "SELECT substr(datetime(ts, ?2), 1, 10) AS day, COALESCE(SUM(json_extract(payload, '$.cost_usd')), 0.0)
         FROM events WHERE ts >= ?1 AND payload LIKE '{\"type\":\"token_usage\"%' GROUP BY day",
    )
    .bind(&since)
    .bind(&shift)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let cost_of = |day: &str| {
        cost_daily
            .iter()
            .find(|(d, _)| d == day)
            .map_or(0.0, |(_, c)| *c)
    };
    let daily: Vec<Value> = daily
        .iter()
        .map(|r| {
            let day: String = r.try_get("day").unwrap_or_default();
            let n = |k: &str| r.try_get::<i64, _>(k).unwrap_or(0);
            json!({ "day": day, "runs": n("runs"), "done": n("done"), "failed": n("failed"),
                    "interrupted": n("interrupted"), "cost": cost_of(&day) })
        })
        .collect();

    let board = sqlx::query(
        "SELECT COALESCE(a.name, s.runtime_kind) AS who, a.avatar, s.runtime_kind AS runtime, a.id AS agent_id,
                COUNT(*) AS runs, SUM(s.status = 'done') AS done, SUM(s.status = 'failed') AS failed,
                COALESCE(SUM((SELECT SUM(json_extract(e.payload, '$.cost_usd')) FROM events e
                              WHERE e.session_id = s.id AND e.payload LIKE '{\"type\":\"token_usage\"%')), 0.0) AS cost,
                AVG((SELECT (julianday(MAX(e.ts)) - julianday(MIN(e.ts))) * 86400.0 FROM events e WHERE e.session_id = s.id)) AS avg_secs
         FROM sessions s LEFT JOIN agents a ON a.id = s.agent_profile
         WHERE s.created_at >= ?1 GROUP BY who, s.runtime_kind ORDER BY runs DESC LIMIT 20",
    )
    .bind(&since)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let board: Vec<Value> = board
        .iter()
        .map(|r| {
            let s = |k: &str| r.try_get::<Option<String>, _>(k).ok().flatten();
            json!({ "who": s("who"), "avatar": s("avatar"), "runtime": s("runtime"), "agent_id": s("agent_id"),
                    "runs": r.try_get::<i64, _>("runs").unwrap_or(0), "done": r.try_get::<i64, _>("done").unwrap_or(0),
                    "failed": r.try_get::<i64, _>("failed").unwrap_or(0), "cost": r.try_get::<f64, _>("cost").unwrap_or(0.0),
                    "avg_secs": r.try_get::<Option<f64>, _>("avg_secs").ok().flatten() })
        })
        .collect();

    let reasons: Vec<(String, i64)> = sqlx::query_as(
        "SELECT substr(COALESCE(json_extract(e.payload, '$.message'), '（没有留下原因）'), 1, 90) AS why, COUNT(*) AS n
         FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE s.created_at >= ?1 AND e.payload LIKE '{\"type\":\"finished\"%' AND json_extract(e.payload, '$.status') = 'failed'
         GROUP BY why ORDER BY n DESC LIMIT 8",
    )
    .bind(&since)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let heat: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT CAST(strftime('%w', datetime(created_at, ?2)) AS INTEGER), CAST(strftime('%H', datetime(created_at, ?2)) AS INTEGER), COUNT(*)
         FROM sessions WHERE created_at >= ?1 GROUP BY 1, 2",
    )
    .bind(&since)
    .bind(&shift)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let total_runs: i64 = daily.iter().map(|d| d["runs"].as_i64().unwrap_or(0)).sum();
    let done: i64 = daily.iter().map(|d| d["done"].as_i64().unwrap_or(0)).sum();
    let failed: i64 = daily
        .iter()
        .map(|d| d["failed"].as_i64().unwrap_or(0))
        .sum();
    let cost: f64 = daily
        .iter()
        .map(|d| d["cost"].as_f64().unwrap_or(0.0))
        .sum();
    Json(json!({
        "days": days,
        "totals": { "runs": total_runs, "done": done, "failed": failed, "cost": cost,
                    "success_rate": if done + failed > 0 { Some(done as f64 / (done + failed) as f64) } else { None } },
        "daily": daily, "board": board,
        "reasons": reasons.iter().map(|(w, n)| json!({ "why": w, "n": n })).collect::<Vec<_>>(),
        "heat": heat.iter().map(|(w, h, n)| json!([w, h, n])).collect::<Vec<_>>(),
    }))
    .into_response()
}
