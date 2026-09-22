use std::sync::Arc;

use async_trait::async_trait;
use blazar_core_types::{EntryKind, NormalizedEntry, Outcome, ProviderSessionId, RuntimeKind};
use blazar_transport::detached::RunMode;
use blazar_transport::{ExecSpec, NodeTransport};
use chrono::Utc;
use futures::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::spec::{AgentSpec, OutputFormat};
use crate::{AgentRuntime, RuntimeError, SessionHandle, SessionSpec};

pub struct CliRuntime {
    transport: Arc<dyn NodeTransport>,
    spec: &'static AgentSpec,
    program: String,
}

fn briefing_of(session: &SessionSpec) -> String {
    [
        session
            .instructions
            .clone()
            .filter(|s| !s.trim().is_empty()),
        session
            .remote_hands
            .as_ref()
            .map(crate::RemoteHands::briefing),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n")
}

pub const CODEX_SANDBOX: &[&str] = &["read-only", "workspace-write", "danger-full-access"];

pub const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

impl CliRuntime {
    pub fn new(transport: Arc<dyn NodeTransport>, spec: &'static AgentSpec) -> Self {
        Self {
            transport,
            spec,
            program: spec.program.to_owned(),
        }
    }

    #[must_use]
    pub fn by_id(transport: Arc<dyn NodeTransport>, id: &str) -> Option<Self> {
        crate::spec::find(id).map(|s| Self::new(transport, s))
    }

    #[must_use]
    pub fn with_program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    #[must_use]
    pub fn spec(&self) -> &'static AgentSpec {
        self.spec
    }

    fn program_exec(&self) -> ExecSpec {
        if self.program.ends_with(".js") {
            ExecSpec::new("node").arg(&self.program)
        } else {
            ExecSpec::new(&self.program)
        }
    }

    fn exec_spec(&self, session: &SessionSpec, resume: Option<&ProviderSessionId>) -> ExecSpec {
        let cwd = session.cwd.display().to_string();
        let extra = self.extra_args(session);

        let briefing = briefing_of(session);
        let prompt = if !briefing.is_empty() && !matches!(self.spec.id, "claude" | "codex" | "grok")
        {
            format!("{briefing}\n\n{}", session.prompt)
        } else {
            session.prompt.clone()
        };
        let args = self.spec.build_args(
            &prompt,
            resume.map(|r| r.0.as_str()),
            session.model.as_deref(),
            Some(&cwd),
            &extra,
        );

        let mut e = self.program_exec().args(args).cwd(&session.cwd);
        for (k, v) in &session.env {
            e = e.env(k, v);
        }
        e
    }

    fn extra_args(&self, session: &SessionSpec) -> Vec<String> {
        let mut extra: Vec<String> = Vec::new();
        if let Some(mode) = &session.permission_mode {
            match self.spec.id {
                "claude" | "grok" => {
                    extra.push("--permission-mode".into());
                    extra.push(mode.clone());
                }

                "codex"
                    if session.remote_hands.is_none() && CODEX_SANDBOX.contains(&mode.as_str()) =>
                {
                    extra.push("-s".into());
                    extra.push(mode.clone());
                }
                _ => {}
            }
        }
        let mut allowed = session.allowed_tools.clone();
        let briefing = briefing_of(session);
        if let Some(h) = &session.remote_hands {
            match self.spec.id {
                "claude" => {
                    extra.push("--tools".into());

                    extra.push(if session.add_dirs.is_empty() {
                        "Task,TodoWrite,WebFetch,WebSearch".into()
                    } else {
                        "Task,TodoWrite,WebFetch,WebSearch,Skill".into()
                    });

                    let mut servers = serde_json::Map::new();
                    servers.insert(
                        crate::REMOTE_HANDS_SERVER.into(),
                        serde_json::json!({ "command": h.command, "args": h.args }),
                    );
                    for m in &session.mcp_servers {
                        servers
                            .entry(m.name.clone())
                            .or_insert_with(|| m.claude_json());
                    }
                    extra.push("--mcp-config".into());
                    extra.push(serde_json::json!({ "mcpServers": servers }).to_string());

                    extra.push("--strict-mcp-config".into());
                    if !briefing.is_empty() {
                        extra.push("--append-system-prompt".into());
                        extra.push(briefing.clone());
                    }

                    let t = |n: &str| format!("mcp__{}__{n}", crate::REMOTE_HANDS_SERVER);
                    allowed.extend(["Read", "Glob", "Grep"].map(t));
                    match session.permission_mode.as_deref() {
                        Some("acceptEdits") => allowed.extend(["Edit", "Write"].map(t)),
                        Some("bypassPermissions") => {
                            allowed.extend(["Edit", "Write", "Bash"].map(t));
                        }
                        _ => {}
                    }
                }
                "codex" => {
                    for f in ["shell_tool", "unified_exec"] {
                        extra.push("--disable".into());
                        extra.push(f.into());
                    }
                    extra.push("-s".into());
                    extra.push("read-only".into());
                    let toml_str = |s: &str| serde_json::Value::String(s.to_owned()).to_string();
                    let srv = crate::REMOTE_HANDS_SERVER;
                    extra.push("-c".into());
                    extra.push(format!(
                        "mcp_servers.{srv}.command={}",
                        toml_str(&h.command)
                    ));
                    extra.push("-c".into());
                    extra.push(format!(
                        "mcp_servers.{srv}.args=[{}]",
                        h.args
                            .iter()
                            .map(|a| toml_str(a))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));

                    extra.push("-c".into());
                    extra.push(format!(
                        "mcp_servers.{srv}.default_tools_approval_mode=\"approve\""
                    ));
                    extra.push("-c".into());
                    extra.push(format!("developer_instructions={}", toml_str(&briefing)));
                }
                _ => {}
            }
        } else if !briefing.is_empty() {
            match self.spec.id {
                "claude" => {
                    extra.push("--append-system-prompt".into());
                    extra.push(briefing.clone());
                }
                "codex" => {
                    extra.push("-c".into());
                    extra.push(format!(
                        "developer_instructions={}",
                        serde_json::Value::String(briefing.clone())
                    ));
                }
                "grok" => {
                    extra.push("--rules".into());
                    extra.push(briefing.clone());
                }
                _ => {}
            }
        }
        if let Some(e) = session.effort.as_deref().filter(|e| EFFORTS.contains(e)) {
            match self.spec.id {
                "claude" | "grok" => {
                    extra.push("--effort".into());
                    extra.push(e.to_owned());
                }
                "codex" => {
                    extra.push("-c".into());
                    extra.push(format!("model_reasoning_effort=\"{e}\""));
                }
                _ => {}
            }
        }

        if session.remote_hands.is_none() && !session.mcp_servers.is_empty() {
            match self.spec.id {
                "claude" => {
                    let servers: serde_json::Map<String, serde_json::Value> = session
                        .mcp_servers
                        .iter()
                        .map(|m| (m.name.clone(), m.claude_json()))
                        .collect();

                    extra.push("--mcp-config".into());
                    extra.push(serde_json::json!({ "mcpServers": servers }).to_string());
                }
                "codex" => {
                    let toml_str = |s: &str| serde_json::Value::String(s.to_owned()).to_string();
                    for m in &session.mcp_servers {
                        let key = format!("mcp_servers.{}", m.name);
                        if m.http {
                            extra.push("-c".into());
                            extra.push(format!("{key}.url={}", toml_str(&m.url)));
                            continue;
                        }
                        extra.push("-c".into());
                        extra.push(format!("{key}.command={}", toml_str(&m.command)));
                        extra.push("-c".into());
                        extra.push(format!(
                            "{key}.args=[{}]",
                            m.args
                                .iter()
                                .map(|a| toml_str(a))
                                .collect::<Vec<_>>()
                                .join(",")
                        ));
                    }
                }
                _ => {}
            }
        }
        if self.spec.id == "claude" {
            for d in &session.add_dirs {
                extra.push("--add-dir".into());
                extra.push(d.clone());
            }
        }
        if self.spec.id == "claude" {
            extra.push("--thinking-display".into());
            extra.push("summarized".into());
        }

        if self.spec.id == "claude"
            && let Some(at) = session.resume_at.as_deref().filter(|u| valid_uuid(u))
        {
            extra.push("--resume-session-at".into());
            extra.push(at.to_owned());
        }
        if self.spec.id == "claude"
            && let Some(settings) = flag_settings(session)
        {
            extra.push("--settings".into());
            extra.push(settings.to_string());
        }
        if self.spec.id == "claude" {
            if !allowed.is_empty() {
                extra.push("--allowedTools".into());
                extra.extend(allowed);
            }
            if !session.disallowed_tools.is_empty() {
                extra.push("--disallowedTools".into());
                extra.extend(session.disallowed_tools.iter().cloned());
            }
        }

        extra.extend(session.extra_args.iter().cloned());
        extra
    }

    #[must_use]
    pub fn interactive(&self) -> bool {
        self.spec.interactive
    }

    #[must_use]
    pub fn detached_plan(
        &self,
        session: &SessionSpec,
        resume: Option<&ProviderSessionId>,
        new_session_id: Option<&str>,
    ) -> DetachedPlan {
        if !self.spec.interactive {
            return DetachedPlan {
                spec: self.exec_spec(session, resume),
                mode: RunMode::Null,
                first_input: None,
            };
        }
        let cwd = session.cwd.display().to_string();
        let mut extra = vec![
            "--input-format".to_owned(),
            "stream-json".to_owned(),
            "--permission-prompt-tool".to_owned(),
            "stdio".to_owned(),
            "--replay-user-messages".to_owned(),
        ];

        if let (None, Some(id)) = (resume, new_session_id) {
            extra.push("--session-id".to_owned());
            extra.push(id.to_owned());
        }
        extra.extend(self.extra_args(session));
        let args = self.spec.base_args(
            resume.map(|r| r.0.as_str()),
            session.model.as_deref(),
            Some(&cwd),
            &extra,
        );
        let mut spec = self.program_exec().args(args).cwd(&session.cwd);
        for (k, v) in &session.env {
            spec = spec.env(k, v);
        }
        DetachedPlan {
            spec,
            mode: RunMode::Relay,
            first_input: Some(user_input_line_with_images(
                "u-1",
                &session.prompt,
                &session.images,
            )),
        }
    }

    #[must_use]
    pub fn parse_line(&self, line: &str) -> Vec<(EntryKind, Option<blazar_core_types::ToolId>)> {
        parse_by_format(self.spec.output, line)
    }
}

#[derive(Debug, Clone)]
pub struct DetachedPlan {
    pub spec: ExecSpec,
    pub mode: RunMode,

    pub first_input: Option<String>,
}

#[must_use]
pub fn user_input_line(msg_id: &str, text: &str) -> String {
    user_input_line_with_images(msg_id, text, &[])
}

#[must_use]
pub fn user_input_line_with_images(
    msg_id: &str,
    text: &str,
    images: &[crate::ImageInput],
) -> String {
    let mut content: Vec<serde_json::Value> = images
        .iter()
        .map(|i| {
            serde_json::json!({
                "type": "image",
                "source": { "type": "base64", "media_type": i.media_type, "data": i.data },
            })
        })
        .collect();
    content.push(serde_json::json!({ "type": "text", "text": text }));
    let v = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": content },
    });
    format!("{msg_id}\t{v}")
}

#[must_use]
pub fn flag_settings(session: &SessionSpec) -> Option<serde_json::Value> {
    let mut m = serde_json::Map::new();
    if let Some(s) = session
        .output_style
        .as_deref()
        .map(str::trim)
        .filter(|s| valid_style(s))
    {
        m.insert("outputStyle".into(), s.into());
    }
    if let Some(f) = session.fast_mode {
        m.insert("fastMode".into(), f.into());
    }
    if session.thinking == Some(false) {
        m.insert("alwaysThinkingEnabled".into(), false.into());
    }
    (!m.is_empty()).then_some(serde_json::Value::Object(m))
}

#[must_use]
pub fn valid_style(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 60 && !s.chars().any(char::is_control)
}

#[must_use]
pub fn control_json(request_id: &str, request: serde_json::Value) -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": request,
    })
    .to_string()
}

#[must_use]
pub fn approval_response_json(
    request_id: &str,
    allow: bool,
    updated_input: Option<&serde_json::Value>,
    message: &str,
) -> String {
    let resp = if allow {
        serde_json::json!({
            "behavior": "allow",
            "updatedInput": updated_input.cloned().unwrap_or_else(|| serde_json::json!({})),
        })
    } else {
        serde_json::json!({ "behavior": "deny", "message": message })
    };
    serde_json::json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": resp },
    })
    .to_string()
}

#[must_use]
pub fn interrupt_json(request_id: &str) -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "interrupt" },
    })
    .to_string()
}

impl CliRuntime {
    async fn run(
        &self,
        session: &SessionSpec,
        resume: Option<&ProviderSessionId>,
    ) -> Result<SessionHandle, RuntimeError> {
        if resume.is_some() && !self.spec.resume.is_supported() {
            return Err(RuntimeError::Unsupported("该 agent 不支持续接会话"));
        }

        let mut lines = self
            .transport
            .spawn_lines(self.exec_spec(session, resume))
            .await
            .map_err(|e| RuntimeError::Transport(e.to_string()))?;

        let killer = lines.killer.clone();

        let (tx, rx) = tokio::sync::mpsc::channel::<NormalizedEntry>(1024);
        let format = self.spec.output;

        tokio::spawn(async move {
            let mut seq: u64 = 1;
            let mut saw_any = false;

            while let Some(line) = lines.stdout.next().await {
                if line.trim().is_empty() {
                    continue;
                }
                let parsed = parse_by_format(format, &line);
                for (kind, parent) in parsed {
                    saw_any = true;
                    let entry = NormalizedEntry {
                        seq,
                        ts: Utc::now(),
                        parent_tool_use_id: parent,
                        kind,
                    };
                    seq += 1;
                    if tx.send(entry).await.is_err() {
                        return;
                    }
                }
            }

            if !saw_any {
                let err = lines.stderr_summary();
                let message = if err.trim().is_empty() {
                    "agent 未产出任何输出（检查 CLI 是否安装、凭据与网络出口）".to_owned()
                } else {
                    format!("agent 未产出任何输出。stderr: {}", err.trim())
                };
                let _ = tx
                    .send(NormalizedEntry {
                        seq,
                        ts: Utc::now(),
                        parent_tool_use_id: None,
                        kind: EntryKind::Finished(Outcome::Failed { message }),
                    })
                    .await;
            }
            drop(lines);
        });

        Ok(SessionHandle {
            events: ReceiverStream::new(rx).boxed(),
            killer: Some(killer),
        })
    }
}

type Parsed = Vec<(EntryKind, Option<blazar_core_types::ToolId>)>;

fn valid_uuid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn parse_by_format(format: OutputFormat, line: &str) -> Parsed {
    match format {
        OutputFormat::ClaudeStreamJson => crate::claude::parse::parse_line(line)
            .map(|p| {
                p.entries
                    .into_iter()
                    .map(|k| (k, p.parent_tool_use_id.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        OutputFormat::CodexJsonl => crate::codex::parse_line(line)
            .into_iter()
            .map(|k| (k, None))
            .collect(),
        OutputFormat::PlainLines => {
            vec![(
                EntryKind::AssistantMessage {
                    text: line.to_owned(),
                },
                None,
            )]
        }
    }
}

#[async_trait]
impl AgentRuntime for CliRuntime {
    fn kind(&self) -> RuntimeKind {
        match self.spec.id {
            "codex" => RuntimeKind::CodexAppServer,
            _ => RuntimeKind::ClaudeCli,
        }
    }

    fn target(&self) -> &str {
        self.transport.target()
    }

    async fn start(&self, spec: SessionSpec) -> Result<SessionHandle, RuntimeError> {
        self.run(&spec, None).await
    }

    async fn resume(
        &self,
        id: &ProviderSessionId,
        spec: SessionSpec,
    ) -> Result<SessionHandle, RuntimeError> {
        self.run(&spec, Some(id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blazar_transport::LocalTransport;

    fn rt(id: &str) -> CliRuntime {
        CliRuntime::by_id(Arc::new(LocalTransport), id).expect("内置表里应有该 agent")
    }

    #[test]
    fn unknown_agent_id_is_rejected() {
        assert!(CliRuntime::by_id(Arc::new(LocalTransport), "不存在的agent").is_none());
    }

    #[test]
    fn claude_gets_permission_and_tool_flags() {
        let r = rt("claude");
        let mut s = SessionSpec::new("/w", "hi");
        s.permission_mode = Some("acceptEdits".into());
        s.disallowed_tools = vec!["Bash".into()];
        let e = r.exec_spec(&s, None);
        assert!(
            e.args
                .windows(2)
                .any(|w| w == ["--permission-mode", "acceptEdits"])
        );
        assert!(
            e.args
                .windows(2)
                .any(|w| w == ["--disallowedTools", "Bash"])
        );
    }

    fn hands() -> crate::RemoteHands {
        crate::RemoteHands {
            node: "gpu-1".into(),
            root: "/home/me/proj".into(),
            command: "/Applications/Blazar.app/Contents/MacOS/blazar-desktop".into(),
            args: vec![
                "__mcp-remote".into(),
                "--node".into(),
                "gpu-1".into(),
                "--root".into(),
                "/home/me/proj".into(),
            ],
        }
    }

    #[test]
    fn remote_hands_claude_loses_local_file_tools() {
        let r = rt("claude");
        let mut s = SessionSpec::new("/local/placeholder", "改一下 main.rs");
        s.permission_mode = Some("acceptEdits".into());
        s.remote_hands = Some(hands());
        let e = r.exec_spec(&s, None);
        let pos = |a: &str| e.args.iter().position(|x| x == a);

        let tools = &e.args[pos("--tools").unwrap() + 1];
        assert_eq!(tools, "Task,TodoWrite,WebFetch,WebSearch");
        assert!(!tools.contains("Bash") && !tools.contains("Edit"));
        assert!(
            pos("--strict-mcp-config").is_some(),
            "不能带上用户本机的其它 MCP"
        );
        let cfg: serde_json::Value =
            serde_json::from_str(&e.args[pos("--mcp-config").unwrap() + 1]).unwrap();
        assert_eq!(cfg["mcpServers"]["blazar"]["args"][2], "gpu-1");

        assert!(e.args.iter().any(|a| a == "mcp__blazar__Edit"));
        assert!(!e.args.iter().any(|a| a == "mcp__blazar__Bash"));

        assert_eq!(e.args.last().unwrap(), "改一下 main.rs");
        assert_eq!(e.args[e.args.len() - 2], "--");
    }

    #[test]
    fn instructions_merge_with_remote_briefing_into_one_prompt() {
        let r = rt("claude");
        let mut s = SessionSpec::new("/p", "hi");
        s.instructions = Some("你是代码审查员，只提意见不改代码。".into());
        s.remote_hands = Some(hands());
        let e = r.exec_spec(&s, None);
        let n = e
            .args
            .iter()
            .filter(|a| *a == "--append-system-prompt")
            .count();
        assert_eq!(
            n, 1,
            "只能有一个 --append-system-prompt，后一个会覆盖前一个"
        );
        let i = e
            .args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .unwrap();
        assert!(e.args[i + 1].contains("代码审查员") && e.args[i + 1].contains("gpu-1"));

        let mut s2 = SessionSpec::new("/p", "hi");
        s2.instructions = Some("写文档".into());
        let e2 = r.exec_spec(&s2, None);
        assert!(e2.args.iter().any(|a| a == "写文档"));
    }

    #[test]
    fn images_go_before_text_in_the_first_message() {
        let img = crate::ImageInput {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        };
        let line = user_input_line_with_images("u-1", "看图", &[img]);
        let json: serde_json::Value =
            serde_json::from_str(line.split_once('\t').unwrap().1).unwrap();
        let c = &json["message"]["content"];
        assert_eq!(c[0]["type"], "image");
        assert_eq!(c[0]["source"]["media_type"], "image/png");
        assert_eq!(c[1]["text"], "看图");
        let ctl: serde_json::Value = serde_json::from_str(&control_json(
            "c-1",
            serde_json::json!({ "subtype": "set_model", "model": "sonnet" }),
        ))
        .unwrap();
        assert_eq!(ctl["request"]["subtype"], "set_model");
    }

    #[test]
    fn effort_maps_per_runtime_and_rejects_garbage() {
        let mut s = SessionSpec::new("/p", "hi");
        s.effort = Some("xhigh".into());
        let c = rt("claude").exec_spec(&s, None).args;
        assert!(c.windows(2).any(|w| w == ["--effort", "xhigh"]));
        let x = rt("codex").exec_spec(&s, None).args.join(" ");
        assert!(x.contains(r#"model_reasoning_effort="xhigh""#));
        s.effort = Some("high; rm -rf /".into());
        assert!(
            !rt("claude")
                .exec_spec(&s, None)
                .args
                .iter()
                .any(|a| a == "--effort")
        );
    }

    #[test]
    fn library_mcp_servers_and_skills_reach_the_cli() {
        let mut s = SessionSpec::new("/p", "hi");
        s.mcp_servers = vec![
            crate::McpServerSpec {
                name: "docs".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "docs-mcp".into()],
                env: [(
                    "API_KEY".to_owned(),
                    "${BLAZAR_MCP_DOCS_API_KEY}".to_owned(),
                )]
                .into(),
                ..Default::default()
            },
            crate::McpServerSpec {
                name: "remote".into(),
                http: true,
                url: "https://mcp.example/mcp".into(),
                ..Default::default()
            },
        ];
        s.add_dirs = vec!["/tmp/blazar-skills/a".into()];
        let c = rt("claude").exec_spec(&s, None).args;
        let at = c.iter().position(|a| a == "--mcp-config").unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&c[at + 1]).unwrap();
        assert_eq!(cfg["mcpServers"]["docs"]["command"], "npx");

        assert_eq!(
            cfg["mcpServers"]["docs"]["env"]["API_KEY"],
            "${BLAZAR_MCP_DOCS_API_KEY}"
        );
        assert_eq!(cfg["mcpServers"]["remote"]["type"], "http");
        assert!(!c.iter().any(|a| a == "--strict-mcp-config"));
        assert!(
            c.windows(2)
                .any(|w| w == ["--add-dir", "/tmp/blazar-skills/a"])
        );

        let x = rt("codex").exec_spec(&s, None).args.join(" ");
        assert!(
            x.contains(r#"mcp_servers.docs.command="npx""#)
                && x.contains(r#"mcp_servers.remote.url="https://mcp.example/mcp""#)
        );
        assert!(!x.contains("--add-dir"));

        s.remote_hands = Some(hands());
        let r = rt("claude").exec_spec(&s, None).args;
        assert_eq!(r.iter().filter(|a| *a == "--mcp-config").count(), 1);
        let at = r.iter().position(|a| a == "--mcp-config").unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&r[at + 1]).unwrap();
        assert!(cfg["mcpServers"]["blazar"].is_object() && cfg["mcpServers"]["docs"].is_object());
        assert!(r.iter().any(|a| a == "--strict-mcp-config"));
        assert!(r.iter().any(|a| a.ends_with(",Skill")));
    }

    #[test]
    fn resume_at_is_claude_only_and_shape_checked() {
        let mut s = SessionSpec::new("/p", "hi");
        s.resume_at = Some("0b9d2c5e-1a2b-4c3d-9e8f-001122334455".into());
        let sid = ProviderSessionId("sess".into());
        let c = rt("claude").exec_spec(&s, Some(&sid)).args;
        assert!(c.windows(2).any(|w| w
            == [
                "--resume-session-at",
                "0b9d2c5e-1a2b-4c3d-9e8f-001122334455"
            ]));

        assert!(
            !rt("codex")
                .exec_spec(&s, Some(&sid))
                .args
                .iter()
                .any(|a| a == "--resume-session-at")
        );
        s.resume_at = Some("--dangerously-skip-permissions".into());
        assert!(
            !rt("claude")
                .exec_spec(&s, Some(&sid))
                .args
                .iter()
                .any(|a| a == "--resume-session-at")
        );
    }

    #[test]
    fn claude_asks_for_thinking_text() {
        let s = SessionSpec::new("/p", "hi");
        let c = rt("claude").exec_spec(&s, None).args;
        assert!(
            c.windows(2)
                .any(|w| w == ["--thinking-display", "summarized"])
        );
        let x = rt("codex").exec_spec(&s, None).args;
        assert!(!x.iter().any(|a| a == "--thinking-display"));
    }

    #[test]
    fn instructions_reach_every_runtime() {
        let mut s = SessionSpec::new("/p", "hi");
        s.instructions = Some("只提意见".into());
        let g = rt("grok").exec_spec(&s, None).args;
        assert!(g.windows(2).any(|w| w == ["--rules", "只提意见"]));
        let d = rt("deepcode").exec_spec(&s, None).args;
        assert_eq!(
            d.last().unwrap(),
            "-p=只提意见\n\nhi",
            "没有系统提示参数的，放进提示词"
        );
        let c = rt("claude").exec_spec(&s, None).args;
        assert_eq!(
            c.last().unwrap(),
            "hi",
            "claude 走 --append-system-prompt，提示词不动"
        );
    }

    #[test]
    fn codex_sandbox_from_permission_mode() {
        let mut s = SessionSpec::new("/p", "hi");
        s.permission_mode = Some("workspace-write".into());
        let a = rt("codex").exec_spec(&s, None).args;
        assert!(a.windows(2).any(|w| w == ["-s", "workspace-write"]));

        s.permission_mode = Some("acceptEdits".into());
        let a = rt("codex").exec_spec(&s, None).args;
        assert!(!a.iter().any(|x| x == "acceptEdits"));

        s.permission_mode = Some("danger-full-access".into());
        s.remote_hands = Some(hands());
        let a = rt("codex").exec_spec(&s, None).args.join(" ");
        assert!(a.contains("-s read-only") && !a.contains("danger-full-access"));
    }

    #[test]
    fn flag_settings_only_for_claude() {
        let mut s = SessionSpec::new("/p", "hi");
        assert!(
            !rt("claude")
                .exec_spec(&s, None)
                .args
                .iter()
                .any(|a| a == "--settings"),
            "什么都没设时不带 --settings"
        );
        s.output_style = Some("Explanatory".into());
        s.fast_mode = Some(true);
        s.thinking = Some(false);
        let c = rt("claude").exec_spec(&s, None).args;
        let i = c.iter().position(|a| a == "--settings").unwrap();
        let v: serde_json::Value = serde_json::from_str(&c[i + 1]).unwrap();
        assert_eq!(v["outputStyle"], "Explanatory");
        assert_eq!(v["fastMode"], true);
        assert_eq!(v["alwaysThinkingEnabled"], false);
        assert!(
            !rt("codex")
                .exec_spec(&s, None)
                .args
                .iter()
                .any(|a| a == "--settings")
        );
        s.thinking = Some(true);
        s.fast_mode = None;
        s.output_style = Some("bad\nstyle".into());
        assert!(flag_settings(&s).is_none(), "思考默认开；非法风格名丢弃");
    }

    #[test]
    fn remote_hands_codex_disables_local_shell() {
        let r = rt("codex");
        let mut s = SessionSpec::new("/local/placeholder", "hi");
        s.remote_hands = Some(hands());
        let e = r.exec_spec(&s, None);
        let joined = e.args.join(" ");
        assert!(
            joined.contains("--disable shell_tool") && joined.contains("--disable unified_exec")
        );
        assert!(joined.contains("-s read-only"));
        assert!(joined.contains(
            r#"mcp_servers.blazar.command="/Applications/Blazar.app/Contents/MacOS/blazar-desktop""#
        ));
        assert!(joined.contains(
            r#"mcp_servers.blazar.args=["__mcp-remote","--node","gpu-1","--root","/home/me/proj"]"#
        ));
        assert!(
            !joined.contains("--mcp-config"),
            "claude 专属参数不能给 codex"
        );
    }

    #[test]
    fn non_claude_agents_do_not_get_claude_only_flags() {
        let r = rt("codex");
        let mut s = SessionSpec::new("/w", "hi");
        s.permission_mode = Some("acceptEdits".into());
        s.disallowed_tools = vec!["Bash".into()];
        let e = r.exec_spec(&s, None);
        assert!(!e.args.iter().any(|a| a == "--permission-mode"));
        assert!(!e.args.iter().any(|a| a == "--disallowedTools"));
    }

    #[tokio::test]
    async fn resume_on_unsupported_agent_errors_clearly() {
        let r = rt("qwen");
        let res = r
            .resume(&ProviderSessionId("x".into()), SessionSpec::new("/w", "hi"))
            .await;
        match res {
            Err(RuntimeError::Unsupported(_)) => {}
            Err(other) => panic!("期望 Unsupported，实得 {other}"),
            Ok(_) => panic!("不支持续接的 agent 不应成功启动"),
        }
    }

    #[test]
    fn plain_lines_becomes_assistant_text() {
        let p = parse_by_format(OutputFormat::PlainLines, "只是一行普通输出");
        assert_eq!(p.len(), 1);
        assert!(matches!(p[0].0, EntryKind::AssistantMessage { .. }));
    }

    #[tokio::test]
    async fn silent_agent_reports_failure_not_success() {
        let r = rt("claude").with_program("true");
        let mut h = r.start(SessionSpec::new(".", "hi")).await.unwrap();
        let entries: Vec<_> = {
            let mut v = Vec::new();
            while let Some(e) = h.events.next().await {
                v.push(e);
            }
            v
        };
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].kind,
            EntryKind::Finished(Outcome::Failed { .. })
        ));
    }

    #[test]
    fn custom_args_reach_the_command_line() {
        let r = rt("claude");
        let mut spec = SessionSpec::new("/w", "hi");
        spec.extra_args = vec!["--fallback-model".into(), "sonnet".into()];
        let exec = r.exec_spec(&spec, None);
        let joined = exec.args.join(" ");
        assert!(
            joined.contains("--fallback-model sonnet"),
            "自定义参数必须出现在命令行里: {joined}"
        );
        assert!(
            joined.ends_with("hi"),
            "提示词仍要排在最后，否则会被当成上一个 flag 的取值: {joined}"
        );
    }

    #[test]
    fn variadic_flags_never_swallow_the_prompt() {
        let r = rt("claude");
        let mut s = SessionSpec::new("/w", "reply: ok");
        s.disallowed_tools = vec!["Bash".into(), "WebFetch".into()];
        s.extra_args = vec!["--add-dir".into(), "/data".into()];
        let a = r.exec_spec(&s, None).args;
        let n = a.len();
        assert_eq!(a[n - 1], "reply: ok");
        assert_eq!(a[n - 2], "--", "提示词前必须有选项结束符: {a:?}");
    }

    #[test]
    fn claude_detached_plan_sends_the_prompt_through_stdin() {
        let r = rt("claude");
        let mut s = SessionSpec::new("/w", "改一下 solver");
        s.permission_mode = Some("default".into());
        let p = r.detached_plan(&s, None, Some("11111111-2222-3333-4444-555555555555"));
        let a = &p.spec.args;
        assert_eq!(p.mode, RunMode::Relay);
        assert!(
            a.windows(2).any(|w| w == ["--input-format", "stream-json"]),
            "{a:?}"
        );
        assert!(
            a.windows(2)
                .any(|w| w == ["--permission-prompt-tool", "stdio"]),
            "{a:?}"
        );
        assert!(a.iter().any(|x| x == "--replay-user-messages"));
        assert!(a.windows(2).any(|w| w[0] == "--session-id"));
        assert!(
            !a.iter().any(|x| x == "改一下 solver"),
            "提示词不能再作为位置参数: {a:?}"
        );
        let first = p.first_input.unwrap();
        let (id, json) = first.split_once('\t').unwrap();
        assert_eq!(id, "u-1");
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["message"]["content"][0]["text"], "改一下 solver");
    }

    #[test]
    fn resuming_uses_resume_not_session_id() {
        let r = rt("claude");
        let p = r.detached_plan(
            &SessionSpec::new("/w", "继续"),
            Some(&ProviderSessionId("abc".into())),
            Some("ignored"),
        );
        assert!(p.spec.args.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(!p.spec.args.iter().any(|x| x == "--session-id"));
    }

    #[test]
    fn non_interactive_agents_keep_the_positional_prompt_and_a_closed_stdin() {
        let r = rt("codex");
        assert!(!r.interactive());
        let p = r.detached_plan(&SessionSpec::new("/w", "hi codex"), None, None);
        assert_eq!(p.mode, RunMode::Null);
        assert!(p.first_input.is_none());
        assert_eq!(p.spec.args.last().map(String::as_str), Some("hi codex"));
    }

    #[test]
    fn approval_answers_have_the_shape_claude_expects() {
        let input = serde_json::json!({"command": "touch x"});
        let allow: serde_json::Value =
            serde_json::from_str(&approval_response_json("r1", true, Some(&input), "")).unwrap();
        assert_eq!(allow["type"], "control_response");
        assert_eq!(allow["response"]["request_id"], "r1");
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(
            allow["response"]["response"]["updatedInput"]["command"], "touch x",
            "2.1.37 不带 updatedInput 会以 ZodError 拒收"
        );
        let deny: serde_json::Value =
            serde_json::from_str(&approval_response_json("r1", false, None, "别碰它")).unwrap();
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert_eq!(deny["response"]["response"]["message"], "别碰它");
    }
}
