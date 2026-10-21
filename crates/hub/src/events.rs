use std::sync::{Arc, OnceLock};

use blazar_core_types::api::{EventFrame, ServerEvent};
use blazar_db::Db;
use chrono::{DateTime, SecondsFormat, Utc};
use tokio::sync::{broadcast, mpsc};

pub const KEEP_SECS: i64 = 24 * 3600;
pub const KEEP_ROWS: i64 = 5000;

#[derive(Default)]
pub struct EventLog {
    tx: OnceLock<mpsc::UnboundedSender<ServerEvent>>,
    boot: Arc<OnceLock<i64>>,
}

#[derive(Debug)]
pub enum Replay {
    Events(Vec<EventFrame>),
    Gap,
}

impl EventLog {
    pub fn start(&self, db: Db, bus: broadcast::Sender<EventFrame>) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        if self.tx.set(tx).is_err() {
            return;
        }
        let boot = self.boot.clone();
        rt.spawn(async move {
            match record(&db, &ServerEvent::WorkspacesChanged).await {
                Ok(id) => {
                    let _ = boot.set(id);
                }
                Err(error) => {
                    tracing::warn!("hub_events 写不进启动标记，重连一律全量刷新: {error:#}")
                }
            }
            while let Some(event) = rx.recv().await {
                let event_id = match record(&db, &event).await {
                    Ok(id) => Some(id),
                    Err(error) => {
                        tracing::warn!("事件写入 hub_events 失败，只做广播: {error:#}");
                        None
                    }
                };
                let _ = bus.send(EventFrame { event_id, event });
            }
        });
    }

    pub fn boot(&self) -> Option<i64> {
        self.boot.get().copied()
    }

    pub fn publish(&self, event: ServerEvent) -> Result<(), ServerEvent> {
        match self.tx.get() {
            Some(tx) => tx.send(event).map_err(|e| e.0),
            None => Err(event),
        }
    }
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

async fn record(db: &Db, event: &ServerEvent) -> anyhow::Result<i64> {
    let payload = serde_json::to_value(event)?;
    let kind = payload["kind"].as_str().unwrap_or_default().to_owned();
    let id = sqlx::query_scalar(
        "INSERT INTO hub_events (kind, payload, created_at) VALUES (?1, ?2, ?3) RETURNING id",
    )
    .bind(kind)
    .bind(payload.to_string())
    .bind(stamp(Utc::now()))
    .fetch_one(db.pool())
    .await?;
    Ok(id)
}

pub async fn head(db: &Db) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'hub_events'), 0)",
    )
    .fetch_one(db.pool())
    .await?)
}

pub async fn replay(db: &Db, since: i64, boot: Option<i64>) -> anyhow::Result<Replay> {
    let (oldest, head): (Option<i64>, i64) = sqlx::query_as(
        "SELECT (SELECT MIN(id) FROM hub_events),
                COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'hub_events'), 0)",
    )
    .fetch_one(db.pool())
    .await?;
    if boot.is_none_or(|b| since < b)
        || since > head
        || since.saturating_add(1) < oldest.unwrap_or(head.saturating_add(1))
    {
        return Ok(Replay::Gap);
    }
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, payload FROM hub_events WHERE id > ?1 ORDER BY id")
            .bind(since)
            .fetch_all(db.pool())
            .await?;
    let last: std::collections::HashMap<&str, i64> =
        rows.iter().map(|(id, p)| (p.as_str(), *id)).collect();
    let mut frames = Vec::with_capacity(last.len());
    for (id, payload) in &rows {
        if last.get(payload.as_str()) != Some(id) {
            continue;
        }
        let id = *id;
        match serde_json::from_str(payload) {
            Ok(event) => frames.push(EventFrame {
                event_id: Some(id),
                event,
            }),
            Err(error) => tracing::warn!("hub_events #{id} 读不回来，跳过: {error}"),
        }
    }
    Ok(Replay::Events(frames))
}

pub async fn prune(db: &Db, now: DateTime<Utc>, keep_secs: i64, keep_rows: i64) -> u64 {
    let cutoff = stamp(now - chrono::Duration::seconds(keep_secs));
    match sqlx::query(
        "DELETE FROM hub_events WHERE created_at < ?1
            OR id <= (SELECT id FROM hub_events ORDER BY id DESC LIMIT 1 OFFSET ?2)",
    )
    .bind(cutoff)
    .bind(keep_rows)
    .execute(db.pool())
    .await
    {
        Ok(done) => done.rows_affected(),
        Err(error) => {
            tracing::warn!("清理 hub_events 失败: {error}");
            0
        }
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
