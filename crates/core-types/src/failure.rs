use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCause {
    Network,
    Busy,
    Login,
    Quota,
    Node,
    Resume,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeLoss {
    Message,
    Conversation,
}

const NODE: &[&str] = &[
    "ssh:",
    "机器连不上",
    "no route to host",
    "host is down",
    "connection closed by remote host",
    "could not resolve hostname",
    "退出码 255",
    "agent 进程已经不在了",
    "启动 agent 失败",
    "permission denied (publickey",
    "host key verification",
    "hostname contains invalid characters",
    "kex_exchange_identification",
    "执行端断开",
    "机器离线",
];

const LOGIN: &[&str] = &[
    "not logged in",
    "authentication",
    "invalid_api_key",
    "invalid api key",
    "unauthorized",
    "oauth_org_not_allowed",
    "account_on_hold",
    "cloud_credential_error",
    "token expired",
    "please run /login",
    "permission_error",
    "forbidden",
    "未登录",
    "登录已失效",
];

const QUOTA: &[&str] = &[
    "rate limit",
    "rate_limit",
    "ratelimit",
    "quota",
    "usage limit",
    "usage_limit",
    "hit your limit",
    "limit reached",
    "too many requests",
    "billing",
    "credits",
    "extra usage",
    "额度",
    "限流",
];

const BUSY: &[&str] = &[
    "overloaded",
    "server_error",
    "internal server error",
    "service unavailable",
    "bad gateway",
    "服务繁忙",
];

const NETWORK: &[&str] = &[
    "enotfound",
    "econnreset",
    "econnrefused",
    "econnaborted",
    "etimedout",
    "eai_again",
    "enetunreach",
    "enetdown",
    "ehostunreach",
    "ehostdown",
    "epipe",
    "err_socket_closed",
    "und_err_socket",
    "err_proxy_tunnel",
    "failedtoopensocket",
    "can't reach the api server",
    "connection lost",
    "computer was asleep",
    "connection dropped",
    "connection refused",
    "connection closed before the response",
    "no internet route",
    "no response from api",
    "timed out",
    "timeout",
    "network error",
    "network_error",
    "connection_error",
    "connection error",
    "fetch failed",
    "socket hang up",
    "getaddrinfo",
    "tls handshake",
    "certificate",
    "network is unreachable",
    "could not resolve",
    "streamsuspended",
    "streamtruncated",
    "断网",
    "网络",
];

fn has_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

fn has_code(text: &str, code: &str) -> bool {
    text.match_indices(code).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + code.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

#[must_use]
pub fn resume_loss(message: &str) -> Option<ResumeLoss> {
    let t = message.to_ascii_lowercase();
    if t.contains("no message found with message.uuid") {
        Some(ResumeLoss::Message)
    } else if t.contains("no conversation found")
        || t.contains("could not continue the previous session")
    {
        Some(ResumeLoss::Conversation)
    } else {
        None
    }
}

impl FailureCause {
    pub const ALL: [Self; 7] = [
        Self::Network,
        Self::Busy,
        Self::Login,
        Self::Quota,
        Self::Node,
        Self::Resume,
        Self::Other,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Busy => "busy",
            Self::Login => "login",
            Self::Quota => "quota",
            Self::Node => "node",
            Self::Resume => "resume",
            Self::Other => "other",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == raw)
    }

    #[must_use]
    pub fn of(message: &str, node_online: bool) -> Self {
        if resume_loss(message).is_some() {
            return Self::Resume;
        }
        if !node_online {
            return Self::Node;
        }
        let t = message.to_ascii_lowercase();
        if has_any(&t, NODE) {
            Self::Node
        } else if has_any(&t, LOGIN) || has_code(&t, "401") || has_code(&t, "403") {
            Self::Login
        } else if has_any(&t, QUOTA) || has_code(&t, "429") {
            Self::Quota
        } else if has_any(&t, BUSY) || ["529", "503", "502", "500"].iter().any(|c| has_code(&t, c))
        {
            Self::Busy
        } else if has_any(&t, NETWORK) {
            Self::Network
        } else {
            Self::Other
        }
    }

    #[must_use]
    pub const fn continues(self) -> bool {
        matches!(self, Self::Network | Self::Busy)
    }

    #[must_use]
    pub const fn is_account(self) -> bool {
        matches!(self, Self::Login | Self::Quota)
    }

    #[must_use]
    pub const fn headline(self) -> &'static str {
        match self {
            Self::Network => "网络断了，没做完",
            Self::Busy => "服务繁忙，没做完",
            Self::Node => "机器连不上，没做完",
            Self::Login => "账号没登录，没做完",
            Self::Quota => "额度用完了，没做完",
            Self::Resume => "上一段对话接不上了",
            Self::Other => "出错了，没做完",
        }
    }

    #[must_use]
    pub const fn advice(self) -> &'static str {
        match self {
            Self::Network | Self::Busy => "可以继续",
            Self::Node => "先把机器连上",
            Self::Login => "先登录",
            Self::Quota => "可以换个账号",
            Self::Resume => "可以重新开始",
            Self::Other => "可以重试",
        }
    }

    #[must_use]
    pub fn one_liner(self) -> String {
        format!("{}，{}", self.headline(), self.advice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(message: &str) -> FailureCause {
        FailureCause::of(message, true)
    }

    #[test]
    fn dropped_connections_read_as_network() {
        for m in [
            "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)",
            "API Error: Connection lost while your computer was asleep",
            "API Error: Connection dropped (ECONNRESET)",
            "Request timed out. Check your internet connection and proxy settings",
            "API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)",
            "API Error: No internet route — check your connection or VPN (ENETUNREACH)",
            "agent 异常退出（退出码 1）。stderr: TypeError: fetch failed",
        ] {
            assert_eq!(of(m), FailureCause::Network, "{m}");
        }
    }

    #[test]
    fn overloaded_servers_read_as_busy() {
        for m in [
            "API Error: 529 overloaded",
            r#"API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            "API Error: 500 Internal server error",
            "API Error: 503 Service Unavailable",
            "server_error",
        ] {
            assert_eq!(of(m), FailureCause::Busy, "{m}");
        }
    }

    #[test]
    fn missing_credentials_read_as_login() {
        for m in [
            "Not logged in · Please run /login（api_error）",
            "authentication_failed：Not logged in · Please run /login",
            r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"Invalid authentication credentials"}}"#,
            "API Error: 403 Forbidden",
            "Invalid API key · Please run /login",
            "invalid_api_key",
        ] {
            assert_eq!(of(m), FailureCause::Login, "{m}");
        }
    }

    #[test]
    fn exhausted_plans_read_as_quota() {
        for m in [
            "You've hit your usage limit · resets 8am (Asia/Shanghai)",
            r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your account's rate limit"}}"#,
            "Fable 5.1 requires usage credits. Switch to another model, or manage usage credits",
            "rate_limit：This request would exceed your account's rate limit. Please try again later.",
            "billing_error",
        ] {
            assert_eq!(of(m), FailureCause::Quota, "{m}");
        }
    }

    #[test]
    fn unreachable_machines_read_as_node_before_anything_else() {
        for m in [
            "agent 异常退出（退出码 255）。stderr: ssh: connect to host gpu-x port 22: Operation timed out",
            "在 gpu-x 上启动 agent 失败: ssh: Could not resolve hostname gpu-x: nodename nor servname provided",
            "agent 进程已经不在了（机器重启、被整组杀掉或目录被删），可以续接上一段对话重来",
            "机器连不上：ssh: connect to host 10.0.0.1 port 22: No route to host",
            "agent 未产出任何输出。stderr: Permission denied (publickey).",
        ] {
            assert_eq!(of(m), FailureCause::Node, "{m}");
        }
        assert_eq!(
            FailureCause::of("API Error: 529 overloaded", false),
            FailureCause::Node,
            "机器离线时，不管报错写什么，都先让用户把机器连上"
        );
    }

    #[test]
    fn lost_resume_points_are_blazar_s_own_problem() {
        let missing = "agent 异常退出（退出码 1）。stderr: No message found with message.uuid of: 9b915e37-5c1e-4a7e-9d3b-2f1c0a8e7b6d";
        assert_eq!(of(missing), FailureCause::Resume);
        assert_eq!(resume_loss(missing), Some(ResumeLoss::Message));
        let gone = "No conversation found with session ID: 019a2c1e-7f3b-7b2a-9c4d-1e2f3a4b5c6d";
        assert_eq!(of(gone), FailureCause::Resume);
        assert_eq!(resume_loss(gone), Some(ResumeLoss::Conversation));
        assert_eq!(
            FailureCause::of(missing, false),
            FailureCause::Resume,
            "续接失败由 Blazar 自己处理，机器状态不改变这一点"
        );
        assert_eq!(resume_loss("API Error: 529 overloaded"), None);
    }

    #[test]
    fn everything_else_stays_other() {
        for m in [
            "agent 异常退出（退出码 1）。stderr: bash: claude: command not found",
            "error_max_turns",
            "API Error: 400 due to tool use concurrency issues.",
            "agent 以错误状态结束",
            "",
        ] {
            assert_eq!(of(m), FailureCause::Other, "{m}");
        }
    }

    #[test]
    fn status_codes_must_stand_alone() {
        assert!(has_code("API Error: 529 overloaded", "529"));
        assert!(has_code("HTTP 502", "502"));
        assert!(has_code("请求没有成功（401）", "401"));
        assert!(!has_code("toolu_01529abc", "529"));
        assert!(!has_code("9b915e37-5c1e-401e-9d3b", "401"));
    }

    #[test]
    fn causes_round_trip_through_their_names() {
        for c in FailureCause::ALL {
            assert_eq!(FailureCause::parse(c.as_str()), Some(c));
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(json, format!("\"{}\"", c.as_str()));
        }
        assert_eq!(FailureCause::parse("nope"), None);
        assert!(FailureCause::Network.continues());
        assert!(FailureCause::Busy.continues());
        assert!(FailureCause::Quota.is_account());
        assert!(!FailureCause::Node.is_account());
    }
}
