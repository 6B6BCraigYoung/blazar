use std::path::PathBuf;

use blazar_transport::detached::{DetachedRun, RunMode};
use blazar_transport::{ExecSpec, shell_quote};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::Shared;
use crate::bots::Caller;

use super::announce::announce;
use super::{
    Job, MAX_TAIL, NOTIFY, Owner, SUMMARY_LINES, WAIT_KINDS, changed, get, nonempty, visible_to,
};

const MAX_COMMAND: usize = 8000;
const MAX_DEADLINE_MINS: i64 = 30 * 24 * 60;
const MAX_WAITS: usize = 20;

fn check_notify(n: Option<&str>) -> Result<String, String> {
    let n = nonempty(n).unwrap_or("wake");
    if NOTIFY.contains(&n) {
        Ok(n.to_owned())
    } else {
        Err(format!("notify must be one of {}", NOTIFY.join(" / ")))
    }
}

fn deadline(mins: Option<i64>) -> Result<Option<String>, String> {
    match mins {
        None => Ok(None),
        Some(m) if (1..=MAX_DEADLINE_MINS).contains(&m) => Ok(Some(
            (Utc::now() + chrono::Duration::minutes(m)).to_rfc3339(),
        )),
        Some(_) => Err(format!(
            "deadline_mins must be between 1 and {MAX_DEADLINE_MINS}"
        )),
    }
}

pub async fn check_waits(st: &Shared, items: &[Value]) -> Result<String, String> {
    if items.len() > MAX_WAITS {
        return Err(format!("waits_for allows at most {MAX_WAITS} items"));
    }
    let mut out = Vec::new();
    for item in items {
        let fields = item.as_object().filter(|o| o.len() == 1).ok_or_else(|| {
            format!(
                "Each waits_for item must have exactly one key: {}",
                WAIT_KINDS.join(" / ")
            )
        })?;
        let (kind, value) = fields.iter().next().ok_or("waits_for has an empty item")?;
        let value = value
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("waits_for {kind} must be a non-empty string"))?;
        match kind.as_str() {
            "node" => {
                node_id(st, value).await?;
            }
            "fact" => {
                if !crate::bots::facts::valid_key(value) {
                    return Err(format!("Invalid fact key: {value}"));
                }
            }
            "path" => {
                if !value.starts_with('/') {
                    return Err(format!("waits_for path must be absolute: {value}"));
                }
            }
            "job" => {
                get(st, value)
                    .await
                    .ok_or_else(|| format!("No such background job {value}"))?;
            }
            other => {
                return Err(format!(
                    "waits_for doesn't support {other}; use {}",
                    WAIT_KINDS.join(" / ")
                ));
            }
        }
        out.push(json!({ kind.as_str(): value }));
    }
    Ok(Value::Array(out).to_string())
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
        .map_err(|e| format!("Can't reach {node}: {e}"))?;
    out.stdout
        .lines()
        .find_map(|l| l.strip_prefix("__BLAZAR_JOBS_ROOT__ "))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| format!("Couldn't determine the job directory on {node}"))
}

async fn node_id(st: &Shared, node: &str) -> Result<String, String> {
    sqlx::query_scalar("SELECT id FROM nodes WHERE name = ?1")
        .bind(node)
        .fetch_optional(st.db.pool())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No such machine {node}"))
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
    waits_for: String,
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
                           waits_for, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
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
    .bind(&j.waits_for)
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
    #[serde(default)]
    pub waits_for: Vec<Value>,
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
        return Err("command can't be empty".into());
    }
    if command.chars().count() > MAX_COMMAND {
        return Err(format!("command is limited to {MAX_COMMAND} characters"));
    }
    let notify = check_notify(req.notify.as_deref())?;
    let deadline_at = deadline(req.deadline_mins)?;
    let waits_for = check_waits(st, &req.waits_for).await?;
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
        .map_err(|e| format!("Failed to start on {node}: {e}"))?;
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
            waits_for,
            deadline_at,
            notify,
        },
    )
    .await;
    if let Err(e) = saved {
        let _ = run.hard_kill().await;
        return Err(format!(
            "Failed to record the job; stopped the process that was just started: {e}"
        ));
    }
    changed(st);
    get(st, &id)
        .await
        .ok_or_else(|| "Couldn't read the job back".into())
}

#[derive(Debug, Default, Deserialize)]
pub struct Register {
    #[serde(default)]
    pub node: Option<String>,
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
    #[serde(default)]
    pub waits_for: Vec<Value>,
}

async fn register_target(
    st: &Shared,
    req: &Register,
    owner: &Owner,
    by: Option<&Caller>,
) -> Result<(String, Option<String>), String> {
    let ws = nonempty(req.workspace_id.as_deref());
    let asked = nonempty(req.node.as_deref());
    if let Some(c) = by.filter(|c| !c.is_lead()) {
        let ws = ws.ok_or(
            "Workers registering a background job must pass a workspace_id from their claims or home workspace; the machine is derived from it",
        )?;
        crate::bots::claims::may_use(st, c, ws).await?;
    }
    match ws {
        Some(ws) => {
            let (node, _) = crate::api::locate(st, ws)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(n) = asked.filter(|n| *n != node) {
                return Err(format!("Workspace {ws} is on {node}, not {n}"));
            }
            Ok((node, Some(ws.to_owned())))
        }
        None => {
            let node = asked.ok_or("Pass workspace_id (or node)")?;
            let ws = match owner {
                Owner::Session { workspace, .. } if !workspace.is_empty() => {
                    Some(workspace.clone())
                }
                _ => None,
            };
            Ok((node.to_owned(), ws))
        }
    }
}

pub fn may_kill(job: &Job, by: Option<&Caller>) -> Result<(), String> {
    if !visible_to(job, by) {
        return Err("Workers can only kill background jobs they started".into());
    }
    if job.kind == "register" && by.is_some_and(|c| !c.is_lead()) {
        return Err("Only the lead bot and the user can kill registered background jobs".into());
    }
    Ok(())
}

pub async fn register(
    st: &Shared,
    req: Register,
    owner: &Owner,
    by: Option<&Caller>,
) -> Result<Job, String> {
    let label = req.label.trim();
    if label.is_empty() {
        return Err("label can't be empty".into());
    }
    let pid = u32::try_from(req.pid)
        .ok()
        .filter(|p| *p > 1)
        .ok_or("Invalid pid")?;
    let notify = check_notify(req.notify.as_deref())?;
    let deadline_at = deadline(req.deadline_mins)?;
    let waits_for = check_waits(st, &req.waits_for).await?;
    let log_path = nonempty(req.log_path.as_deref()).map(str::to_owned);
    if log_path.as_deref().is_some_and(|p| !p.starts_with('/')) {
        return Err("Log path must be absolute".into());
    }
    let (node, workspace_id) = register_target(st, &req, owner, by).await?;
    let node = node.as_str();
    let node_id = node_id(st, node).await?;
    let id = uuid::Uuid::now_v7().to_string();
    let root = root(st, node).await?;
    let run = DetachedRun::adopt(st.transport(node), &root, &id, pid)
        .await
        .map_err(|e| format!("Failed to register on {node}: {e}"))?;
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
            waits_for,
            deadline_at,
            notify,
        },
    )
    .await?;
    changed(st);
    get(st, &id)
        .await
        .ok_or_else(|| "Couldn't read the job back".into())
}

pub(super) async fn finish(
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
        .map_err(|e| format!("Can't reach {}: {e}", job.node))?;
    if out.code != 0 && out.stdout.is_empty() {
        return Err(format!("Couldn't read the log: {}", out.stderr.trim()));
    }
    Ok(out.stdout)
}

pub async fn kill(
    st: &Shared,
    id: &str,
    by: Option<&Caller>,
    killer: &Owner,
) -> Result<Job, String> {
    let job = get(st, id).await.ok_or("No such background job")?;
    may_kill(&job, by)?;
    if job.status != "running" {
        return Err(format!("This job is already {}", job.status));
    }
    let dir = job.run_dir.clone().ok_or("This job has no job directory")?;
    let run = DetachedRun::attach(st.transport(&job.node), dir, job.id.clone());
    let stopped = if job.kind == "register" {
        run.stop_adopted().await
    } else {
        run.hard_kill().await
    };
    stopped.map_err(|e| format!("Failed to kill: {e}"))?;
    if finish(st, &job.id, "killed", None, None).await {
        changed(st);
        if let Some(done) = get(st, &job.id).await
            && !done.owner().same(killer)
        {
            let tail = log(st, &done, SUMMARY_LINES).await.unwrap_or_default();
            announce(st, &done, &tail).await;
        }
    }
    get(st, id)
        .await
        .ok_or_else(|| "Couldn't read the job back".into())
}
