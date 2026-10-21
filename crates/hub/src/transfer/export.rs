use blazar_core_types::transfer::{
    AgentSpec, AutopilotSpec, BotSpec, ExportFile, FORMAT, HookSpec, McpSpec, MemorySpec, NodeSpec,
    ProjectSpec, RuleSpec, SkillFileSpec, SkillSpec, SnippetSpec, VERSION, WorkspaceRef,
    WorkspaceSpec,
};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use super::keys;

fn json_or(raw: Option<String>, fallback: Value) -> Value {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(fallback)
}

fn ws_ref(node: Option<String>, path: Option<String>) -> Option<WorkspaceRef> {
    Some(WorkspaceRef {
        node: node?,
        path: path?,
    })
}

pub async fn export_from(conn: &mut SqliteConnection) -> Result<ExportFile, sqlx::Error> {
    let prefs =
        sqlx::query_scalar::<_, String>("SELECT value FROM kv_settings WHERE key = 'prefs'")
            .fetch_optional(&mut *conn)
            .await?
            .and_then(|v| serde_json::from_str::<Value>(&v).ok());

    let nodes = sqlx::query(
        "SELECT name, transport, endpoint, network, labels, role FROM nodes
         WHERE name <> 'local' ORDER BY name",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| NodeSpec {
        name: r.get("name"),
        transport: r.get("transport"),
        endpoint: r.get("endpoint"),
        network: r.get("network"),
        labels: match json_or(r.get("labels"), json!([])) {
            Value::Array(labels) => Value::Array(labels),
            _ => json!([]),
        },
        role: r.get("role"),
    })
    .collect();

    let projects = sqlx::query_scalar::<_, String>("SELECT name FROM projects ORDER BY name")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|name| ProjectSpec { name })
        .collect();

    let workspaces = sqlx::query(
        "SELECT n.name AS node, w.path, w.name, p.name AS project, w.branch, w.base_commit,
                w.target_branch, w.repo_root
         FROM workspaces w JOIN nodes n ON n.id = w.node_id
         LEFT JOIN projects p ON p.id = w.project_id
         WHERE w.origin = 'manual' AND w.parent_id IS NULL AND w.task_id IS NULL
           AND w.id NOT IN (SELECT home_ws_id FROM bots
                            UNION SELECT brain_ws_id FROM bots WHERE brain_ws_id IS NOT NULL)
         ORDER BY n.name, w.path",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| WorkspaceSpec {
        node: r.get("node"),
        path: r.get("path"),
        name: r.get("name"),
        project: r.get("project"),
        branch: r.get("branch"),
        base_commit: r.get("base_commit"),
        target_branch: r.get("target_branch"),
        repo_root: r.get("repo_root"),
    })
    .collect();

    let mut skills = Vec::new();
    for r in sqlx::query(
        "SELECT id, name, description, source, origin, version FROM skills
         ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let id: String = r.get("id");
        let files =
            sqlx::query("SELECT path, content FROM skill_files WHERE skill_id = ?1 ORDER BY path")
                .bind(&id)
                .fetch_all(&mut *conn)
                .await?
                .iter()
                .map(|f| SkillFileSpec {
                    path: f.get("path"),
                    content: f.get("content"),
                })
                .collect();
        skills.push(SkillSpec {
            name: r.get("name"),
            description: r.get("description"),
            source: r.get("source"),
            origin: r.get("origin"),
            version: r.get("version"),
            files,
        });
    }

    let mcp_servers = sqlx::query(
        "SELECT name, description, transport, command, args, url, env, headers FROM mcp_servers
         ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| McpSpec {
        name: r.get("name"),
        description: r.get("description"),
        transport: r.get("transport"),
        command: r.get("command"),
        args: serde_json::from_str(&r.get::<String, _>("args")).unwrap_or_default(),
        url: r.get("url"),
        env: keys(&r.get::<String, _>("env")),
        headers: keys(&r.get::<String, _>("headers")),
    })
    .collect();

    let mut agents = Vec::new();
    for r in sqlx::query("SELECT * FROM agents WHERE archived_at IS NULL ORDER BY name")
        .fetch_all(&mut *conn)
        .await?
    {
        let id: String = r.get("id");
        let skills = sqlx::query_scalar::<_, String>(
            "SELECT s.name FROM agent_skills x JOIN skills s ON s.id = x.skill_id
             WHERE x.agent_id = ?1 ORDER BY s.name COLLATE NOCASE",
        )
        .bind(&id)
        .fetch_all(&mut *conn)
        .await?;
        let mcp_servers = sqlx::query_scalar::<_, String>(
            "SELECT m.name FROM agent_mcp x JOIN mcp_servers m ON m.id = x.server_id
             WHERE x.agent_id = ?1 ORDER BY m.name COLLATE NOCASE",
        )
        .bind(&id)
        .fetch_all(&mut *conn)
        .await?;
        agents.push(AgentSpec {
            name: r.get("name"),
            runtime: r.get("runtime"),
            description: r.get("description"),
            instructions: r.get("instructions"),
            model: r.get("model"),
            permission_mode: r.get("permission_mode"),
            custom_env: keys(&r.get::<String, _>("custom_env")),
            color: r.get("color"),
            avatar: r.get("avatar"),
            starters: json_or(r.get("starters"), json!([])),
            thinking_level: r.get("thinking_level"),
            custom_args: serde_json::from_str(&r.get::<String, _>("custom_args"))
                .unwrap_or_default(),
            max_concurrent: Some(r.get("max_concurrent")),
            skills,
            mcp_servers,
        });
    }

    let snippets = sqlx::query("SELECT name, body FROM snippets ORDER BY name COLLATE NOCASE")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(|r| SnippetSpec {
            name: r.get("name"),
            body: r.get("body"),
        })
        .collect();

    let hooks = sqlx::query(
        "SELECT event, command, target, matcher, blocking, timeout_secs FROM hooks
         ORDER BY event, command, target, matcher",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| HookSpec {
        event: r.get("event"),
        command: r.get("command"),
        target: r.get("target"),
        matcher: r.get("matcher"),
        blocking: r.get::<i64, _>("blocking") != 0,
        timeout_secs: r.get("timeout_secs"),
    })
    .collect();

    let approval_rules = sqlx::query(
        "SELECT r.tool, r.pattern, r.enabled, n.name AS node, w.path
         FROM approval_rules r LEFT JOIN workspaces w ON w.id = r.workspace_id
         LEFT JOIN nodes n ON n.id = w.node_id
         ORDER BY r.tool, r.pattern, n.name, w.path",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| RuleSpec {
        tool: r.get("tool"),
        pattern: r.get("pattern"),
        enabled: r.get::<i64, _>("enabled") != 0,
        workspace: ws_ref(r.get("node"), r.get("path")),
    })
    .collect();

    let autopilots = sqlx::query(
        "SELECT a.*, g.name AS agent_name, n.name AS node, w.path AS ws_path
         FROM autopilots a LEFT JOIN agents g ON g.id = a.agent_profile
         LEFT JOIN workspaces w ON w.id = a.workspace_id
         LEFT JOIN nodes n ON n.id = w.node_id
         ORDER BY a.name, n.name, w.path",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| AutopilotSpec {
        name: r.get("name"),
        instructions: r.get("instructions"),
        workspace: ws_ref(r.get("node"), r.get("ws_path")),
        agent: r.get("agent_name"),
        runtime: r.get("runtime"),
        model: r.get("model"),
        permission_mode: r.get("permission_mode"),
        mode: r.get("mode"),
        title_template: r.get("title_template"),
        cron: r.get("cron"),
        timezone: r.get("timezone"),
        concurrency: r.get("concurrency"),
    })
    .collect();

    let bots = sqlx::query(
        "SELECT b.handle, b.display_name, b.role, g.name AS agent, b.priority, b.runtimes
         FROM bots b JOIN agents g ON g.id = b.agent_id
         WHERE b.lifetime = 'fixed' AND b.archived_at IS NULL ORDER BY b.handle",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(|r| BotSpec {
        handle: r.get("handle"),
        display_name: r.get("display_name"),
        role: r.get("role"),
        agent: r.get("agent"),
        priority: r.get("priority"),
        runtimes: serde_json::from_str(&r.get::<String, _>("runtimes")).unwrap_or_default(),
    })
    .collect();

    let mut memories: Vec<MemorySpec> = sqlx::query(
        "SELECT m.scope, m.name, m.description, m.type, m.body, b.handle
         FROM memories m LEFT JOIN bots b ON m.scope = 'bot:' || b.id
              AND b.lifetime = 'fixed' AND b.archived_at IS NULL
         WHERE m.archived_at IS NULL",
    )
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .filter_map(|r| {
        let scope: String = r.get("scope");
        let scope = if scope == "shared" {
            scope
        } else {
            format!("bot:{}", r.get::<Option<String>, _>("handle")?)
        };
        Some(MemorySpec {
            scope,
            name: r.get("name"),
            description: r.get("description"),
            kind: r.get("type"),
            body: r.get("body"),
        })
    })
    .collect();
    memories.sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));

    Ok(ExportFile {
        format: FORMAT.into(),
        version: VERSION,
        exported_at: Utc::now().to_rfc3339(),
        hub_version: env!("CARGO_PKG_VERSION").into(),
        prefs,
        nodes,
        projects,
        workspaces,
        skills,
        mcp_servers,
        agents,
        snippets,
        hooks,
        approval_rules,
        autopilots,
        bots,
        memories,
    })
}
