use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use axum::http::HeaderMap;
use blazar_core_types::{SessionId, WorkspaceId};
use blazar_transport::detached::{DetachedRun, ProbeItem, RunMode, RunState};
use blazar_transport::{ExecSpec, shell_quote};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{Caller, caller_of, opt, s};
use crate::api::Shared;
use crate::state::ServerEvent;

pub const STATUSES: &[&str] = &["running", "done", "failed", "killed", "lost"];
pub const NOTIFY: &[&str] = &["wake", "inbox", "none"];
pub const SESSION_HEADER: &str = "x-blazar-session";
pub const EVERY: Duration = Duration::from_secs(30);
const PROBE_TIMEOUT: Duration = Duration::from_secs(25);
const SUMMARY_LINES: usize = 20;
pub const MAX_TAIL: usize = 2000;
const MAX_COMMAND: usize = 8000;
const MAX_DEADLINE_MINS: i64 = 30 * 24 * 60;

#[derive(Default)]
pub struct JobsState {
    pub root: Mutex<Option<PathBuf>>,
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
        "exit_code": j.exit_code,
        "last_error": j.last_error,
    })
}

pub async fn fleet_brief(st: &Shared) -> Vec<Value> {
    recent(st, None, 50).await.iter().map(brief).collect()
}

fn check_notify(n: Option<&str>) -> Result<String, String> {
    let n = nonempty(n).unwrap_or("wake");
    if NOTIFY.contains(&n) {
        Ok(n.to_owned())
    } else {
        Err(format!("notify 只能是 {}", NOTIFY.join(" / ")))
    }
}

fn deadline(mins: Option<i64>) -> Result<Option<String>, String> {
    match mins {
        None => Ok(None),
        Some(m) if (1..=MAX_DEADLINE_MINS).contains(&m) => Ok(Some(
            (Utc::now() + chrono::Duration::minutes(m)).to_rfc3339(),
        )),
        Some(_) => Err(format!("deadline_mins 要在 1 到 {MAX_DEADLINE_MINS} 之间")),
    }
}

pub async fn root(st: &Shared, node: &str) -> Result<PathBuf, String> {
    if let Some(r) = st
        .bots
        .jobs
        .root
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return Ok(r);
    }
    let out = st
        .transport(node)
        .exec(
            ExecSpec::new("bash")
                .arg("-lc")
                .arg("printf '__BLAZAR_JOBS_ROOT__ %s\\n' \"$HOME/.blazar/jobs\""),
        )
        .await
        .map_err(|e| format!("连不上 {node}：{e}"))?;
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix("__BLAZAR_JOBS_ROOT__ "))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| format!("无法确定 {node} 上的任务目录"))
}

async fn node_id(st: &Shared, node: &str) -> Result<String, String> {
    sqlx::query_scalar("SELECT id FROM nodes WHERE name = ?1")
        .bind(node)
        .fetch_optional(st.db.pool())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("没有这台机器 {node}"))
}

struct NewJob {
    id: String,
    owner: Owner,
    workspace_id: Option<String>,
    node_id: String,
    kind: &'static str,
    label: String,
    command: Option<String>,
    cwd: Option<String>,
    run_dir: String,
    pid: Option<i64>,
    log_path: Option<String>,
    deadline_at: Option<String>,
    notify: String,
}

async fn insert(st: &Shared, j: NewJob) -> Result<(), String> {
    let (bot, thread) = match &j.owner {
        Owner::Bot { bot, thread } => (Some(bot.clone()), thread.clone()),
        Owner::Session { thread, .. } => (None, Some(thread.clone())),
        Owner::User => (None, None),
    };
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO jobs (id, owner_kind, bot_id, session_id, workspace_id, node_id, kind, label,
                           command, cwd, run_dir, pid, log_path, deadline_at, notify,
                           created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
    )
    .bind(&j.id)
    .bind(j.owner.kind())
    .bind(bot)
    .bind(thread)
    .bind(&j.workspace_id)
    .bind(&j.node_id)
    .bind(j.kind)
    .bind(&j.label)
    .bind(&j.command)
    .bind(&j.cwd)
    .bind(&j.run_dir)
    .bind(j.pid)
    .bind(&j.log_path)
    .bind(&j.deadline_at)
    .bind(&j.notify)
    .bind(now)
    .execute(st.db.pool())
    .await
    .map(drop)
    .map_err(|e| e.to_string())
}

#[derive(Debug, Default, Deserialize)]
pub struct Spawn {
    pub workspace_id: String,
    pub command: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub deadline_mins: Option<i64>,
    #[serde(default)]
    pub notify: Option<String>,
}

fn label_of(label: Option<&str>, fallback: &str) -> String {
    nonempty(label)
        .unwrap_or_else(|| fallback.lines().next().unwrap_or_default().trim())
        .chars()
        .take(120)
        .collect()
}

pub async fn spawn(st: &Shared, req: Spawn, owner: &Owner) -> Result<Job, String> {
    let command = req.command.trim();
    if command.is_empty() {
        return Err("command 不能为空".into());
    }
    if command.chars().count() > MAX_COMMAND {
        return Err(format!("command 最多 {MAX_COMMAND} 字"));
    }
    let notify = check_notify(req.notify.as_deref())?;
    let deadline_at = deadline(req.deadline_mins)?;
    let (node, path) = crate::api::locate(st, req.workspace_id.trim())
        .await
        .map_err(|e| e.to_string())?;
    let node_id = node_id(st, &node).await?;
    let cwd = match nonempty(req.cwd.as_deref()) {
        Some(c) if c.starts_with('/') || c.starts_with('~') => c.to_owned(),
        Some(c) => format!("{}/{c}", path.trim_end_matches('/')),
        None => path,
    };
    let id = uuid::Uuid::now_v7().to_string();
    let root = root(st, &node).await?;
    let log = root.join(&id).join("log").display().to_string();
    let script = format!(
        "exec >>{} 2>&1\ncd {} || exit 96\n{command}\n",
        shell_quote(&log),
        crate::scripts::cd_target(&cwd)
    );
    let spec = ExecSpec::new("bash").arg("-lc").arg(script);
    let run = DetachedRun::launch_in(st.transport(&node), &root, &id, &spec, None, RunMode::Null)
        .await
        .map_err(|e| format!("在 {node} 上启动失败：{e}"))?;
    let saved = insert(
        st,
        NewJob {
            id: id.clone(),
            owner: owner.clone(),
            workspace_id: Some(req.workspace_id.trim().to_owned()),
            node_id,
            kind: "spawn",
            label: label_of(req.label.as_deref(), command),
            command: Some(command.to_owned()),
            cwd: Some(cwd),
            run_dir: run.dir.clone(),
            pid: None,
            log_path: Some(log),
            deadline_at,
            notify,
        },
    )
    .await;
    if let Err(e) = saved {
        let _ = run.hard_kill().await;
        return Err(format!("登记失败，已停掉刚起的进程：{e}"));
    }
    changed(st);
    get(st, &id).await.ok_or_else(|| "任务读不回来".into())
}

#[derive(Debug, Default, Deserialize)]
pub struct Register {
    pub node: String,
    pub pid: i64,
    pub label: String,
    #[serde(default)]
    pub log_path: Option<String>,
    #[serde(default)]
    pub deadline_mins: Option<i64>,
    #[serde(default)]
    pub notify: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

pub async fn register(st: &Shared, req: Register, owner: &Owner) -> Result<Job, String> {
    let label = req.label.trim();
    if label.is_empty() {
        return Err("label 不能为空".into());
    }
    let pid = u32::try_from(req.pid)
        .ok()
        .filter(|p| *p > 1)
        .ok_or("pid 无效")?;
    let notify = check_notify(req.notify.as_deref())?;
    let deadline_at = deadline(req.deadline_mins)?;
    let log_path = nonempty(req.log_path.as_deref()).map(str::to_owned);
    if log_path.as_deref().is_some_and(|p| !p.starts_with('/')) {
        return Err("日志路径要用绝对路径".into());
    }
    let node = req.node.trim();
    let node_id = node_id(st, node).await?;
    let workspace_id = match nonempty(req.workspace_id.as_deref()) {
        Some(ws) => {
            crate::api::locate(st, ws)
                .await
                .map_err(|e| e.to_string())?;
            Some(ws.to_owned())
        }
        None => match owner {
            Owner::Session { workspace, .. } if !workspace.is_empty() => Some(workspace.clone()),
            _ => None,
        },
    };
    let id = uuid::Uuid::now_v7().to_string();
    let root = root(st, node).await?;
    let run = DetachedRun::adopt(st.transport(node), &root, &id, pid)
        .await
        .map_err(|e| format!("在 {node} 上登记失败：{e}"))?;
    insert(
        st,
        NewJob {
            id: id.clone(),
            owner: owner.clone(),
            workspace_id,
            node_id,
            kind: "register",
            label: label.chars().take(120).collect(),
            command: None,
            cwd: None,
            run_dir: run.dir,
            pid: Some(i64::from(pid)),
            log_path,
            deadline_at,
            notify,
        },
    )
    .await?;
    changed(st);
    get(st, &id).await.ok_or_else(|| "任务读不回来".into())
}

async fn finish(
    st: &Shared,
    id: &str,
    status: &str,
    code: Option<i32>,
    exited_at: Option<i64>,
) -> bool {
    let now = Utc::now();
    let ended = exited_at
        .and_then(|t| DateTime::from_timestamp(t, 0))
        .filter(|t| *t <= now)
        .unwrap_or(now);
    sqlx::query(
        "UPDATE jobs SET status = ?2, exit_code = ?3, ended_at = ?4, updated_at = ?5,
                         last_checked_at = ?5, last_error = NULL
         WHERE id = ?1 AND status = 'running'",
    )
    .bind(id)
    .bind(status)
    .bind(code)
    .bind(ended.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(st.db.pool())
    .await
    .is_ok_and(|r| r.rows_affected() == 1)
}

pub async fn log(st: &Shared, job: &Job, lines: usize) -> Result<String, String> {
    let Some(file) = job.log_file() else {
        return Ok(String::new());
    };
    let out = st
        .transport(&job.node)
        .exec(
            ExecSpec::new("tail")
                .arg("-n")
                .arg(lines.clamp(1, MAX_TAIL).to_string())
                .arg("--")
                .arg(file),
        )
        .await
        .map_err(|e| format!("连不上 {}：{e}", job.node))?;
    if out.code != 0 && out.stdout.is_empty() {
        return Err(format!("读不到日志：{}", out.stderr.trim()));
    }
    Ok(out.stdout)
}

pub async fn kill(
    st: &Shared,
    id: &str,
    by: Option<&Caller>,
    killer: &Owner,
) -> Result<Job, String> {
    let job = get(st, id).await.ok_or("没有这个后台任务")?;
    if !visible_to(&job, by) {
        return Err("worker 只能结束自己发起的后台任务".into());
    }
    if job.status != "running" {
        return Err(format!("这个任务已经是 {} 状态", job.status));
    }
    let dir = job.run_dir.clone().ok_or("这个任务没有任务目录")?;
    let run = DetachedRun::attach(st.transport(&job.node), dir, job.id.clone());
    let stopped = if job.kind == "register" {
        run.stop_adopted().await
    } else {
        run.hard_kill().await
    };
    stopped.map_err(|e| format!("结束失败：{e}"))?;
    if finish(st, &job.id, "killed", None, None).await {
        changed(st);
        if let Some(done) = get(st, &job.id).await
            && !done.owner().same(killer)
        {
            let tail = log(st, &done, SUMMARY_LINES).await.unwrap_or_default();
            announce(st, &done, &tail).await;
        }
    }
    get(st, id).await.ok_or_else(|| "任务读不回来".into())
}

pub async fn watch(st: Shared) {
    loop {
        sweep(&st).await;
        tokio::time::sleep(EVERY).await;
    }
}

pub async fn sweep(st: &Shared) {
    let running: Vec<Job> = sqlx::query(&format!("{SELECT} WHERE j.status = 'running'"))
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(job_of)
        .collect();
    let mut by_node: BTreeMap<String, Vec<Job>> = BTreeMap::new();
    for j in running.into_iter().filter(|j| j.run_dir.is_some()) {
        by_node.entry(j.node.clone()).or_default().push(j);
    }
    let checks = by_node
        .into_iter()
        .map(|(node, jobs)| check_node(st, node, jobs));
    let touched = futures::future::join_all(checks).await;
    if touched.into_iter().any(|t| t) {
        changed(st);
    }
}

#[must_use]
pub fn verdict(job: &Job, state: &RunState) -> Option<(&'static str, Option<i32>)> {
    match state {
        RunState::Alive { .. } => None,
        RunState::Exited { code: 0, .. } => Some(("done", Some(0))),
        RunState::Exited { code, .. } => Some(("failed", Some(*code))),
        RunState::Lost { .. } if job.kind == "register" => Some(("done", None)),
        RunState::Lost { .. } | RunState::Missing => Some(("lost", None)),
    }
}

async fn check_node(st: &Shared, node: String, jobs: Vec<Job>) -> bool {
    let items: Vec<ProbeItem> = jobs
        .iter()
        .map(|j| ProbeItem {
            dir: j.run_dir.clone().unwrap_or_default(),
            tail_of: j.log_file(),
            tail_lines: SUMMARY_LINES,
        })
        .collect();
    let transport = st.transport(&node);
    let now = Utc::now().to_rfc3339();
    let probed = match tokio::time::timeout(
        PROBE_TIMEOUT,
        blazar_transport::detached::probe_many(&transport, &items),
    )
    .await
    {
        Ok(Ok(states)) => states,
        Ok(Err(e)) => return note_error(st, &jobs, &format!("连不上 {node}：{e}")).await,
        Err(_) => {
            return note_error(
                st,
                &jobs,
                &format!("{node} {} 秒内没有回应", PROBE_TIMEOUT.as_secs()),
            )
            .await;
        }
    };
    let mut touched = false;
    for (job, probed) in jobs.iter().zip(probed) {
        match verdict(job, &probed.state) {
            None => {
                let _ = sqlx::query(
                    "UPDATE jobs SET last_checked_at = ?2, last_error = NULL WHERE id = ?1",
                )
                .bind(&job.id)
                .bind(&now)
                .execute(st.db.pool())
                .await;
                touched |= job.last_error.is_some();
                touched |= past_deadline(st, job).await;
            }
            Some((status, code)) => {
                if finish(st, &job.id, status, code, probed.exited_at).await {
                    touched = true;
                    if let Some(done) = get(st, &job.id).await {
                        announce(st, &done, &probed.tail).await;
                    }
                }
            }
        }
    }
    touched
}

async fn note_error(st: &Shared, jobs: &[Job], error: &str) -> bool {
    let mut touched = false;
    for j in jobs {
        touched |= j.last_error.as_deref() != Some(error);
        let _ = sqlx::query("UPDATE jobs SET last_error = ?2 WHERE id = ?1 AND status = 'running'")
            .bind(&j.id)
            .bind(error)
            .execute(st.db.pool())
            .await;
    }
    touched
}

async fn past_deadline(st: &Shared, job: &Job) -> bool {
    let Some(at) = job.deadline_at.as_deref() else {
        return false;
    };
    let now = Utc::now();
    if job.deadline_noted_at.is_some() || at > now.to_rfc3339().as_str() {
        return false;
    }
    let first = sqlx::query(
        "UPDATE jobs SET deadline_noted_at = ?2, updated_at = ?2
         WHERE id = ?1 AND deadline_noted_at IS NULL",
    )
    .bind(&job.id)
    .bind(now.to_rfc3339())
    .execute(st.db.pool())
    .await
    .is_ok_and(|r| r.rows_affected() == 1);
    if first {
        crate::inbox::push(
            st,
            crate::inbox::Item {
                kind: "job",
                title: format!("后台任务「{}」过了截止时间还在跑", job.label),
                body: format!(
                    "机器 {}，发起方 {}，已经跑了 {}。要结束就到任务页的「后台任务」里点「结束」。",
                    job.node,
                    job.owner_label(),
                    took(&job.created_at, None)
                ),
                workspace_id: job.workspace_id.clone(),
                thread_id: None,
                session_id: None,
                ref_id: Some(job.id.clone()),
            },
        )
        .await;
    }
    first
}

#[must_use]
pub fn took(from: &str, to: Option<&str>) -> String {
    let parse = |t: &str| {
        DateTime::parse_from_rfc3339(t)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let end = to.and_then(parse).unwrap_or_else(Utc::now);
    let secs = parse(from).map_or(0, |start| (end - start).num_seconds().max(0));
    match secs {
        0..60 => format!("{secs} 秒"),
        60..3600 => format!("{} 分 {} 秒", secs / 60, secs % 60),
        _ => format!("{} 小时 {} 分", secs / 3600, secs % 3600 / 60),
    }
}

fn headline(job: &Job) -> String {
    let how = match job.status.as_str() {
        "done" => "跑完了",
        "failed" => "失败了",
        "killed" => "被结束了",
        "lost" => "失联了",
        _ => "还在跑",
    };
    format!("后台任务「{}」{how}", job.label)
}

#[must_use]
pub fn summary(job: &Job, tail: &str) -> String {
    let code = match (job.exit_code, job.status.as_str()) {
        (Some(c), _) => format!("退出码：{c}"),
        (None, "killed") => "退出码：无（被结束）".into(),
        (None, "lost") => "退出码：无（任务目录或进程不见了）".into(),
        (None, _) => "退出码未知（登记的进程拿不到退出码）".into(),
    };
    let lines: Vec<&str> = tail.lines().collect();
    let tail = lines[lines.len().saturating_sub(SUMMARY_LINES)..].join("\n");
    let mut out = format!(
        "{}。\n任务 id：{}\n机器：{}\n{code}\n耗时：{}\n",
        headline(job),
        job.id,
        job.node,
        took(&job.created_at, job.ended_at.as_deref()),
    );
    if let Some(c) = &job.command {
        out.push_str(&format!("命令：{c}\n"));
    }
    if tail.trim().is_empty() {
        out.push_str("日志最后 20 行：（没有输出）\n");
    } else {
        out.push_str(&format!("日志最后 20 行：\n```\n{tail}\n```\n"));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Resume,
    Interject,
    Busy,
}

pub async fn route(st: &Shared, ws: WorkspaceId, thread: &str) -> Wake {
    let running: Option<(SessionId, bool)> = st
        .running
        .read()
        .await
        .iter()
        .find(|(_, r)| r.workspace_id == ws)
        .map(|(sid, r)| (*sid, r.live.as_ref().is_some_and(|l| l.interactive)));
    let Some((sid, interactive)) = running else {
        return Wake::Resume;
    };
    let its: Option<String> =
        sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten();
    if interactive && its.as_deref() == Some(thread) {
        Wake::Interject
    } else {
        Wake::Busy
    }
}

async fn wake_session(st: &Shared, thread: &str, text: &str) -> Result<(), String> {
    let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT workspace_id, runtime_kind, agent_profile, node FROM sessions
         WHERE COALESCE(thread_id, id) = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let (ws, runtime, profile, node) = row.ok_or("找不到发起任务的会话")?;
    let wsid = WorkspaceId(ws.parse().map_err(|_| "工作区 id 无效")?);
    let how = route(st, wsid, thread).await;
    if how == Wake::Busy {
        return Err("工作区正在跑别的对话".into());
    }
    let agent = if runtime == "claude_cli" {
        "claude".to_owned()
    } else {
        runtime
    };
    let req = crate::api::PromptRequest {
        text: format!("（hub 通知）你派出的后台任务结束了。\n\n{text}"),
        resume: true,
        resume_session: Some(thread.to_owned()),
        interject: how == Wake::Interject,
        agent: Some(agent),
        profile,
        brain: Some(if node.as_deref().is_none_or(|n| n == "local") {
            "local".into()
        } else {
            "node".into()
        }),
        ..crate::api::PromptRequest::default()
    };
    let out = crate::api::prompt_for_bot(st.clone(), ws, req).await?;
    if out["admitted"] == false {
        return Err(out["reason"].as_str().unwrap_or("工作区被占着").to_owned());
    }
    Ok(())
}

async fn to_inbox(st: &Shared, job: &Job, text: String) {
    let thread = job
        .session_id
        .clone()
        .filter(|_| job.owner_kind == "session");
    let home: Option<String> = match &thread {
        Some(t) => sqlx::query_scalar(
            "SELECT workspace_id FROM sessions WHERE COALESCE(thread_id, id) = ?1
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(t)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten(),
        None => None,
    };
    crate::inbox::push(
        st,
        crate::inbox::Item {
            kind: "job",
            title: headline(job),
            body: text,
            workspace_id: home.clone().or_else(|| job.workspace_id.clone()),
            thread_id: thread.filter(|_| home.is_some()),
            session_id: None,
            ref_id: Some(job.id.clone()),
        },
    )
    .await;
}

pub async fn announce(st: &Shared, job: &Job, tail: &str) {
    if job.notify == "none" {
        return;
    }
    let text = summary(job, tail);
    if job.notify == "wake" {
        match job.owner() {
            Owner::Bot { bot, thread } => {
                let hint = match thread {
                    Some(t) => super::thread_by_session_thread(st, &t)
                        .await
                        .filter(|th| th.bot_id == bot)
                        .map(|th| th.id),
                    None => None,
                };
                let sent = super::send(
                    st,
                    &bot,
                    super::NewMail {
                        body: text.clone(),
                        subject: Some(headline(job)),
                        thread: hint,
                        from_kind: "system".into(),
                        from_ref: format!("job:{}", job.id),
                        ..super::NewMail::default()
                    },
                )
                .await;
                match sent {
                    Ok(_) => return,
                    Err(e) => {
                        tracing::warn!(target: "blazar::jobs", "给 bot 发后台任务通知失败，改进收件箱: {e}");
                    }
                }
            }
            Owner::Session { thread, .. } => match wake_session(st, &thread, &text).await {
                Ok(()) => return,
                Err(e) => {
                    tracing::info!(target: "blazar::jobs", "没能唤醒发起任务的会话，改进收件箱: {e}");
                }
            },
            Owner::User => {}
        }
    }
    to_inbox(st, job, text).await;
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
