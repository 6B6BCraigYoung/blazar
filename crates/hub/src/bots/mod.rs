use std::collections::HashSet;
use std::sync::Mutex;

use axum::http::HeaderMap;
use blazar_core_types::{SessionId, WorkspaceId};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::api::Shared;
use crate::state::ServerEvent;

pub mod api;
pub mod claims;
pub mod dispatch;
pub mod facts;
pub mod jobs;
pub mod memory;
pub mod rules;
pub mod status;

mod briefing;
mod lifecycle;
mod mailbox;
mod threads;

#[cfg(test)]
mod tests;

pub use briefing::{briefing, hands, hands_name, home_path};
pub(crate) use lifecycle::insert;
pub use lifecycle::{
    NewBot, Patch, Spawn, archive, create, ensure_local_workspace, pause, promote, restore,
    set_status, spawn, update,
};
pub use mailbox::{
    NewMail, expire_due, mail, mailbox, mark_delivered, mark_failed, on_run_finished, pending,
    pending_counts, render, render_one, replies_to, reply, send, sender_label, sent_by, wait_reply,
};
pub use threads::{
    bind_thread, caller_thread, close_thread, main_thread, open_thread, thread,
    thread_by_session_thread, threads,
};

pub const ROLES: &[&str] = &["lead", "worker"];
pub const PRIORITIES: &[&str] = &["P0", "P1", "P2"];
pub const BRAINS: &[&str] = &["hub", "node"];
pub const MAIL_PRIORITIES: &[&str] = &["normal", "urgent"];
pub const FROM_KINDS: &[&str] = &["user", "bot", "feishu", "rule", "schedule", "system"];
pub const THREAD_KINDS: &[&str] = &["main", "task", "feishu"];
pub const CALLER_HEADER: &str = "x-blazar-caller";

#[derive(Default)]
pub struct BotsState {
    pub wake: tokio::sync::Notify,
    pub starting: Mutex<HashSet<String>>,
    pub jobs: jobs::JobsState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bot {
    pub id: String,
    pub handle: String,
    pub display_name: String,
    pub role: String,
    pub agent_id: String,
    pub home_ws_id: String,
    pub brain: String,
    pub brain_ws_id: Option<String>,
    pub priority: String,
    pub runtimes: Vec<Value>,
    pub hot_until: Option<String>,
    pub status: String,
    pub status_note: String,
    pub last_active_at: Option<String>,
    pub archived_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub lifetime: String,
    pub idle_ttl_secs: Option<i64>,
}

pub const LIFETIMES: &[&str] = &["fixed", "temporary"];
pub const DEFAULT_IDLE_TTL_SECS: i64 = 86_400;

impl Bot {
    #[must_use]
    pub fn is_temporary(&self) -> bool {
        self.lifetime == "temporary"
    }

    #[must_use]
    pub fn paused(&self) -> bool {
        self.status == "paused"
    }

    #[must_use]
    pub fn is_lead(&self) -> bool {
        self.role == "lead"
    }

    #[must_use]
    pub fn hot(&self) -> bool {
        self.hot_until
            .as_deref()
            .is_some_and(|t| t > Utc::now().to_rfc3339().as_str())
    }

    #[must_use]
    pub fn run_workspace(&self) -> &str {
        match (&self.brain[..], &self.brain_ws_id) {
            ("node", Some(ws)) => ws,
            _ => &self.home_ws_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub bot_id: String,
    pub thread_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub task_id: Option<String>,
    pub runtime: Option<String>,
    pub summary: String,
    pub closed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub to_bot: String,
    pub thread_hint: Option<String>,
    pub from_kind: String,
    pub from_ref: String,
    pub from_thread: Option<String>,
    pub reply_to: Option<String>,
    pub subject: String,
    pub body: String,
    pub attachments: Value,
    pub priority: String,
    pub status: String,
    pub session_id: Option<String>,
    pub delivered_at: Option<String>,
    pub done_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    pub bot: String,
    pub handle: String,
    pub role: String,
    #[serde(default)]
    pub session: Option<String>,
}

impl Caller {
    #[must_use]
    pub fn is_lead(&self) -> bool {
        self.role == "lead"
    }

    #[must_use]
    pub fn header_value(&self) -> String {
        json!(self).to_string()
    }
}

#[must_use]
pub fn caller_of(headers: &HeaderMap) -> Option<Caller> {
    headers
        .get(CALLER_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str(v).ok())
}

fn s(r: &SqliteRow, k: &str) -> String {
    r.try_get(k).unwrap_or_default()
}

fn opt(r: &SqliteRow, k: &str) -> Option<String> {
    r.try_get(k).ok().flatten()
}

fn bot_of(r: &SqliteRow) -> Bot {
    Bot {
        id: s(r, "id"),
        handle: s(r, "handle"),
        display_name: s(r, "display_name"),
        role: s(r, "role"),
        agent_id: s(r, "agent_id"),
        home_ws_id: s(r, "home_ws_id"),
        brain: s(r, "brain"),
        brain_ws_id: opt(r, "brain_ws_id"),
        priority: s(r, "priority"),
        runtimes: serde_json::from_str(&s(r, "runtimes")).unwrap_or_default(),
        hot_until: opt(r, "hot_until"),
        status: s(r, "status"),
        status_note: s(r, "status_note"),
        last_active_at: opt(r, "last_active_at"),
        archived_at: opt(r, "archived_at"),
        created_at: s(r, "created_at"),
        updated_at: s(r, "updated_at"),
        lifetime: {
            let l = s(r, "lifetime");
            if l.is_empty() { "fixed".into() } else { l }
        },
        idle_ttl_secs: r.try_get("idle_ttl_secs").ok().flatten(),
    }
}

fn thread_of(r: &SqliteRow) -> Thread {
    Thread {
        id: s(r, "id"),
        bot_id: s(r, "bot_id"),
        thread_id: opt(r, "thread_id"),
        kind: s(r, "kind"),
        title: s(r, "title"),
        task_id: opt(r, "task_id"),
        runtime: opt(r, "runtime"),
        summary: s(r, "summary"),
        closed_at: opt(r, "closed_at"),
        created_at: s(r, "created_at"),
        updated_at: s(r, "updated_at"),
    }
}

fn mail_of(r: &SqliteRow) -> Mail {
    Mail {
        id: s(r, "id"),
        to_bot: s(r, "to_bot"),
        thread_hint: opt(r, "thread_hint"),
        from_kind: s(r, "from_kind"),
        from_ref: s(r, "from_ref"),
        from_thread: opt(r, "from_thread"),
        reply_to: opt(r, "reply_to"),
        subject: s(r, "subject"),
        body: s(r, "body"),
        attachments: serde_json::from_str(&s(r, "attachments")).unwrap_or(json!([])),
        priority: s(r, "priority"),
        status: s(r, "status"),
        session_id: opt(r, "session_id"),
        delivered_at: opt(r, "delivered_at"),
        done_at: opt(r, "done_at"),
        expires_at: opt(r, "expires_at"),
        created_at: s(r, "created_at"),
    }
}

pub fn changed(st: &Shared) {
    st.emit(ServerEvent::BotsChanged);
    st.bots.wake.notify_waiters();
}

#[must_use]
pub fn valid_handle(h: &str) -> bool {
    blazar_core_types::api::valid_bot_handle(h)
}

#[must_use]
pub fn default_home(handle: &str) -> String {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("blazar-bots")
        .join(handle)
        .to_string_lossy()
        .into_owned()
}

pub async fn all(st: &Shared) -> Vec<Bot> {
    sqlx::query("SELECT * FROM bots WHERE archived_at IS NULL ORDER BY created_at")
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(bot_of)
        .collect()
}

pub async fn archived(st: &Shared) -> Vec<Bot> {
    sqlx::query(
        "SELECT * FROM bots WHERE archived_at IS NOT NULL ORDER BY archived_at DESC LIMIT 200",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(bot_of)
    .collect()
}

pub async fn get(st: &Shared, id_or_handle: &str) -> Option<Bot> {
    sqlx::query(
        "SELECT * FROM bots WHERE id = ?1 OR handle = ?1
         ORDER BY CASE WHEN archived_at IS NULL THEN 0 ELSE 1 END, created_at DESC LIMIT 1",
    )
    .bind(id_or_handle.trim())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|r| bot_of(&r))
}

pub async fn by_home(st: &Shared, workspace_id: &str) -> Option<Bot> {
    sqlx::query(
        "SELECT * FROM bots WHERE archived_at IS NULL AND (home_ws_id = ?1 OR brain_ws_id = ?1) LIMIT 1",
    )
    .bind(workspace_id)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|r| bot_of(&r))
}

pub async fn by_session(st: &Shared, sid: SessionId) -> Option<Bot> {
    sqlx::query(
        "SELECT b.* FROM bots b JOIN sessions s ON s.workspace_id IN (b.home_ws_id, b.brain_ws_id)
         WHERE s.id = ?1 AND b.archived_at IS NULL LIMIT 1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|r| bot_of(&r))
}

pub async fn is_hot(st: &Shared, ws: WorkspaceId) -> bool {
    by_home(st, &ws.to_string())
        .await
        .is_some_and(|b| b.hot() && !b.paused())
}
