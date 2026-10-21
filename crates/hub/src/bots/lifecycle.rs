use blazar_core_types::WorkspaceId;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use crate::api::Shared;

use super::mailbox::{NewMail, send};
use super::threads::{caller_thread, insert_thread};
use super::{
    BRAINS, Bot, Caller, DEFAULT_IDLE_TTL_SECS, LIFETIMES, Mail, PRIORITIES, ROLES, changed,
    default_home, get, valid_handle,
};

pub async fn ensure_local_workspace(
    conn: &mut SqliteConnection,
    path: &str,
    name: &str,
) -> Result<String, String> {
    let path = path.trim().trim_end_matches('/');
    if path.is_empty() || !path.starts_with('/') {
        return Err("家目录要用绝对路径".into());
    }
    std::fs::create_dir_all(path).map_err(|e| format!("创建家目录 {path} 失败：{e}"))?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES (?1, 'local', 'local', 'online', ?2) ON CONFLICT (name) DO NOTHING",
    )
    .bind(blazar_core_types::NodeId::new().to_string())
    .bind(&now)
    .execute(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;
    let node_id: String = sqlx::query_scalar("SELECT id FROM nodes WHERE name = 'local'")
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (node_id, path) DO NOTHING",
    )
    .bind(WorkspaceId::new().to_string())
    .bind(&node_id)
    .bind(name)
    .bind(path)
    .bind(&now)
    .execute(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query_scalar("SELECT id FROM workspaces WHERE node_id = ?1 AND path = ?2")
        .bind(&node_id)
        .bind(path)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Default, Deserialize)]
pub struct NewBot {
    pub handle: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub duty: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub lifetime: Option<String>,
    #[serde(default)]
    pub idle_ttl_secs: Option<i64>,
    #[serde(default)]
    pub home_path: Option<String>,
    #[serde(default)]
    pub home_ws_id: Option<String>,
    #[serde(default)]
    pub brain: Option<String>,
    #[serde(default)]
    pub brain_ws_id: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub runtimes: Option<Vec<Value>>,
}

fn validate_new(req: &NewBot) -> Result<(), String> {
    if !valid_handle(req.handle.trim()) {
        return Err("handle 只能用小写字母、数字和连字符，1–32 个字符".into());
    }
    if let Some(role) = &req.role
        && !ROLES.contains(&role.as_str())
    {
        return Err(format!("未知角色 {role}"));
    }
    if let Some(brain) = &req.brain
        && !BRAINS.contains(&brain.as_str())
    {
        return Err(format!("未知思考位置 {brain}"));
    }
    if let Some(priority) = &req.priority
        && !PRIORITIES.contains(&priority.as_str())
    {
        return Err(format!("未知优先级 {priority}"));
    }
    if let Some(lifetime) = &req.lifetime
        && !LIFETIMES.contains(&lifetime.as_str())
    {
        return Err(format!("未知生命周期 {lifetime}，可用 fixed / temporary"));
    }
    if req
        .display_name
        .as_deref()
        .is_some_and(|n| n.trim().chars().count() > 40)
    {
        return Err("名字最多 40 个字".into());
    }
    Ok(())
}

async fn handle_free(conn: &mut SqliteConnection, handle: &str) -> Result<(), String> {
    let taken: Option<Option<String>> =
        sqlx::query_scalar("SELECT archived_at FROM bots WHERE handle = ?1 LIMIT 1")
            .bind(handle)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
    match taken {
        Some(Some(_)) => Err(format!(
            "handle「{handle}」被一个已归档的 bot 占着，恢复它或者换个名字"
        )),
        Some(None) => Err(format!("handle「{handle}」已经有了")),
        None => Ok(()),
    }
}

pub async fn create(st: &Shared, mut req: NewBot) -> Result<Bot, String> {
    validate_new(&req)?;
    let handle = req.handle.trim().to_owned();
    {
        let mut conn = st.db.pool().acquire().await.map_err(|e| e.to_string())?;
        handle_free(&mut conn, &handle).await?;
    }
    if req.agent_id.trim().is_empty() {
        let duty = req
            .duty
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .ok_or("给它一句职责，或者选一个智能体档案")?;
        if duty.chars().count() > 2000 {
            return Err("职责最多 2000 字".into());
        }
        let runtime = req.runtime.clone().unwrap_or_else(|| "claude".into());
        let summary: String = duty
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(255)
            .collect();
        req.agent_id =
            crate::agents::ensure_profile(st, &format!("bot:{handle}"), &runtime, &summary, duty)
                .await?
                .id;
    }
    let mut tx = st
        .db
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|e| e.to_string())?;
    let id = insert(&mut tx, req).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    changed(st);
    get(st, &id)
        .await
        .ok_or_else(|| "刚建的 bot 读不回来".into())
}

pub(crate) async fn insert(conn: &mut SqliteConnection, req: NewBot) -> Result<String, String> {
    validate_new(&req)?;
    let handle = req.handle.trim().to_owned();
    handle_free(conn, &handle).await?;
    let role = req.role.unwrap_or_else(|| "worker".into());
    let brain = req.brain.unwrap_or_else(|| "hub".into());
    let priority = req.priority.unwrap_or_else(|| "P1".into());
    let lifetime = req.lifetime.unwrap_or_else(|| "fixed".into());
    let idle_ttl = if lifetime == "temporary" {
        Some(
            req.idle_ttl_secs
                .filter(|t| *t > 0)
                .unwrap_or(DEFAULT_IDLE_TTL_SECS)
                .min(30 * 86_400),
        )
    } else {
        None
    };
    let agent: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT name, archived_at FROM agents WHERE id = ?1")
            .bind(req.agent_id.trim())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
    match agent {
        None => return Err("没有这个智能体档案".into()),
        Some((name, Some(_))) => return Err(format!("智能体「{name}」已归档")),
        Some(_) => {}
    }
    if brain == "node" {
        let ws = req.brain_ws_id.as_deref().unwrap_or_default();
        if !workspace_exists(conn, ws).await? {
            return Err("在工作机器上思考需要指定一个存在的工作区".into());
        }
    }
    let display_name = req
        .display_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| handle.clone());
    let home_ws_id = match req.home_ws_id.filter(|w| !w.trim().is_empty()) {
        Some(ws) => {
            if !workspace_exists(conn, &ws).await? {
                return Err("家工作区不存在".into());
            }
            ws
        }
        None => {
            let path = req.home_path.unwrap_or_else(|| default_home(&handle));
            ensure_local_workspace(conn, &path, &format!("bot:{handle}")).await?
        }
    };
    let now = Utc::now().to_rfc3339();
    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO bots (id, handle, display_name, role, agent_id, home_ws_id, brain, brain_ws_id,
                           priority, runtimes, created_at, updated_at, lifetime, idle_ttl_secs)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13)",
    )
    .bind(&id)
    .bind(&handle)
    .bind(&display_name)
    .bind(&role)
    .bind(req.agent_id.trim())
    .bind(&home_ws_id)
    .bind(&brain)
    .bind(req.brain_ws_id.filter(|_| brain == "node"))
    .bind(&priority)
    .bind(json!(req.runtimes.unwrap_or_default()).to_string())
    .bind(&now)
    .bind(&lifetime)
    .bind(idle_ttl)
    .execute(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;
    insert_thread(conn, &id, "main", "").await?;
    Ok(id)
}

async fn workspace_exists(conn: &mut SqliteConnection, id: &str) -> Result<bool, String> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workspaces WHERE id = ?1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
}

#[derive(Debug, Default, Deserialize)]
pub struct Patch {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub brain: Option<String>,
    #[serde(default)]
    pub brain_ws_id: Option<String>,
    #[serde(default)]
    pub runtimes: Option<Vec<Value>>,
    #[serde(default)]
    pub hot_for_secs: Option<i64>,
    #[serde(default)]
    pub lifetime: Option<String>,
    #[serde(default)]
    pub idle_ttl_secs: Option<i64>,
}

pub async fn update(st: &Shared, id: &str, p: Patch) -> Result<Bot, String> {
    let bot = get(st, id).await.ok_or("没有这个 bot")?;
    if let Some(r) = p.role.as_deref()
        && !ROLES.contains(&r)
    {
        return Err(format!("未知角色 {r}"));
    }
    if let Some(x) = p.priority.as_deref()
        && !PRIORITIES.contains(&x)
    {
        return Err(format!("未知优先级 {x}"));
    }
    if let Some(b) = p.brain.as_deref()
        && !BRAINS.contains(&b)
    {
        return Err(format!("未知思考位置 {b}"));
    }
    if let Some(a) = p.agent_id.as_deref() {
        crate::agents::get(st, a)
            .await
            .ok_or("没有这个智能体档案")?;
    }
    if let Some(l) = p.lifetime.as_deref()
        && !LIFETIMES.contains(&l)
    {
        return Err(format!("未知生命周期 {l}"));
    }
    let lifetime = p.lifetime.clone().unwrap_or(bot.lifetime.clone());
    let idle_ttl = if lifetime == "temporary" {
        Some(
            p.idle_ttl_secs
                .filter(|t| *t > 0)
                .or(bot.idle_ttl_secs)
                .unwrap_or(DEFAULT_IDLE_TTL_SECS)
                .min(30 * 86_400),
        )
    } else {
        None
    };
    let brain = p.brain.clone().unwrap_or(bot.brain.clone());
    let brain_ws = p.brain_ws_id.clone().or(bot.brain_ws_id.clone());
    if brain == "node" {
        let ws = brain_ws.as_deref().unwrap_or_default();
        if crate::api::locate(st, ws).await.is_err() {
            return Err("在工作机器上思考需要指定一个存在的工作区".into());
        }
    }
    let display = p
        .display_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty() && n.chars().count() <= 40);
    let hot_until = p.hot_for_secs.map(|secs| {
        if secs <= 0 {
            String::new()
        } else {
            (Utc::now() + chrono::Duration::seconds(secs.min(86_400))).to_rfc3339()
        }
    });
    sqlx::query(
        "UPDATE bots SET display_name = COALESCE(?2, display_name), role = COALESCE(?3, role),
                agent_id = COALESCE(?4, agent_id), priority = COALESCE(?5, priority),
                brain = ?6, brain_ws_id = ?7, runtimes = COALESCE(?8, runtimes),
                hot_until = CASE WHEN ?9 IS NULL THEN hot_until WHEN ?9 = '' THEN NULL ELSE ?9 END,
                updated_at = ?10, lifetime = ?11, idle_ttl_secs = ?12
         WHERE id = ?1",
    )
    .bind(&bot.id)
    .bind(display)
    .bind(p.role)
    .bind(p.agent_id)
    .bind(p.priority)
    .bind(&brain)
    .bind(brain_ws.filter(|_| brain == "node"))
    .bind(p.runtimes.map(|r| json!(r).to_string()))
    .bind(hot_until)
    .bind(Utc::now().to_rfc3339())
    .bind(&lifetime)
    .bind(idle_ttl)
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    changed(st);
    get(st, &bot.id).await.ok_or_else(|| "bot 不见了".into())
}

pub async fn promote(st: &Shared, id: &str) -> Result<Bot, String> {
    let bot = get(st, id).await.ok_or("没有这个 bot")?;
    sqlx::query(
        "UPDATE bots SET lifetime = 'fixed', idle_ttl_secs = NULL, updated_at = ?2 WHERE id = ?1",
    )
    .bind(&bot.id)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    changed(st);
    get(st, &bot.id).await.ok_or_else(|| "bot 不见了".into())
}

pub async fn restore(st: &Shared, id: &str) -> Result<Bot, String> {
    let bot = get(st, id).await.ok_or("没有这个 bot")?;
    if bot.archived_at.is_none() {
        return Ok(bot);
    }
    if get(st, &bot.handle)
        .await
        .is_some_and(|b| b.id != bot.id && b.archived_at.is_none())
    {
        return Err(format!("handle「{}」已经被另一个 bot 用了", bot.handle));
    }
    sqlx::query(
        "UPDATE bots SET archived_at = NULL, status = 'idle', status_note = '', updated_at = ?2,
                last_active_at = ?2
         WHERE id = ?1",
    )
    .bind(&bot.id)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    changed(st);
    get(st, &bot.id).await.ok_or_else(|| "bot 不见了".into())
}

#[derive(Debug, Default, Deserialize)]
pub struct Spawn {
    #[serde(default)]
    pub handle: Option<String>,
    pub duty: String,
    pub text: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub idle_ttl_secs: Option<i64>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub home_path: Option<String>,
}

pub async fn spawn(st: &Shared, req: Spawn, by: Option<&Caller>) -> Result<(Bot, Mail), String> {
    let handle = match req
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        Some(h) => h.to_owned(),
        None => {
            let mut picked = None;
            for _ in 0..8 {
                let mut bytes = [0u8; 2];
                getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
                let candidate = format!("tmp-{:02x}{:02x}", bytes[0], bytes[1]);
                if get(st, &candidate).await.is_none() {
                    picked = Some(candidate);
                    break;
                }
            }
            picked.ok_or("一时想不出没用过的名字，指定一个 handle 吧")?
        }
    };
    let bot = create(
        st,
        NewBot {
            handle,
            display_name: req.display_name,
            duty: Some(req.duty),
            runtime: req.runtime,
            lifetime: Some("temporary".into()),
            idle_ttl_secs: req.idle_ttl_secs,
            priority: req.priority,
            home_path: req.home_path,
            ..NewBot::default()
        },
    )
    .await?;
    let (from_kind, from_ref, from_thread) = match by {
        Some(c) => ("bot".to_owned(), c.bot.clone(), caller_thread(st, c).await),
        None => ("user".to_owned(), String::new(), None),
    };
    let mail = send(
        st,
        &bot.id,
        NewMail {
            body: req.text,
            from_kind,
            from_ref,
            from_thread,
            ..NewMail::default()
        },
    )
    .await?;
    Ok((bot, mail))
}

pub async fn set_status(st: &Shared, id: &str, status: &str, note: &str) {
    let _ = sqlx::query(
        "UPDATE bots SET status = ?2, status_note = ?3, updated_at = ?4,
                last_active_at = CASE WHEN ?2 IN ('running', 'idle') THEN ?4 ELSE last_active_at END
         WHERE id = ?1",
    )
    .bind(id)
    .bind(status)
    .bind(note.chars().take(300).collect::<String>())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    changed(st);
}

pub async fn pause(st: &Shared, id: &str, paused: bool) -> Result<Bot, String> {
    let bot = get(st, id).await.ok_or("没有这个 bot")?;
    let status = if paused { "paused" } else { "idle" };
    set_status(st, &bot.id, status, "").await;
    get(st, &bot.id).await.ok_or_else(|| "bot 不见了".into())
}

pub async fn archive(st: &Shared, id: &str) -> Result<(), String> {
    let bot = get(st, id).await.ok_or("没有这个 bot")?;
    sqlx::query(
        "UPDATE bots SET archived_at = ?2, status = 'paused', updated_at = ?2 WHERE id = ?1",
    )
    .bind(&bot.id)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let _ = sqlx::query(
        "UPDATE bot_mail SET status = 'expired' WHERE to_bot = ?1 AND status = 'queued'",
    )
    .bind(&bot.id)
    .execute(st.db.pool())
    .await;
    changed(st);
    Ok(())
}
