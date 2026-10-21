use blazar_core_types::transfer::{ExportFile, ImportReport, PAUSED_REASON, WorkspaceRef};
use chrono::Utc;
use sqlx::SqliteConnection;

use crate::api::Shared;
use crate::state::ServerEvent;

use super::plan::plan;
use super::{Op, Options, Plan, agent_of};

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

async fn id_of(conn: &mut SqliteConnection, sql: &str, key: &str) -> Result<String, String> {
    sqlx::query_scalar::<_, String>(sql)
        .bind(key)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("找不到「{key}」"))
}

async fn ws_id_of(conn: &mut SqliteConnection, r: &WorkspaceRef) -> Result<String, String> {
    sqlx::query_scalar::<_, String>(
        "SELECT w.id FROM workspaces w JOIN nodes n ON n.id = w.node_id
         WHERE n.name = ?1 AND w.path = ?2",
    )
    .bind(&r.node)
    .bind(&r.path)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("找不到工作区 {}:{}", r.node, r.path))
}

async fn live_agent(conn: &mut SqliteConnection, name: &str) -> Result<String, String> {
    id_of(
        conn,
        "SELECT id FROM agents WHERE name = ?1 AND archived_at IS NULL",
        name,
    )
    .await
}

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

async fn apply_one(
    conn: &mut SqliteConnection,
    op: &Op,
    now: &str,
    bot_home: Option<&std::path::Path>,
) -> Result<(), String> {
    let e = |e: sqlx::Error| e.to_string();
    match op {
        Op::Prefs(v) => {
            sqlx::query(
                "INSERT INTO kv_settings (key, value, updated_at) VALUES ('prefs', ?1, ?2)",
            )
            .bind(v.to_string())
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Node(n) => {
            sqlx::query(
                "INSERT INTO nodes (id, name, transport, endpoint, network, labels, role, status,
                                    created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'offline', ?8)",
            )
            .bind(blazar_core_types::NodeId::new().to_string())
            .bind(n.name.trim())
            .bind(&n.transport)
            .bind(&n.endpoint)
            .bind(&n.network)
            .bind(if n.labels.is_null() {
                "[]".to_owned()
            } else {
                n.labels.to_string()
            })
            .bind(&n.role)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Project(name) => {
            sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)")
                .bind(blazar_core_types::ProjectId::new().to_string())
                .bind(name)
                .bind(now)
                .execute(&mut *conn)
                .await
                .map_err(e)?;
        }
        Op::Workspace(w) => {
            let node = id_of(conn, "SELECT id FROM nodes WHERE name = ?1", &w.node).await?;
            let project = match &w.project {
                Some(p) => Some(id_of(conn, "SELECT id FROM projects WHERE name = ?1", p).await?),
                None => None,
            };
            sqlx::query(
                "INSERT INTO workspaces (id, project_id, node_id, name, path, branch, base_commit,
                                         origin, created_at, repo_root, target_branch)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'manual', ?8, ?9, ?10)",
            )
            .bind(blazar_core_types::WorkspaceId::new().to_string())
            .bind(project)
            .bind(node)
            .bind(&w.name)
            .bind(&w.path)
            .bind(&w.branch)
            .bind(&w.base_commit)
            .bind(now)
            .bind(&w.repo_root)
            .bind(&w.target_branch)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Skill(s) => {
            let id = new_id();
            sqlx::query(
                "INSERT INTO skills (id, name, description, source, origin, version, created_at,
                                     updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            )
            .bind(&id)
            .bind(s.name.trim())
            .bind(&s.description)
            .bind(&s.source)
            .bind(&s.origin)
            .bind(&s.version)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
            for f in &s.files {
                sqlx::query(
                    "INSERT INTO skill_files (skill_id, path, content) VALUES (?1, ?2, ?3)",
                )
                .bind(&id)
                .bind(&f.path)
                .bind(&f.content)
                .execute(&mut *conn)
                .await
                .map_err(e)?;
            }
        }
        Op::Mcp(m) => {
            sqlx::query(
                "INSERT INTO mcp_servers (id, name, description, transport, command, args, url, env,
                                          headers, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            )
            .bind(new_id())
            .bind(m.name.trim())
            .bind(m.description.trim())
            .bind(&m.transport)
            .bind(m.command.trim())
            .bind(to_json(&m.args))
            .bind(m.url.trim())
            .bind(to_json(&m.env))
            .bind(to_json(&m.headers))
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Agent(spec) => {
            let a = agent_of(spec)?;
            let id = new_id();
            sqlx::query(
                "INSERT INTO agents (id, name, runtime, description, instructions, model,
                                     permission_mode, custom_env, color, avatar, starters,
                                     thinking_level, custom_args, max_concurrent, created_at,
                                     updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
            )
            .bind(&id)
            .bind(&a.name)
            .bind(&a.runtime)
            .bind(&a.description)
            .bind(&a.instructions)
            .bind(&a.model)
            .bind(&a.permission_mode)
            .bind(to_json(&a.env))
            .bind(&a.color)
            .bind(&a.avatar)
            .bind(to_json(&a.starters))
            .bind(&a.thinking_level)
            .bind(to_json(&a.custom_args))
            .bind(a.max_concurrent)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
            for s in &spec.skills {
                let skill = id_of(
                    conn,
                    "SELECT id FROM skills WHERE name = ?1 COLLATE NOCASE",
                    s,
                )
                .await?;
                sqlx::query(
                    "INSERT OR IGNORE INTO agent_skills (agent_id, skill_id) VALUES (?1, ?2)",
                )
                .bind(&id)
                .bind(skill)
                .execute(&mut *conn)
                .await
                .map_err(e)?;
            }
            for m in &spec.mcp_servers {
                let server = id_of(
                    conn,
                    "SELECT id FROM mcp_servers WHERE name = ?1 COLLATE NOCASE",
                    m,
                )
                .await?;
                sqlx::query(
                    "INSERT OR IGNORE INTO agent_mcp (agent_id, server_id) VALUES (?1, ?2)",
                )
                .bind(&id)
                .bind(server)
                .execute(&mut *conn)
                .await
                .map_err(e)?;
            }
        }
        Op::Snippet(s) => {
            sqlx::query(
                "INSERT INTO snippets (id, name, body, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
            )
            .bind(new_id())
            .bind(s.name.trim())
            .bind(&s.body)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Hook(h) => {
            sqlx::query(
                "INSERT INTO hooks (id, event, command, target, matcher, blocking, timeout_secs,
                                    enabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
            )
            .bind(new_id())
            .bind(&h.event)
            .bind(&h.command)
            .bind(&h.target)
            .bind(&h.matcher)
            .bind(i64::from(h.blocking))
            .bind(h.timeout_secs)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Rule(r) => {
            let ws = match &r.workspace {
                Some(w) => Some(ws_id_of(conn, w).await?),
                None => None,
            };
            sqlx::query(
                "INSERT INTO approval_rules (id, tool, pattern, workspace_id, enabled, hits,
                                             created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            )
            .bind(new_id())
            .bind(r.tool.trim())
            .bind(r.pattern.trim())
            .bind(ws)
            .bind(i64::from(r.enabled))
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Autopilot(a) => {
            let ws = match &a.workspace {
                Some(w) => ws_id_of(conn, w).await?,
                None => return Err(format!("自动化「{}」没有工作区", a.name)),
            };
            let agent = match &a.agent {
                Some(name) => Some(live_agent(conn, name).await?),
                None => None,
            };
            let nz = |s: &Option<String>| s.clone().filter(|v| !v.trim().is_empty());
            sqlx::query(
                "INSERT INTO autopilots (id, name, instructions, workspace_id, agent_profile,
                                         runtime, model, permission_mode, mode, title_template,
                                         status, paused_reason, cron, timezone, concurrency,
                                         created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'paused', ?11, ?12, ?13, ?14,
                         ?15, ?15)",
            )
            .bind(new_id())
            .bind(a.name.trim())
            .bind(&a.instructions)
            .bind(ws)
            .bind(agent)
            .bind(nz(&a.runtime))
            .bind(nz(&a.model))
            .bind(nz(&a.permission_mode))
            .bind(&a.mode)
            .bind(nz(&a.title_template))
            .bind(PAUSED_REASON)
            .bind(nz(&a.cron))
            .bind(&a.timezone)
            .bind(&a.concurrency)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
        Op::Bot(b) => {
            let agent_id = live_agent(conn, &b.agent).await?;
            crate::bots::insert(
                conn,
                crate::bots::NewBot {
                    handle: b.handle.clone(),
                    display_name: Some(b.display_name.clone()),
                    role: Some(b.role.clone()),
                    agent_id,
                    lifetime: Some("fixed".into()),
                    priority: Some(b.priority.clone()),
                    runtimes: Some(b.runtimes.clone()),
                    home_path: bot_home.map(|root| root.join(&b.handle).display().to_string()),
                    ..crate::bots::NewBot::default()
                },
            )
            .await
            .map_err(|err| format!("建 bot「{}」失败：{err}", b.handle))?;
        }
        Op::Memory(m) => {
            let scope = match m.scope.strip_prefix("bot:") {
                Some(handle) => format!(
                    "bot:{}",
                    id_of(
                        conn,
                        "SELECT id FROM bots WHERE handle = ?1 AND archived_at IS NULL",
                        handle,
                    )
                    .await?
                ),
                None => m.scope.clone(),
            };
            sqlx::query(
                "INSERT INTO memories (id, scope, name, description, type, body, version,
                                       updated_by, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 'import', ?7)",
            )
            .bind(new_id())
            .bind(scope)
            .bind(&m.name)
            .bind(m.description.trim())
            .bind(&m.kind)
            .bind(&m.body)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(e)?;
        }
    }
    Ok(())
}

pub async fn apply(conn: &mut SqliteConnection, plan: &Plan) -> Result<(), String> {
    let now = Utc::now().to_rfc3339();
    for (n, op) in plan.ops.iter().enumerate() {
        apply_one(conn, op, &now, plan.bot_home.as_deref())
            .await
            .map_err(|err| {
                let item = plan
                    .report
                    .items
                    .iter()
                    .filter(|i| i.imported)
                    .nth(n)
                    .map(|i| format!("{} {}", i.section, i.key))
                    .unwrap_or_default();
                format!("导入 {item} 时出错，已全部回滚：{err}")
            })?;
    }
    Ok(())
}

fn fresh_dirs(plan: &Plan) -> Vec<std::path::PathBuf> {
    let Some(root) = plan.bot_home.as_deref() else {
        return Vec::new();
    };
    let mut fresh = Vec::new();
    for op in &plan.ops {
        let Op::Bot(b) = op else { continue };
        let mut dir = Some(root.join(&b.handle));
        while let Some(d) = dir {
            if d.exists() {
                break;
            }
            dir = d.parent().map(std::path::Path::to_path_buf);
            if !fresh.contains(&d) {
                fresh.push(d);
            }
        }
    }
    fresh.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    fresh
}

pub async fn import(
    st: &Shared,
    file: &ExportFile,
    opts: &Options,
    dry_run: bool,
) -> Result<ImportReport, String> {
    if dry_run {
        let mut conn = st.db.pool().acquire().await.map_err(|e| e.to_string())?;
        return Ok(plan(&mut conn, file, opts).await?.report);
    }
    let mut tx = st
        .db
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|e| e.to_string())?;
    let plan = plan(&mut tx, file, opts).await?;
    let fresh = fresh_dirs(&plan);
    let done = match apply(&mut tx, &plan).await {
        Ok(()) => tx.commit().await.map_err(|e| e.to_string()),
        Err(err) => {
            drop(tx);
            Err(err)
        }
    };
    if let Err(err) = done {
        for dir in fresh {
            let _ = std::fs::remove_dir(dir);
        }
        return Err(err);
    }
    let mut report = plan.report;
    report.dry_run = false;
    report.applied = true;
    for ev in [
        ServerEvent::NodesChanged,
        ServerEvent::WorkspacesChanged,
        ServerEvent::AutopilotsChanged,
        ServerEvent::ResyncRequired,
    ] {
        st.emit(ev);
    }
    crate::bots::changed(st);
    Ok(report)
}
