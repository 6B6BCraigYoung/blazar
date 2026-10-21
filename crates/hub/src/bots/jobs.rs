use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use axum::http::HeaderMap;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{Caller, caller_of, opt, s};
use crate::api::Shared;
use crate::state::ServerEvent;

mod announce;
mod launch;
mod watch;

pub use announce::{Wake, announce, route, summary, took};
pub use launch::{Register, Spawn, check_waits, kill, log, may_kill, register, root, spawn};
pub use watch::{missing_paths, sweep, verdict, watch};

pub const STATUSES: &[&str] = &["running", "done", "failed", "killed", "lost"];
pub const NOTIFY: &[&str] = &["wake", "inbox", "none"];
pub const SESSION_HEADER: &str = "x-blazar-session";
pub const EVERY: Duration = Duration::from_secs(30);
const SUMMARY_LINES: usize = 20;
pub const MAX_TAIL: usize = 2000;
pub const WAIT_KINDS: &[&str] = &["node", "fact", "path", "job"];

#[derive(Default)]
pub struct JobsState {
    pub root: Mutex<Option<PathBuf>>,
    pub missing: Mutex<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub owner_kind: String,
    pub bot_id: Option<String>,
    pub bot_handle: Option<String>,
    pub session_id: Option<String>,
    pub owner_workspace: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub node: String,
    pub kind: String,
    pub label: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub run_dir: Option<String>,
    pub pid: Option<i64>,
    pub log_path: Option<String>,
    pub waits_for: Value,
    pub deadline_at: Option<String>,
    pub deadline_noted_at: Option<String>,
    pub status: String,
    pub exit_code: Option<i64>,
    pub notify: String,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
}

impl Job {
    #[must_use]
    pub fn waits(&self) -> Vec<(String, String)> {
        self.waits_for
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| {
                WAIT_KINDS.iter().find_map(|k| {
                    w.get(*k)
                        .and_then(Value::as_str)
                        .map(|v| ((*k).to_owned(), v.to_owned()))
                })
            })
            .collect()
    }

    #[must_use]
    pub fn wait_paths(&self) -> Vec<String> {
        self.waits()
            .into_iter()
            .filter(|(k, _)| k == "path")
            .map(|(_, v)| v)
            .collect()
    }

    #[must_use]
    pub fn log_file(&self) -> Option<String> {
        self.log_path.clone().or_else(|| {
            self.run_dir
                .as_ref()
                .filter(|_| self.kind == "spawn")
                .map(|d| format!("{d}/log"))
        })
    }

    #[must_use]
    pub fn owner(&self) -> Owner {
        match (self.owner_kind.as_str(), &self.bot_id, &self.session_id) {
            ("bot", Some(bot), thread) => Owner::Bot {
                bot: bot.clone(),
                thread: thread.clone(),
            },
            ("session", _, Some(thread)) => Owner::Session {
                thread: thread.clone(),
                workspace: String::new(),
            },
            _ => Owner::User,
        }
    }

    #[must_use]
    pub fn owner_label(&self) -> String {
        match self.owner_kind.as_str() {
            "bot" => self
                .bot_handle
                .clone()
                .unwrap_or_else(|| "已删除的 bot".into()),
            "session" => self
                .owner_workspace
                .clone()
                .unwrap_or_else(|| "会话".into()),
            _ => "用户".into(),
        }
    }
}

fn job_of(r: &SqliteRow) -> Job {
    Job {
        id: s(r, "id"),
        owner_kind: s(r, "owner_kind"),
        bot_id: opt(r, "bot_id"),
        bot_handle: opt(r, "bot_handle"),
        session_id: opt(r, "session_id"),
        owner_workspace: opt(r, "owner_workspace"),
        workspace_id: opt(r, "workspace_id"),
        workspace_name: opt(r, "workspace_name"),
        node: s(r, "node"),
        kind: s(r, "kind"),
        label: s(r, "label"),
        command: opt(r, "command"),
        cwd: opt(r, "cwd"),
        run_dir: opt(r, "run_dir"),
        pid: r.try_get("pid").ok().flatten(),
        log_path: opt(r, "log_path"),
        waits_for: serde_json::from_str(&s(r, "waits_for")).unwrap_or_else(|_| json!([])),
        deadline_at: opt(r, "deadline_at"),
        deadline_noted_at: opt(r, "deadline_noted_at"),
        status: s(r, "status"),
        exit_code: r.try_get("exit_code").ok().flatten(),
        notify: s(r, "notify"),
        last_checked_at: opt(r, "last_checked_at"),
        last_error: opt(r, "last_error"),
        created_at: s(r, "created_at"),
        updated_at: s(r, "updated_at"),
        ended_at: opt(r, "ended_at"),
    }
}

const SELECT: &str = "SELECT j.*, n.name AS node, b.handle AS bot_handle, w.name AS workspace_name,
        (SELECT ow.name FROM sessions os JOIN workspaces ow ON ow.id = os.workspace_id
         WHERE COALESCE(os.thread_id, os.id) = j.session_id ORDER BY os.created_at DESC LIMIT 1)
        AS owner_workspace
    FROM jobs j JOIN nodes n ON n.id = j.node_id
    LEFT JOIN bots b ON b.id = j.bot_id
    LEFT JOIN workspaces w ON w.id = j.workspace_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Bot { bot: String, thread: Option<String> },
    Session { thread: String, workspace: String },
    User,
}

impl Owner {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bot { .. } => "bot",
            Self::Session { .. } => "session",
            Self::User => "user",
        }
    }

    #[must_use]
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bot { bot: a, .. }, Self::Bot { bot: b, .. }) => a == b,
            (Self::Session { thread: a, .. }, Self::Session { thread: b, .. }) => a == b,
            (Self::User, Self::User) => true,
            _ => false,
        }
    }
}

async fn session_thread(st: &Shared, sid: Option<&str>) -> Option<(String, String)> {
    sqlx::query_as("SELECT COALESCE(thread_id, id), workspace_id FROM sessions WHERE id = ?1")
        .bind(sid?)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
}

pub async fn owner_of(st: &Shared, headers: &HeaderMap) -> Owner {
    if let Some(c) = caller_of(headers) {
        let thread = session_thread(st, c.session.as_deref())
            .await
            .map(|(t, _)| t);
        return Owner::Bot { bot: c.bot, thread };
    }
    let sid = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok());
    match session_thread(st, sid).await {
        Some((thread, workspace)) => Owner::Session { thread, workspace },
        None => Owner::User,
    }
}

#[must_use]
pub fn visible_to(job: &Job, by: Option<&Caller>) -> bool {
    match by {
        Some(c) if !c.is_lead() => job.bot_id.as_deref() == Some(c.bot.as_str()),
        _ => true,
    }
}

pub fn changed(st: &Shared) {
    st.emit(ServerEvent::JobsChanged);
}

pub async fn get(st: &Shared, id: &str) -> Option<Job> {
    sqlx::query(&format!("{SELECT} WHERE j.id = ?1"))
        .bind(id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| job_of(&r))
}

#[derive(Debug, Default, Deserialize)]
pub struct Filter {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub bot: Option<String>,
}

fn nonempty(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|v| !v.is_empty())
}

pub async fn list(st: &Shared, f: &Filter, by: Option<&Caller>) -> Result<Vec<Job>, String> {
    let status = nonempty(f.status.as_deref());
    if let Some(s) = status
        && !STATUSES.contains(&s)
    {
        return Err(format!("未知状态 {s}，可用 {}", STATUSES.join(" / ")));
    }
    let bot = match nonempty(f.bot.as_deref()) {
        Some(b) => Some(super::get(st, b).await.ok_or("没有这个 bot")?.id),
        None => None,
    };
    let only = by.filter(|c| !c.is_lead()).map(|c| c.bot.clone());
    let rows = sqlx::query(&format!(
        "{SELECT} WHERE (?1 IS NULL OR j.status = ?1) AND (?2 IS NULL OR j.workspace_id = ?2)
           AND (?3 IS NULL OR j.bot_id = ?3) AND (?4 IS NULL OR j.bot_id = ?4)
         ORDER BY j.status != 'running', j.created_at DESC LIMIT 200"
    ))
    .bind(status)
    .bind(nonempty(f.workspace_id.as_deref()))
    .bind(bot)
    .bind(only)
    .fetch_all(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows.iter().map(job_of).collect())
}

pub async fn running_of_bot(st: &Shared, bot_id: &str) -> Vec<Job> {
    recent(st, Some(bot_id), 20).await
}

async fn recent(st: &Shared, bot_id: Option<&str>, limit: i64) -> Vec<Job> {
    let since = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    sqlx::query(&format!(
        "{SELECT} WHERE (?1 IS NULL OR j.bot_id = ?1)
           AND (j.status = 'running' OR j.ended_at > ?2)
         ORDER BY j.status != 'running', j.created_at DESC LIMIT ?3"
    ))
    .bind(bot_id)
    .bind(since)
    .bind(limit)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(job_of)
    .collect()
}

#[must_use]
pub fn brief(j: &Job) -> Value {
    json!({
        "id": j.id,
        "label": j.label,
        "node": j.node,
        "status": j.status,
        "owner": j.owner_label(),
        "workspace_id": j.workspace_id,
        "started_at": j.created_at,
        "ended_at": j.ended_at,
        "deadline_at": j.deadline_at,
        "waits_for": j.waits_for,
        "exit_code": j.exit_code,
        "last_error": j.last_error,
    })
}

pub async fn fleet_brief(st: &Shared) -> Vec<Value> {
    recent(st, None, 50).await.iter().map(brief).collect()
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
