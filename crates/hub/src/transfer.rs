use std::collections::{BTreeMap, HashSet};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use blazar_core_types::transfer::{
    AgentSpec, AutopilotSpec, BotSpec, ExportFile, FORMAT, HookSpec, ImportAction, ImportItem,
    ImportReport, McpSpec, MemorySpec, NodeSpec, PAUSED_REASON, ProjectSpec, RuleSpec,
    SkillFileSpec, SkillSpec, SnippetSpec, VERSION, WorkspaceRef, WorkspaceSpec,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Row, SqliteConnection};

use crate::api::Shared;
use crate::state::ServerEvent;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn refuse_bots(headers: &HeaderMap) -> Option<Response> {
    crate::bots::caller_of(headers).map(|_| fail(StatusCode::FORBIDDEN, "bot 不能导出或导入配置"))
}

fn blank(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.into_keys().map(|k| (k, String::new())).collect()
}

fn keys(raw: &str) -> BTreeMap<String, String> {
    blank(serde_json::from_str(raw).unwrap_or_default())
}

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

pub fn parse(bytes: &[u8]) -> Result<ExportFile, String> {
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("文件不是合法的 JSON：{e}"))?;
    if raw["format"].as_str() != Some(FORMAT) {
        return Err("这不是 Blazar 导出的配置文件".into());
    }
    match raw["version"].as_u64() {
        Some(VERSION) => {}
        Some(v) => {
            return Err(format!(
                "导出文件的格式版本是 {v}，这个 hub 只认 {VERSION}；请升级 Blazar 后再导入"
            ));
        }
        None => return Err("导出文件缺少格式版本".into()),
    }
    serde_json::from_value(raw).map_err(|e| format!("导出文件内容不完整：{e}"))
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub local_node: Option<String>,
    pub bot_home: Option<std::path::PathBuf>,
}

enum Op {
    Prefs(Value),
    Node(NodeSpec),
    Project(String),
    Workspace(WorkspaceSpec),
    Skill(SkillSpec),
    Mcp(McpSpec),
    Agent(AgentSpec),
    Snippet(SnippetSpec),
    Hook(HookSpec),
    Rule(RuleSpec),
    Autopilot(AutopilotSpec),
    Bot(BotSpec),
    Memory(MemorySpec),
}

pub struct Plan {
    pub report: ImportReport,
    ops: Vec<Op>,
    bot_home: Option<std::path::PathBuf>,
}

#[derive(Default)]
struct Planned {
    nodes: HashSet<String>,
    projects: HashSet<String>,
    workspaces: HashSet<WorkspaceRef>,
    skills: HashSet<String>,
    mcp: HashSet<String>,
    agents: HashSet<String>,
    snippets: HashSet<String>,
    hooks: HashSet<(String, String, String, Option<String>)>,
    rules: HashSet<(String, String, Option<WorkspaceRef>)>,
    autopilots: HashSet<(String, WorkspaceRef)>,
    bots: HashSet<String>,
    memories: HashSet<(String, String)>,
}

struct Planner<'c> {
    conn: &'c mut SqliteConnection,
    local_node: Option<String>,
    report: ImportReport,
    ops: Vec<Op>,
    planned: Planned,
}

type Sql<T> = Result<T, sqlx::Error>;

impl Planner<'_> {
    fn item(&mut self, section: &str, key: &str, action: ImportAction, detail: impl Into<String>) {
        let imported = action == ImportAction::Create;
        self.report.push(ImportItem {
            section: section.into(),
            key: key.into(),
            action,
            imported,
            detail: detail.into(),
        });
    }

    fn create(&mut self, section: &str, key: &str, detail: impl Into<String>, op: Op) {
        self.item(section, key, ImportAction::Create, detail);
        self.ops.push(op);
    }

    fn skip(&mut self, section: &str, key: &str, detail: impl Into<String>) {
        self.item(section, key, ImportAction::Skip, detail);
    }

    fn refuse(&mut self, section: &str, key: &str, detail: impl Into<String>) {
        self.item(section, key, ImportAction::Attention, detail);
    }

    fn create_with_attention(&mut self, section: &str, key: &str, detail: String, op: Op) {
        self.report.push(ImportItem {
            section: section.into(),
            key: key.into(),
            action: ImportAction::Attention,
            imported: true,
            detail,
        });
        self.ops.push(op);
    }

    async fn count(&mut self, sql: &str, binds: &[Option<&str>]) -> Sql<bool> {
        let mut q = sqlx::query_scalar::<_, i64>(sql);
        for b in binds {
            q = q.bind(*b);
        }
        Ok(q.fetch_one(&mut *self.conn).await? > 0)
    }

    fn map_ref(&self, r: &WorkspaceRef) -> Result<WorkspaceRef, String> {
        if r.node == "local" {
            match &self.local_node {
                Some(n) => Ok(WorkspaceRef {
                    node: n.clone(),
                    path: r.path.clone(),
                }),
                None => Err(format!(
                    "{} 在原 hub 的本机上；要导入就用 blazar import --local-node <节点名> 指定它在新 hub 上属于哪台机器",
                    r.path
                )),
            }
        } else {
            Ok(r.clone())
        }
    }

    async fn node_available(&mut self, name: &str) -> Sql<bool> {
        Ok(self.planned.nodes.contains(name)
            || self
                .count("SELECT COUNT(*) FROM nodes WHERE name = ?1", &[Some(name)])
                .await?)
    }

    async fn workspace_id(&mut self, r: &WorkspaceRef) -> Sql<Option<String>> {
        sqlx::query_scalar(
            "SELECT w.id FROM workspaces w JOIN nodes n ON n.id = w.node_id
             WHERE n.name = ?1 AND w.path = ?2",
        )
        .bind(&r.node)
        .bind(&r.path)
        .fetch_optional(&mut *self.conn)
        .await
    }

    async fn resolve_ref(
        &mut self,
        r: &WorkspaceRef,
    ) -> Result<(WorkspaceRef, Option<String>), String> {
        let mapped = self.map_ref(r)?;
        if self.planned.workspaces.contains(&mapped) {
            return Ok((mapped, None));
        }
        match self.workspace_id(&mapped).await {
            Ok(Some(id)) => Ok((mapped, Some(id))),
            Ok(None) => Err(format!(
                "工作区 {}:{} 不在新 hub 上，也没有随这次导入新建",
                mapped.node, mapped.path
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn agent_available(&mut self, name: &str) -> Sql<bool> {
        Ok(self.planned.agents.contains(name)
            || self
                .count(
                    "SELECT COUNT(*) FROM agents WHERE name = ?1 AND archived_at IS NULL",
                    &[Some(name)],
                )
                .await?)
    }

    async fn plan(&mut self, file: &ExportFile) -> Sql<()> {
        if let Some(local) = self.local_node.clone()
            && !self.node_available(&local).await?
            && !file.nodes.iter().any(|n| n.name == local)
        {
            self.report.notes.push(format!(
                "--local-node 指定的节点「{local}」在新 hub 上不存在，本机工作区无法映射"
            ));
        }
        self.prefs(file).await?;
        self.nodes(file).await?;
        self.projects(file).await?;
        self.workspaces(file).await?;
        self.skills(file).await?;
        self.mcp(file).await?;
        self.agents(file).await?;
        self.snippets(file).await?;
        self.hooks(file).await?;
        self.rules(file).await?;
        self.autopilots(file).await?;
        self.bots(file).await?;
        self.memories(file).await?;
        if self.ops.iter().any(|o| matches!(o, Op::Autopilot(_))) {
            self.report.notes.push(
                "导入的自动化都是暂停的：先在原 hub 上停掉它们，再到这里启用，免得两边重复跑"
                    .into(),
            );
        }
        if self.ops.iter().any(|o| matches!(o, Op::Hook(_))) {
            self.report
                .notes
                .push("导入的钩子都是停用的：确认命令在这台机器上能跑再启用".into());
        }
        self.report
            .notes
            .push("账号和会话历史不迁移：在这台机器上重新登录账号，再把清空的密钥值填回去".into());
        Ok(())
    }

    async fn prefs(&mut self, file: &ExportFile) -> Sql<()> {
        let Some(prefs) = file.prefs.clone().filter(|p| !p.is_null()) else {
            return Ok(());
        };
        if self
            .count("SELECT COUNT(*) FROM kv_settings WHERE key = 'prefs'", &[])
            .await?
        {
            self.skip("prefs", "prefs", "新 hub 已有偏好设置，不覆盖");
        } else {
            self.create("prefs", "prefs", "", Op::Prefs(prefs));
        }
        Ok(())
    }

    async fn nodes(&mut self, file: &ExportFile) -> Sql<()> {
        for n in &file.nodes {
            let key = n.name.trim();
            if key.is_empty()
                || key == "local"
                || key.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                self.refuse("nodes", key, "节点名不合法");
                continue;
            }
            if n.role
                .as_deref()
                .is_some_and(|r| !matches!(r, "dev" | "personal"))
            {
                self.refuse("nodes", key, "节点角色只能是 dev 或 personal");
                continue;
            }
            if self.planned.nodes.contains(key) {
                self.skip("nodes", key, "文件里重复");
            } else if self
                .count("SELECT COUNT(*) FROM nodes WHERE name = ?1", &[Some(key)])
                .await?
            {
                self.skip("nodes", key, "已存在");
            } else {
                self.planned.nodes.insert(key.to_owned());
                let detail = if n.transport == "ssh" {
                    format!("hub 机器要能 ssh 到 {key}")
                } else {
                    String::new()
                };
                self.create("nodes", key, detail, Op::Node(n.clone()));
            }
        }
        Ok(())
    }

    async fn projects(&mut self, file: &ExportFile) -> Sql<()> {
        for p in &file.projects {
            let key = p.name.trim();
            if key.is_empty() {
                self.refuse("projects", key, "项目名为空");
            } else if self.planned.projects.contains(key) {
                self.skip("projects", key, "文件里重复");
            } else if self
                .count(
                    "SELECT COUNT(*) FROM projects WHERE name = ?1",
                    &[Some(key)],
                )
                .await?
            {
                self.skip("projects", key, "已存在");
            } else {
                self.planned.projects.insert(key.to_owned());
                self.create("projects", key, "", Op::Project(key.to_owned()));
            }
        }
        Ok(())
    }

    async fn workspaces(&mut self, file: &ExportFile) -> Sql<()> {
        for w in &file.workspaces {
            let key = format!("{}:{}", w.node, w.path);
            if !w.path.starts_with('/') && !w.path.starts_with('~') {
                self.refuse("workspaces", &key, "路径要是绝对路径");
                continue;
            }
            let mapped = match self.map_ref(&WorkspaceRef {
                node: w.node.clone(),
                path: w.path.clone(),
            }) {
                Ok(m) => m,
                Err(e) => {
                    self.refuse("workspaces", &key, e);
                    continue;
                }
            };
            let key = format!("{}:{}", mapped.node, mapped.path);
            if !self.node_available(&mapped.node).await? {
                self.refuse(
                    "workspaces",
                    &key,
                    format!("节点「{}」在新 hub 上不存在", mapped.node),
                );
                continue;
            }
            if self.planned.workspaces.contains(&mapped) {
                self.skip("workspaces", &key, "文件里重复");
                continue;
            }
            if self.workspace_id(&mapped).await?.is_some() {
                self.skip("workspaces", &key, "已存在");
                continue;
            }
            self.planned.workspaces.insert(mapped.clone());
            let mut spec = w.clone();
            spec.node = mapped.node;
            let project_missing = match spec.project.as_deref() {
                Some(p) => {
                    !self.planned.projects.contains(p)
                        && !self
                            .count("SELECT COUNT(*) FROM projects WHERE name = ?1", &[Some(p)])
                            .await?
                }
                None => false,
            };
            if project_missing {
                let detail = format!(
                    "项目「{}」不存在，工作区不归入项目",
                    spec.project.take().unwrap_or_default()
                );
                self.create_with_attention("workspaces", &key, detail, Op::Workspace(spec));
            } else {
                self.create("workspaces", &key, "", Op::Workspace(spec));
            }
        }
        Ok(())
    }

    async fn skills(&mut self, file: &ExportFile) -> Sql<()> {
        for s in &file.skills {
            let key = s.name.trim();
            if !crate::library::valid_name(key) {
                self.refuse("skills", key, "skill 名不合法");
                continue;
            }
            if let Some(bad) = s.files.iter().find(|f| !crate::library::valid_rel(&f.path)) {
                self.refuse("skills", key, format!("文件路径不合法：{}", bad.path));
                continue;
            }
            let lower = key.to_lowercase();
            if self.planned.skills.contains(&lower) {
                self.skip("skills", key, "文件里重复");
            } else if self
                .count(
                    "SELECT COUNT(*) FROM skills WHERE name = ?1 COLLATE NOCASE",
                    &[Some(key)],
                )
                .await?
            {
                self.skip("skills", key, "已存在");
            } else {
                self.planned.skills.insert(lower);
                self.create("skills", key, "", Op::Skill(s.clone()));
            }
        }
        Ok(())
    }

    async fn mcp(&mut self, file: &ExportFile) -> Sql<()> {
        for m in &file.mcp_servers {
            let key = m.name.trim();
            let body = crate::library::McpBody {
                name: m.name.clone(),
                description: m.description.clone(),
                transport: m.transport.clone(),
                command: m.command.clone(),
                args: m.args.clone(),
                url: m.url.clone(),
                env: Some(m.env.clone()),
                headers: Some(m.headers.clone()),
            };
            if let Err(e) = crate::library::check_mcp(&body) {
                self.refuse("mcp_servers", key, e);
                continue;
            }
            let lower = key.to_lowercase();
            if self.planned.mcp.contains(&lower) {
                self.skip("mcp_servers", key, "文件里重复");
                continue;
            }
            if self
                .count(
                    "SELECT COUNT(*) FROM mcp_servers WHERE name = ?1 COLLATE NOCASE",
                    &[Some(key)],
                )
                .await?
            {
                self.skip("mcp_servers", key, "已存在");
                continue;
            }
            self.planned.mcp.insert(lower);
            let mut secrets = Vec::new();
            if !m.env.is_empty() {
                secrets.push(format!(
                    "env {}",
                    m.env.keys().cloned().collect::<Vec<_>>().join("、")
                ));
            }
            if !m.headers.is_empty() {
                secrets.push(format!(
                    "headers {}",
                    m.headers.keys().cloned().collect::<Vec<_>>().join("、")
                ));
            }
            let mut spec = m.clone();
            spec.env = blank(spec.env);
            spec.headers = blank(spec.headers);
            if secrets.is_empty() {
                self.create("mcp_servers", key, "", Op::Mcp(spec));
            } else {
                let detail = format!("导入后重新填写密钥值：{}", secrets.join("；"));
                self.create_with_attention("mcp_servers", key, detail, Op::Mcp(spec));
            }
        }
        Ok(())
    }

    async fn agents(&mut self, file: &ExportFile) -> Sql<()> {
        for a in &file.agents {
            let key = a.name.trim().to_owned();
            let agent = match agent_of(a) {
                Ok(agent) => agent,
                Err(e) => {
                    self.refuse("agents", &key, e);
                    continue;
                }
            };
            if self.planned.agents.contains(&agent.name) {
                self.skip("agents", &key, "文件里重复");
                continue;
            }
            if self
                .count(
                    "SELECT COUNT(*) FROM agents WHERE name = ?1 AND archived_at IS NULL",
                    &[Some(&agent.name)],
                )
                .await?
            {
                self.skip("agents", &key, "已存在");
                continue;
            }
            self.planned.agents.insert(agent.name.clone());
            let mut notes = Vec::new();
            let mut spec = a.clone();
            spec.custom_env = blank(spec.custom_env);
            let mut skills = Vec::new();
            for s in &a.skills {
                if self.planned.skills.contains(&s.to_lowercase())
                    || self
                        .count(
                            "SELECT COUNT(*) FROM skills WHERE name = ?1 COLLATE NOCASE",
                            &[Some(s)],
                        )
                        .await?
                {
                    skills.push(s.clone());
                } else {
                    notes.push(format!("skill「{s}」不存在，没挂上"));
                }
            }
            let mut servers = Vec::new();
            for m in &a.mcp_servers {
                if self.planned.mcp.contains(&m.to_lowercase())
                    || self
                        .count(
                            "SELECT COUNT(*) FROM mcp_servers WHERE name = ?1 COLLATE NOCASE",
                            &[Some(m)],
                        )
                        .await?
                {
                    servers.push(m.clone());
                } else {
                    notes.push(format!("MCP「{m}」不存在，没挂上"));
                }
            }
            spec.skills = skills;
            spec.mcp_servers = servers;
            if !spec.custom_env.is_empty() {
                notes.push(format!(
                    "重新填写环境变量的值：{}",
                    spec.custom_env
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("、")
                ));
            }
            if notes.is_empty() {
                self.create("agents", &key, "", Op::Agent(spec));
            } else {
                self.create_with_attention("agents", &key, notes.join("；"), Op::Agent(spec));
            }
        }
        Ok(())
    }

    async fn snippets(&mut self, file: &ExportFile) -> Sql<()> {
        for s in &file.snippets {
            let key = s.name.trim();
            if let Err(e) = crate::snippets::check(&s.name, &s.body) {
                self.refuse("snippets", key, e);
                continue;
            }
            let lower = key.to_lowercase();
            if self.planned.snippets.contains(&lower) {
                self.skip("snippets", key, "文件里重复");
            } else if self
                .count(
                    "SELECT COUNT(*) FROM snippets WHERE name = ?1 COLLATE NOCASE",
                    &[Some(key)],
                )
                .await?
            {
                self.skip("snippets", key, "已存在");
            } else {
                self.planned.snippets.insert(lower);
                self.create("snippets", key, "", Op::Snippet(s.clone()));
            }
        }
        Ok(())
    }

    async fn hooks(&mut self, file: &ExportFile) -> Sql<()> {
        for h in &file.hooks {
            let key = format!("{}：{}", h.event, h.command);
            let input = crate::api::HookInput {
                event: h.event.clone(),
                command: h.command.clone(),
                target: h.target.clone(),
                matcher: h.matcher.clone(),
                blocking: h.blocking,
                timeout_secs: h.timeout_secs,
                enabled: false,
            };
            if let Err(e) = input.validate() {
                self.refuse("hooks", &key, e);
                continue;
            }
            let natural = (
                h.event.clone(),
                h.command.clone(),
                h.target.clone(),
                h.matcher.clone(),
            );
            if self.planned.hooks.contains(&natural) {
                self.skip("hooks", &key, "文件里重复");
            } else if self
                .count(
                    "SELECT COUNT(*) FROM hooks WHERE event = ?1 AND command = ?2 AND target = ?3
                       AND matcher IS ?4",
                    &[
                        Some(&h.event),
                        Some(&h.command),
                        Some(&h.target),
                        h.matcher.as_deref(),
                    ],
                )
                .await?
            {
                self.skip("hooks", &key, "已存在");
            } else {
                self.planned.hooks.insert(natural);
                self.create("hooks", &key, "导入后停用", Op::Hook(h.clone()));
            }
        }
        Ok(())
    }

    async fn rules(&mut self, file: &ExportFile) -> Sql<()> {
        for r in &file.approval_rules {
            let scope = r
                .workspace
                .as_ref()
                .map_or_else(|| "全局".to_owned(), |w| format!("{}:{}", w.node, w.path));
            let key = format!("{} {} @ {scope}", r.tool, r.pattern);
            let body = crate::rules::RuleBody {
                tool: r.tool.clone(),
                pattern: r.pattern.clone(),
                workspace_id: None,
            };
            if let Err(e) = crate::rules::check(&body) {
                self.refuse("approval_rules", &key, e);
                continue;
            }
            let (workspace, ws_id) = match &r.workspace {
                None => (None, None),
                Some(w) => match self.resolve_ref(w).await {
                    Ok((mapped, id)) => (Some(mapped), id),
                    Err(e) => {
                        self.refuse("approval_rules", &key, e);
                        continue;
                    }
                },
            };
            let natural = (
                r.tool.trim().to_owned(),
                r.pattern.trim().to_owned(),
                workspace.clone(),
            );
            if self.planned.rules.contains(&natural) {
                self.skip("approval_rules", &key, "文件里重复");
                continue;
            }
            let exists = if workspace.is_some() && ws_id.is_none() {
                false
            } else {
                self.count(
                    "SELECT COUNT(*) FROM approval_rules
                     WHERE tool = ?1 AND pattern = ?2 AND workspace_id IS ?3",
                    &[
                        Some(r.tool.trim()),
                        Some(r.pattern.trim()),
                        ws_id.as_deref(),
                    ],
                )
                .await?
            };
            if exists {
                self.skip("approval_rules", &key, "已存在");
            } else {
                self.planned.rules.insert(natural);
                let mut spec = r.clone();
                spec.workspace = workspace;
                self.create("approval_rules", &key, "", Op::Rule(spec));
            }
        }
        Ok(())
    }

    async fn autopilots(&mut self, file: &ExportFile) -> Sql<()> {
        for a in &file.autopilots {
            let key = a.name.trim().to_owned();
            let body = crate::autopilot::Body {
                name: Some(a.name.clone()),
                instructions: Some(a.instructions.clone()),
                mode: Some(a.mode.clone()),
                cron: a.cron.clone(),
                timezone: Some(a.timezone.clone()),
                concurrency: Some(a.concurrency.clone()),
                ..crate::autopilot::Body::default()
            };
            if let Err(e) = crate::autopilot::validate(&body) {
                self.refuse("autopilots", &key, e);
                continue;
            }
            if a.instructions.trim().is_empty() {
                self.refuse("autopilots", &key, "指令为空");
                continue;
            }
            let Some(w) = &a.workspace else {
                self.refuse("autopilots", &key, "原自动化没有工作区");
                continue;
            };
            let (workspace, ws_id) = match self.resolve_ref(w).await {
                Ok(v) => v,
                Err(e) => {
                    self.refuse("autopilots", &key, e);
                    continue;
                }
            };
            let natural = (key.clone(), workspace.clone());
            if self.planned.autopilots.contains(&natural) {
                self.skip("autopilots", &key, "文件里重复");
                continue;
            }
            if let Some(id) = &ws_id
                && self
                    .count(
                        "SELECT COUNT(*) FROM autopilots WHERE name = ?1 AND workspace_id = ?2",
                        &[Some(&key), Some(id)],
                    )
                    .await?
            {
                self.skip("autopilots", &key, "已存在");
                continue;
            }
            self.planned.autopilots.insert(natural);
            let mut spec = a.clone();
            spec.workspace = Some(workspace);
            let paused = "导入后暂停；先在原 hub 上停掉它，再到这里启用";
            match spec.agent.clone() {
                Some(agent) if !self.agent_available(&agent).await? => {
                    spec.agent = None;
                    let detail = format!("{paused}；智能体档案「{agent}」不存在，已清空");
                    self.create_with_attention("autopilots", &key, detail, Op::Autopilot(spec));
                }
                _ => self.create("autopilots", &key, paused, Op::Autopilot(spec)),
            }
        }
        Ok(())
    }

    async fn bots(&mut self, file: &ExportFile) -> Sql<()> {
        for b in &file.bots {
            let key = b.handle.trim().to_owned();
            if !crate::bots::valid_handle(&key) {
                self.refuse("bots", &key, "handle 不合法");
                continue;
            }
            if !crate::bots::ROLES.contains(&b.role.as_str())
                || !crate::bots::PRIORITIES.contains(&b.priority.as_str())
            {
                self.refuse("bots", &key, "角色或优先级不认识");
                continue;
            }
            if self.planned.bots.contains(&key) {
                self.skip("bots", &key, "文件里重复");
                continue;
            }
            let existing: Option<Option<String>> =
                sqlx::query_scalar("SELECT archived_at FROM bots WHERE handle = ?1")
                    .bind(&key)
                    .fetch_optional(&mut *self.conn)
                    .await?;
            match existing {
                Some(None) => {
                    self.skip("bots", &key, "已存在");
                    continue;
                }
                Some(Some(_)) => {
                    self.refuse("bots", &key, "这个 handle 被一个已归档的 bot 占着");
                    continue;
                }
                None => {}
            }
            if !self.agent_available(&b.agent).await? {
                self.refuse("bots", &key, format!("智能体档案「{}」不存在", b.agent));
                continue;
            }
            self.planned.bots.insert(key.clone());
            self.create(
                "bots",
                &key,
                "在 hub 机器上新建家工作区",
                Op::Bot(b.clone()),
            );
        }
        Ok(())
    }

    async fn memories(&mut self, file: &ExportFile) -> Sql<()> {
        for m in &file.memories {
            let key = format!("{}/{}", m.scope, m.name);
            if !crate::bots::memory::valid_name(&m.name)
                || m.description.trim().is_empty()
                || m.description.chars().count() > 300
                || m.body.trim().is_empty()
                || m.body.chars().count() > 20_000
                || !crate::bots::memory::TYPES.contains(&m.kind.as_str())
            {
                self.refuse("memories", &key, "内容不合法");
                continue;
            }
            let scope = if m.scope == "shared" {
                Some("shared".to_owned())
            } else if let Some(handle) = m.scope.strip_prefix("bot:") {
                if self.planned.bots.contains(handle) {
                    None
                } else {
                    let id: Option<String> = sqlx::query_scalar(
                        "SELECT id FROM bots WHERE handle = ?1 AND archived_at IS NULL",
                    )
                    .bind(handle)
                    .fetch_optional(&mut *self.conn)
                    .await?;
                    match id {
                        Some(id) => Some(format!("bot:{id}")),
                        None => {
                            self.refuse("memories", &key, format!("bot「{handle}」没有导入"));
                            continue;
                        }
                    }
                }
            } else {
                self.refuse("memories", &key, "作用域不认识");
                continue;
            };
            let natural = (m.scope.clone(), m.name.clone());
            if self.planned.memories.contains(&natural) {
                self.skip("memories", &key, "文件里重复");
                continue;
            }
            if let Some(scope) = &scope
                && self
                    .count(
                        "SELECT COUNT(*) FROM memories WHERE scope = ?1 AND name = ?2",
                        &[Some(scope), Some(&m.name)],
                    )
                    .await?
            {
                self.skip("memories", &key, "已存在");
                continue;
            }
            self.planned.memories.insert(natural);
            self.create("memories", &key, "", Op::Memory(m.clone()));
        }
        Ok(())
    }
}

fn agent_of(a: &AgentSpec) -> Result<crate::agents::Agent, String> {
    let agent = crate::agents::normalize(crate::agents::Agent {
        id: String::new(),
        name: a.name.clone(),
        runtime: a.runtime.clone(),
        description: a.description.clone(),
        instructions: a.instructions.clone(),
        model: a.model.clone(),
        permission_mode: a.permission_mode.clone(),
        env: blank(a.custom_env.clone()),
        color: a.color.clone(),
        avatar: a.avatar.clone(),
        starters: if a.starters.is_null() {
            Vec::new()
        } else {
            serde_json::from_value(a.starters.clone()).map_err(|_| "开场白格式不对".to_owned())?
        },
        thinking_level: a.thinking_level.clone(),
        custom_args: a.custom_args.clone(),
        max_concurrent: a.max_concurrent.unwrap_or(1),
        account: None,
        archived_at: None,
        created_at: String::new(),
        updated_at: String::new(),
    });
    crate::agents::validate(&agent)?;
    Ok(agent)
}

pub async fn plan(
    conn: &mut SqliteConnection,
    file: &ExportFile,
    opts: &Options,
) -> Result<Plan, String> {
    let mut planner = Planner {
        conn,
        local_node: opts
            .local_node
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned),
        report: ImportReport {
            dry_run: true,
            ..ImportReport::default()
        },
        ops: Vec::new(),
        planned: Planned::default(),
    };
    planner.plan(file).await.map_err(|e| e.to_string())?;
    Ok(Plan {
        report: planner.report,
        ops: planner.ops,
        bot_home: opts.bot_home.clone(),
    })
}

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

pub async fn export_handler(State(st): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(r) = refuse_bots(&headers) {
        return r;
    }
    let file = async {
        let mut tx = st.db.pool().begin().await?;
        export_from(&mut tx).await
    }
    .await;
    let file = match file {
        Ok(f) => f,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let body = match serde_json::to_vec_pretty(&file) {
        Ok(b) => b,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let name = format!(
        "blazar-export-{}.json",
        chrono::Local::now().format("%Y%m%d")
    );
    (
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct ImportQuery {
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub local_node: Option<String>,
}

pub async fn import_handler(
    State(st): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> Response {
    if let Some(r) = refuse_bots(&headers) {
        return r;
    }
    let file = match parse(&body) {
        Ok(f) => f,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let opts = Options {
        local_node: q.local_node,
        bot_home: None,
    };
    match import(&st, &file, &opts, q.dry_run.unwrap_or(true)).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
