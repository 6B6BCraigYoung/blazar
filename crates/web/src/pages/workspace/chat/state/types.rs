use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTINUE_TEXT: &str = crate::failure::SWITCHED;
pub const ACC_RUNTIMES: [&str; 2] = ["claude", "codex"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Thread {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_at: Option<String>,
    #[serde(default)]
    pub cause: Option<blazar_core_types::failure::FailureCause>,
    #[serde(default)]
    pub handled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubLoad {
    Loading,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Queued {
    pub id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub images: usize,
    #[serde(default)]
    pub held: Option<String>,
    #[serde(default)]
    pub request: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Starter {
    pub label: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub runtime_label: String,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub starters: Vec<Starter>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub remote_hands: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct CliCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "argumentHint")]
    pub argument_hint: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Catalog {
    pub commands: Vec<CliCommand>,
    pub output_styles: Vec<String>,
    pub output_style: Option<String>,
    pub mcp_servers: Vec<blazar_core_types::McpServerStatus>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ShellRun {
    pub command: String,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub code: i32,
    #[serde(default)]
    pub truncated: bool,
}

impl ShellRun {
    pub fn context(&self) -> String {
        format!(
            "<bash-input>{}</bash-input>\n\n<bash-stdout>{}</bash-stdout><bash-stderr>{}</bash-stderr>",
            self.command,
            self.stdout.trim_end(),
            self.stderr.trim_end()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelSel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default = "yes")]
    pub thinking: bool,
    #[serde(default)]
    pub fast: bool,
    #[serde(default)]
    pub style: String,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            thinking: true,
            fast: false,
            style: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingMsg {
    pub id: u32,
    pub text: String,
    pub images: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attach {
    pub name: String,
    pub media_type: String,
    pub data: String,
    pub url: String,
}

pub type Mode = (&'static str, &'static str, &'static str, &'static str);

pub const MODES_CLAUDE: [Mode; 5] = [
    (
        "default",
        "Manual",
        "Claude will ask for approval before making each edit",
        "hand",
    ),
    (
        "acceptEdits",
        "Edit automatically",
        "Claude will edit files without asking and ask before running commands",
        "code",
    ),
    (
        "plan",
        "Plan",
        "Claude will explore the code and present a plan before editing",
        "plan",
    ),
    (
        "auto",
        "Auto",
        "Claude will approve actions that pass a safety check and pause for anything risky",
        "bolt",
    ),
    (
        "bypassPermissions",
        "Bypass permissions",
        "Claude will not ask about anything. Use with care.",
        "warn",
    ),
];
pub const MODES_CODEX: [Mode; 3] = [
    (
        "read-only",
        "Read Only",
        "Codex can read files and answer questions. Codex requires approval to make edits, run commands, or access network.",
        "plan",
    ),
    (
        "workspace-write",
        "Auto",
        "Codex can read files, make edits, and run commands in the workspace. Codex requires approval to work outside the workspace or access network.",
        "bolt",
    ),
    (
        "danger-full-access",
        "Full Access",
        "Codex can read files, make edits, and run commands with network access, without approval. Exercise caution.",
        "warn",
    ),
];

pub fn default_mode(rt: &str) -> &'static str {
    match rt {
        "codex" => "workspace-write",
        _ => "auto",
    }
}

pub fn modes_for(rt: &str) -> &'static [Mode] {
    match rt {
        "claude" => &MODES_CLAUDE,
        "codex" => &MODES_CODEX,
        _ => &[],
    }
}
