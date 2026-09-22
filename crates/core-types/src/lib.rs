pub mod sanitize;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {

            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            fn from(v: Uuid) -> Self {
                Self(v)
            }
        }
    };
}

id_type!(NodeId);
id_type!(ProjectId);
id_type!(WorkspaceId);
id_type!(SessionId);
id_type!(ApprovalId);

impl ApprovalId {
    #[must_use]
    pub fn from_provider(request_id: &str) -> Self {
        const NS: Uuid = Uuid::from_u128(0x6b0f_7a4e_1c2d_4e5f_9a8b_7c6d_5e4f_3a2b);
        Self(Uuid::new_v5(&NS, request_id.as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSessionId(pub String);

impl std::fmt::Display for ProviderSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    OpenAi,
    Google,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    ClaudeCli,

    CodexAppServer,
}

impl RuntimeKind {
    #[must_use]
    pub fn provider(self) -> Provider {
        match self {
            Self::ClaudeCli => Provider::Anthropic,
            Self::CodexAppServer => Provider::OpenAi,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedEntry {
    pub seq: u64,
    pub ts: DateTime<Utc>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<ToolId>,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl McpServerStatus {
    #[must_use]
    pub fn list_from(v: Option<&serde_json::Value>) -> Vec<Self> {
        let s = |x: &serde_json::Value, k: &str| {
            x.get(k)
                .and_then(serde_json::Value::as_str)
                .map(|s| crate::sanitize::truncate_utf8(s, 120))
        };
        v.and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some(Self {
                            name: s(x, "name")?,
                            status: s(x, "status").unwrap_or_else(|| "unknown".into()),
                            source: s(x, "source"),
                        })
                    })
                    .take(200)
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
    SessionStarted {
        provider_session_id: ProviderSessionId,
        model: Option<String>,
        cwd: Option<String>,
        permission_mode: Option<String>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_style: Option<String>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        fast_mode: Option<String>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        fast_mode_reason: Option<String>,

        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mcp_servers: Vec<McpServerStatus>,
    },
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: ToolId,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        id: ToolId,
        ok: bool,

        content: String,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured: Option<serde_json::Value>,
    },

    Approval {
        id: ApprovalId,
        request: serde_json::Value,
    },

    ApprovalResolved {
        id: ApprovalId,
        decision: ApprovalDecision,
    },

    InputConsumed {
        text: String,
    },
    TokenUsage(TokenUsage),
    RateLimit(RateLimit),
    Error {
        message: String,
    },

    BackgroundTask {
        task_id: String,

        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_type: Option<String>,
    },

    Finished(Outcome),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,

    AutoAllowed {
        #[serde(default)]
        rule: String,
    },
    Deny {
        #[serde(default)]
        message: String,
    },

    Cancelled,

    Aborted,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_creation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    pub provider: Provider,

    pub allowed: bool,

    pub windows: Vec<RateLimitWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitWindow {
    pub name: String,

    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Success {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,

        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        denied: Vec<String>,
    },
    Failed {
        message: String,
    },
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    AwaitingApproval,

    Errored,

    Completed,

    Running,

    Idle,
}

impl ActivityState {
    #[must_use]
    pub fn from_entry(kind: &EntryKind) -> Option<Self> {
        match kind {
            EntryKind::Approval { .. } => Some(Self::AwaitingApproval),

            EntryKind::ApprovalResolved { .. } | EntryKind::InputConsumed { .. } => {
                Some(Self::Running)
            }
            EntryKind::Error { .. } => Some(Self::Errored),
            EntryKind::Finished(Outcome::Failed { .. }) => Some(Self::Errored),

            EntryKind::Finished(Outcome::Success { denied, .. }) if !denied.is_empty() => {
                Some(Self::Errored)
            }
            EntryKind::Finished(_) => Some(Self::Completed),

            EntryKind::BackgroundTask { status, .. }
                if matches!(status.as_str(), "killed" | "stopped" | "failed") =>
            {
                Some(Self::Errored)
            }
            EntryKind::BackgroundTask { .. } => None,
            EntryKind::SessionStarted { .. }
            | EntryKind::UserMessage { .. }
            | EntryKind::AssistantMessage { .. }
            | EntryKind::Thinking { .. }
            | EntryKind::ToolUse { .. }
            | EntryKind::ToolResult { .. } => Some(Self::Running),
            EntryKind::TokenUsage(_) | EntryKind::RateLimit(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_time_ordered() {
        let a = WorkspaceId::new();
        let b = WorkspaceId::new();
        assert!(a < b, "v7 应按生成时间有序，利于索引局部性");
    }

    #[test]
    fn awaiting_approval_outranks_running() {
        assert!(ActivityState::AwaitingApproval < ActivityState::Running);
        assert!(ActivityState::Errored < ActivityState::Completed);
    }

    #[test]
    fn usage_events_do_not_change_activity() {
        assert!(ActivityState::from_entry(&EntryKind::TokenUsage(TokenUsage::default())).is_none());
    }

    #[test]
    fn entry_kind_roundtrips() {
        let kind = EntryKind::ToolUse {
            id: ToolId("toolu_1".into()),
            name: "Read".into(),
            input: serde_json::json!({ "file_path": "/tmp/a.txt" }),
        };
        let json = serde_json::to_string(&kind).unwrap();
        assert!(json.contains(r#""type":"tool_use""#));
        let back: EntryKind = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, EntryKind::ToolUse { .. }));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    QuotaLimit,

    AuthExpired,

    NetworkUnreachable,

    NodeUnreachable,

    ApprovalTimeout,

    Interrupted,

    MissingDependency,

    AgentError,

    Unknown,
}

impl FailureClass {
    #[must_use]
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::NetworkUnreachable | Self::NodeUnreachable)
    }

    #[must_use]
    pub fn should_switch_account(self) -> bool {
        matches!(self, Self::QuotaLimit | Self::AuthExpired)
    }

    #[must_use]
    pub fn should_switch_node(self) -> bool {
        matches!(self, Self::NodeUnreachable | Self::MissingDependency)
    }

    #[must_use]
    pub fn from_claude(error_kind: Option<&str>, subtype: Option<&str>) -> Option<Self> {
        let by_kind = error_kind.and_then(|k| {
            Some(match k {
                "authentication_failed"
                | "oauth_org_not_allowed"
                | "account_on_hold"
                | "cloud_credential_error"
                | "invalid_api_key" => Self::AuthExpired,
                "rate_limit" | "billing_error" | "quota_exceeded" | "overloaded_error" => {
                    Self::QuotaLimit
                }
                "network_error" | "connection_error" | "timeout" => Self::NetworkUnreachable,
                _ => return None,
            })
        });
        by_kind.or_else(|| {
            Some(match subtype? {
                "error_max_budget_usd" | "error_max_turns" => Self::QuotaLimit,
                "error_during_execution" | "error_max_structured_output_retries" => {
                    Self::AgentError
                }
                _ => return None,
            })
        })
    }

    #[must_use]
    pub fn classify(text: &str) -> Self {
        let head = text.split(['：', ':', ' ']).next().unwrap_or("");
        if let Some(c) = Self::from_claude(Some(head), Some(head)) {
            return c;
        }
        let t = text.to_ascii_lowercase();
        let has = |pats: &[&str]| pats.iter().any(|p| t.contains(p));

        if t.contains("ssh:") {
            return Self::NodeUnreachable;
        }

        if has(&[
            "rate limit",
            "rate_limit",
            "quota",
            "429",
            "usage limit",
            "限流",
        ]) {
            Self::QuotaLimit
        } else if has(&[
            "401",
            "unauthorized",
            "invalid api key",
            "token expired",
            "authentication",
        ]) {
            Self::AuthExpired
        } else if has(&[
            "connection refused",
            "could not resolve",
            "network is unreachable",
            "timed out",
            "timeout",
            "403 forbidden",
            "tls handshake",
        ]) {
            Self::NetworkUnreachable
        } else if has(&[
            "no route to host",
            "host key verification",
            "permission denied (publickey",
            "ssh: connect",
            "broken pipe",
            "hostname contains invalid characters",
            "could not resolve hostname",
            "机器连不上",
            "connection closed by remote host",
        ]) {
            Self::NodeUnreachable
        } else if has(&["requires approval", "permission denied by user", "审批"]) {
            Self::ApprovalTimeout
        } else if has(&["interrupted", "cancelled", "canceled", "sigint"]) {
            Self::Interrupted
        } else if has(&[
            "command not found",
            "no such file or directory",
            "not installed",
            "cannot execute",
            "未找到命令",
            "没有那个文件或目录",
            "无法执行",
        ]) {
            Self::MissingDependency
        } else if t.is_empty() {
            Self::Unknown
        } else {
            Self::AgentError
        }
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    #[test]
    fn quota_is_not_retryable_but_switches_account() {
        let c = FailureClass::classify("Error: 429 rate limit exceeded");
        assert_eq!(c, FailureClass::QuotaLimit);
        assert!(!c.is_retryable(), "额度耗尽时重试只会再撞一次墙");
        assert!(c.should_switch_account());
    }

    #[test]
    fn network_is_retryable() {
        let c = FailureClass::classify("curl: (7) Failed to connect: Connection refused");
        assert_eq!(c, FailureClass::NetworkUnreachable);
        assert!(c.is_retryable());
    }

    #[test]
    fn missing_cli_switches_node_not_retry() {
        let c = FailureClass::classify("bash: codex: command not found");
        assert_eq!(c, FailureClass::MissingDependency);
        assert!(!c.is_retryable(), "同一台机器上再试一次仍然没有这个命令");
        assert!(c.should_switch_node());
    }

    #[test]
    fn ssh_failure_switches_node() {
        let c = FailureClass::classify("ssh: connect to host gpu9 port 22: No route to host");
        assert_eq!(c, FailureClass::NodeUnreachable);
        assert!(c.should_switch_node());
    }

    #[test]
    fn geo_block_reads_as_network_not_auth() {
        let c = FailureClass::classify("HTTP error: 403 Forbidden, url: https://api.anthropic.com");
        assert_eq!(c, FailureClass::NetworkUnreachable);
    }

    #[test]
    fn ssh_level_failures_point_at_the_node() {
        for msg in [
            "agent 未产出任何输出。stderr: hostname contains invalid characters",
            "ssh: Could not resolve hostname gpu9: nodename nor servname provided",
            "机器连不上：ssh: connect to host 10.0.0.1 port 22: Operation timed out",
        ] {
            let c = FailureClass::classify(msg);
            assert_eq!(c, FailureClass::NodeUnreachable, "应判为节点不可达: {msg}");
            assert!(c.should_switch_node());
        }
    }

    #[test]
    fn real_shell_errors_classify_as_missing_dependency() {
        for msg in [
            "agent 未产出任何输出。stderr: bash: line 1: /nonexistent/claude: No such file or directory",
            "agent 未产出任何输出。stderr: bash: claude: command not found",
            "zsh: 未找到命令: codex",
        ] {
            assert_eq!(
                FailureClass::classify(msg),
                FailureClass::MissingDependency,
                "应识别为缺依赖: {msg}"
            );
        }
    }

    #[test]
    fn empty_text_is_unknown_not_agent_error() {
        assert_eq!(FailureClass::classify(""), FailureClass::Unknown);
    }
}
