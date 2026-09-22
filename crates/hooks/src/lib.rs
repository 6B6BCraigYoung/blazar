use std::collections::BTreeMap;
use std::sync::Arc;

use blazar_transport::{ExecSpec, NodeTransport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart,

    TurnStart,

    PreToolUse,

    PostToolUse,

    TurnEnd,

    Approval,

    Error,

    SessionEnd,
}

impl HookEvent {
    #[must_use]
    pub fn can_block(self) -> bool {
        matches!(self, Self::PreToolUse | Self::TurnStart)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookTarget {
    #[default]
    Hub,

    Node,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub event: HookEvent,

    pub command: String,
    #[serde(default)]
    pub target: HookTarget,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,

    #[serde(default)]
    pub blocking: bool,

    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_timeout() -> u64 {
    30
}
const fn default_enabled() -> bool {
    true
}

impl Hook {
    #[must_use]
    pub fn matches(&self, event: HookEvent, tool: Option<&str>) -> bool {
        if !self.enabled || self.event != event {
            return false;
        }
        match (&self.matcher, tool) {
            (None, _) => true,
            (Some(m), _) if m == "*" || m.is_empty() => true,
            (Some(m), Some(t)) => {
                if let Some(prefix) = m.strip_suffix('*') {
                    t.starts_with(prefix)
                } else {
                    m == t
                }
            }

            (Some(_), None) => false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HookContext {
    pub workspace: String,
    pub node: String,
    pub cwd: String,
    pub session_id: String,
    pub tool_name: Option<String>,
    pub extra: BTreeMap<String, String>,
}

impl HookContext {
    fn env(&self, event: HookEvent) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(
            "BLAZAR_EVENT".into(),
            serde_json::to_string(&event)
                .unwrap_or_default()
                .trim_matches('"')
                .to_owned(),
        );
        m.insert("BLAZAR_WORKSPACE".into(), self.workspace.clone());
        m.insert("BLAZAR_NODE".into(), self.node.clone());
        m.insert("BLAZAR_CWD".into(), self.cwd.clone());
        m.insert("BLAZAR_SESSION".into(), self.session_id.clone());
        if let Some(t) = &self.tool_name {
            m.insert("BLAZAR_TOOL".into(), t.clone());
        }
        for (k, v) in &self.extra {
            m.insert(format!("BLAZAR_{}", k.to_uppercase()), v.clone());
        }
        m
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookOutcome {
    pub command: String,
    pub target: HookTarget,
    pub code: i32,
    pub stdout: String,
    pub stderr: String,

    pub blocked: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookReport {
    pub outcomes: Vec<HookOutcome>,
}

impl HookReport {
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.outcomes.iter().any(|o| o.blocked)
    }

    #[must_use]
    pub fn block_reason(&self) -> Option<String> {
        self.outcomes.iter().find(|o| o.blocked).map(|o| {
            let detail = if o.stderr.trim().is_empty() {
                o.stdout.trim()
            } else {
                o.stderr.trim()
            };
            if detail.is_empty() {
                format!("钩子 `{}` 以退出码 {} 否决", o.command, o.code)
            } else {
                format!("钩子 `{}` 否决: {}", o.command, detail)
            }
        })
    }
}

pub struct HookRunner {
    hooks: Vec<Hook>,
    hub: Arc<dyn NodeTransport>,
}

impl HookRunner {
    pub fn new(hooks: Vec<Hook>, hub: Arc<dyn NodeTransport>) -> Self {
        Self { hooks, hub }
    }

    pub async fn fire(
        &self,
        event: HookEvent,
        ctx: &HookContext,
        node: Option<Arc<dyn NodeTransport>>,
    ) -> HookReport {
        let mut report = HookReport::default();
        let env = ctx.env(event);

        for hook in self
            .hooks
            .iter()
            .filter(|h| h.matches(event, ctx.tool_name.as_deref()))
        {
            let transport = match hook.target {
                HookTarget::Hub => self.hub.clone(),
                HookTarget::Node => match &node {
                    Some(t) => t.clone(),

                    None => continue,
                },
            };

            let mut spec = ExecSpec::new("bash").arg("-lc").arg(&hook.command);
            if !ctx.cwd.is_empty() && hook.target == HookTarget::Node {
                spec = spec.cwd(&ctx.cwd);
            }
            for (k, v) in &env {
                spec = spec.env(k, v);
            }

            let fut = transport.exec(spec);
            let res =
                tokio::time::timeout(std::time::Duration::from_secs(hook.timeout_secs), fut).await;

            let outcome = match res {
                Ok(Ok(out)) => HookOutcome {
                    command: hook.command.clone(),
                    target: hook.target,
                    code: out.code,
                    stdout: out.stdout,
                    stderr: out.stderr,

                    blocked: hook.blocking && out.code != 0 && hook.event.can_block(),
                    timed_out: false,
                },
                Ok(Err(err)) => HookOutcome {
                    command: hook.command.clone(),
                    target: hook.target,
                    code: -1,
                    stdout: String::new(),
                    stderr: err.to_string(),
                    blocked: hook.blocking && hook.event.can_block(),
                    timed_out: false,
                },
                Err(_) => HookOutcome {
                    command: hook.command.clone(),
                    target: hook.target,
                    code: -1,
                    stdout: String::new(),
                    stderr: format!("钩子超时（>{}s）", hook.timeout_secs),
                    blocked: hook.blocking && hook.event.can_block(),
                    timed_out: true,
                },
            };

            let stop = outcome.blocked;
            report.outcomes.push(outcome);

            if stop {
                break;
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blazar_transport::LocalTransport;

    fn hook(event: HookEvent, cmd: &str) -> Hook {
        Hook {
            event,
            command: cmd.into(),
            target: HookTarget::Hub,
            matcher: None,
            blocking: false,
            timeout_secs: 10,
            enabled: true,
        }
    }

    fn ctx() -> HookContext {
        HookContext {
            workspace: "ws1".into(),
            node: "gpu1".into(),
            cwd: String::new(),
            session_id: "s1".into(),
            tool_name: Some("Bash".into()),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn matcher_supports_prefix_wildcard() {
        let mut h = hook(HookEvent::PreToolUse, "true");
        h.matcher = Some("mcp__*".into());
        assert!(h.matches(HookEvent::PreToolUse, Some("mcp__github__list")));
        assert!(!h.matches(HookEvent::PreToolUse, Some("Bash")));
    }

    #[test]
    fn matcher_without_tool_does_not_fire() {
        let mut h = hook(HookEvent::SessionStart, "true");
        h.matcher = Some("Bash".into());
        assert!(!h.matches(HookEvent::SessionStart, None));
    }

    #[test]
    fn disabled_hook_never_matches() {
        let mut h = hook(HookEvent::TurnEnd, "true");
        h.enabled = false;
        assert!(!h.matches(HookEvent::TurnEnd, None));
    }

    #[test]
    fn only_pre_events_can_block() {
        assert!(HookEvent::PreToolUse.can_block());
        assert!(!HookEvent::PostToolUse.can_block());
        assert!(!HookEvent::SessionEnd.can_block());
    }

    #[tokio::test]
    async fn blocking_hook_vetoes_and_stops_the_rest() {
        let mut deny = hook(HookEvent::PreToolUse, "echo 不允许 >&2; exit 1");
        deny.blocking = true;
        let after = hook(HookEvent::PreToolUse, "echo 不该跑到这里");

        let r = HookRunner::new(vec![deny, after], Arc::new(LocalTransport));
        let report = r.fire(HookEvent::PreToolUse, &ctx(), None).await;

        assert!(report.is_blocked());
        assert!(report.block_reason().unwrap().contains("不允许"));
        assert_eq!(report.outcomes.len(), 1, "被否决后不应继续跑后续钩子");
    }

    #[tokio::test]
    async fn post_event_failure_does_not_block() {
        let mut h = hook(HookEvent::PostToolUse, "exit 3");
        h.blocking = true;
        let r = HookRunner::new(vec![h], Arc::new(LocalTransport));
        let report = r.fire(HookEvent::PostToolUse, &ctx(), None).await;
        assert!(!report.is_blocked());
        assert_eq!(report.outcomes[0].code, 3);
    }

    #[tokio::test]
    async fn context_reaches_the_script_as_env() {
        let r = HookRunner::new(
            vec![hook(
                HookEvent::TurnEnd,
                "echo \"$BLAZAR_NODE/$BLAZAR_TOOL\"",
            )],
            Arc::new(LocalTransport),
        );
        let report = r.fire(HookEvent::TurnEnd, &ctx(), None).await;
        assert_eq!(report.outcomes[0].stdout.trim(), "gpu1/Bash");
    }

    #[tokio::test]
    async fn node_hook_is_skipped_without_node_transport() {
        let mut h = hook(HookEvent::TurnEnd, "echo 跑错地方了");
        h.target = HookTarget::Node;
        let r = HookRunner::new(vec![h], Arc::new(LocalTransport));
        let report = r.fire(HookEvent::TurnEnd, &ctx(), None).await;
        assert!(report.outcomes.is_empty());
    }

    #[tokio::test]
    async fn timeout_is_reported_not_hung() {
        let mut h = hook(HookEvent::TurnEnd, "sleep 30");
        h.timeout_secs = 1;
        let r = HookRunner::new(vec![h], Arc::new(LocalTransport));
        let report = r.fire(HookEvent::TurnEnd, &ctx(), None).await;
        assert!(report.outcomes[0].timed_out);
    }
}
