use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FORMAT: &str = "blazar-export";
pub const VERSION: u64 = 1;
pub const MAX_IMPORT_BYTES: usize = 32 * 1024 * 1024;
pub const PAUSED_REASON: &str = "导入后待启用";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportFile {
    pub format: String,
    pub version: u64,
    pub exported_at: String,
    pub hub_version: String,
    #[serde(default)]
    pub prefs: Option<Value>,
    #[serde(default)]
    pub nodes: Vec<NodeSpec>,
    #[serde(default)]
    pub projects: Vec<ProjectSpec>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSpec>,
    #[serde(default)]
    pub skills: Vec<SkillSpec>,
    #[serde(default)]
    pub mcp_servers: Vec<McpSpec>,
    #[serde(default)]
    pub agents: Vec<AgentSpec>,
    #[serde(default)]
    pub snippets: Vec<SnippetSpec>,
    #[serde(default)]
    pub hooks: Vec<HookSpec>,
    #[serde(default)]
    pub approval_rules: Vec<RuleSpec>,
    #[serde(default)]
    pub autopilots: Vec<AutopilotSpec>,
    #[serde(default)]
    pub bots: Vec<BotSpec>,
    #[serde(default)]
    pub memories: Vec<MemorySpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub node: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeSpec {
    pub name: String,
    pub transport: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub labels: Value,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectSpec {
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub node: String,
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub target_branch: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillFileSpec {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub files: Vec<SkillFileSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub transport: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub name: String,
    pub runtime: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub custom_env: BTreeMap<String, String>,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub starters: Value,
    #[serde(default)]
    pub thinking_level: Option<String>,
    #[serde(default)]
    pub custom_args: Vec<String>,
    #[serde(default)]
    pub max_concurrent: Option<i64>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SnippetSpec {
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookSpec {
    pub event: String,
    pub command: String,
    pub target: String,
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub blocking: bool,
    pub timeout_secs: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleSpec {
    pub tool: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub workspace: Option<WorkspaceRef>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AutopilotSpec {
    pub name: String,
    pub instructions: String,
    #[serde(default)]
    pub workspace: Option<WorkspaceRef>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    pub mode: String,
    #[serde(default)]
    pub title_template: Option<String>,
    #[serde(default)]
    pub cron: Option<String>,
    pub timezone: String,
    pub concurrency: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BotSpec {
    pub handle: String,
    pub display_name: String,
    pub role: String,
    pub agent: String,
    pub priority: String,
    #[serde(default)]
    pub runtimes: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemorySpec {
    pub scope: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportAction {
    Create,
    Skip,
    Attention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportItem {
    pub section: String,
    pub key: String,
    pub action: ImportAction,
    pub imported: bool,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportCounts {
    pub create: usize,
    pub skip: usize,
    pub attention: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub applied: bool,
    pub counts: ImportCounts,
    pub items: Vec<ImportItem>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl ImportReport {
    pub fn push(&mut self, item: ImportItem) {
        match item.action {
            ImportAction::Create => self.counts.create += 1,
            ImportAction::Skip => self.counts.skip += 1,
            ImportAction::Attention => self.counts.attention += 1,
        }
        self.items.push(item);
    }
}

#[must_use]
pub fn section_label(section: &str) -> &str {
    match section {
        "prefs" => "偏好",
        "nodes" => "节点",
        "projects" => "项目",
        "workspaces" => "工作区",
        "skills" => "Skills",
        "mcp_servers" => "MCP",
        "agents" => "智能体档案",
        "snippets" => "片段",
        "hooks" => "钩子",
        "approval_rules" => "审批规则",
        "autopilots" => "自动化",
        "bots" => "bot",
        "memories" => "memory",
        other => other,
    }
}
