use std::path::PathBuf;

use async_trait::async_trait;
use blazar_core_types::{NormalizedEntry, ProviderSessionId, RuntimeKind};
use futures::stream::BoxStream;

pub mod claude;
pub mod cli_runtime;
pub mod codex;
pub mod discover;
pub mod spec;

pub use cli_runtime::CliRuntime;
pub use cli_runtime::{control_json, flag_settings, user_input_line_with_images, valid_style};
pub use discover::{DiscoveredAgent, discover};

pub use spec::{AgentSpec, OutputFormat, ResumeStyle};

#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub cwd: PathBuf,

    pub prompt: String,

    pub model: Option<String>,

    pub permission_mode: Option<String>,

    pub allowed_tools: Vec<String>,

    pub disallowed_tools: Vec<String>,

    pub env: std::collections::BTreeMap<String, String>,

    pub extra_args: Vec<String>,

    pub remote_hands: Option<RemoteHands>,

    pub instructions: Option<String>,

    pub effort: Option<String>,

    pub images: Vec<ImageInput>,

    pub output_style: Option<String>,

    pub fast_mode: Option<bool>,

    pub thinking: Option<bool>,

    pub mcp_servers: Vec<McpServerSpec>,

    pub add_dirs: Vec<String>,

    pub resume_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageInput {
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone)]
pub struct RemoteHands {
    pub node: String,

    pub root: String,

    pub command: String,
    pub args: Vec<String>,
}

pub const REMOTE_HANDS_SERVER: &str = "blazar";

#[must_use]
pub fn supports_remote_hands(id: &str) -> bool {
    matches!(id, "claude" | "codex")
}

impl RemoteHands {
    #[must_use]
    pub fn briefing(&self) -> String {
        format!(
            "你正在操作远端机器 {node} 上的工作区 {root}。\n\
             所有读写文件、执行命令都必须用 {srv} 这个 MCP server 提供的工具（Bash / Read / Write / Edit / Glob / Grep），\n\
             它们在 {node}:{root} 上执行，相对路径相对这个目录。你本机的工作目录只是占位，里面没有代码，不要在本机找文件。",
            node = self.node,
            root = self.root,
            srv = REMOTE_HANDS_SERVER,
        )
    }

    #[must_use]
    pub fn mcp_json(&self) -> String {
        serde_json::json!({ "mcpServers": { REMOTE_HANDS_SERVER: {
            "command": self.command, "args": self.args,
        }}})
        .to_string()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerSpec {
    pub name: String,

    pub http: bool,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub env: std::collections::BTreeMap<String, String>,
    pub headers: std::collections::BTreeMap<String, String>,
}

impl McpServerSpec {
    #[must_use]
    pub fn claude_json(&self) -> serde_json::Value {
        if self.http {
            serde_json::json!({ "type": "http", "url": self.url, "headers": self.headers })
        } else {
            serde_json::json!({ "command": self.command, "args": self.args, "env": self.env })
        }
    }
}

impl SessionSpec {
    pub fn new(cwd: impl Into<PathBuf>, prompt: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            prompt: prompt.into(),
            model: None,
            permission_mode: None,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            env: std::collections::BTreeMap::new(),
            extra_args: Vec::new(),
            remote_hands: None,
            instructions: None,
            effort: None,
            output_style: None,
            fast_mode: None,
            thinking: None,
            images: Vec::new(),
            resume_at: None,
            mcp_servers: Vec::new(),
            add_dirs: Vec::new(),
        }
    }
}

pub struct SessionHandle {
    pub events: BoxStream<'static, NormalizedEntry>,

    pub killer: Option<blazar_transport::ProcessKiller>,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("启动 agent 进程失败: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("agent 进程的 {0} 管道不可用")]
    MissingPipe(&'static str),

    #[error("该 runtime 不支持 {0}")]
    Unsupported(&'static str),

    #[error("传输层失败: {0}")]
    Transport(String),
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    fn kind(&self) -> RuntimeKind;

    fn target(&self) -> &str;

    async fn start(&self, spec: SessionSpec) -> Result<SessionHandle, RuntimeError>;

    async fn resume(
        &self,
        id: &ProviderSessionId,
        spec: SessionSpec,
    ) -> Result<SessionHandle, RuntimeError>;
}
