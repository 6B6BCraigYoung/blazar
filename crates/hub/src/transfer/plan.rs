use std::collections::HashSet;

use blazar_core_types::transfer::{
    ExportFile, ImportAction, ImportItem, ImportReport, WorkspaceRef,
};
use sqlx::SqliteConnection;

use super::{Op, Options, Plan, agent_of, blank};

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
