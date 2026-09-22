use std::sync::Arc;

use async_trait::async_trait;
use blazar_core_types::{NormalizedEntry, ProviderSessionId, RuntimeKind};
use blazar_transport::{ExecSpec, NodeTransport};
use chrono::Utc;
use futures::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::{AgentRuntime, RuntimeError, SessionHandle, SessionSpec};

pub mod parse;

pub struct ClaudeCliRuntime {
    transport: Arc<dyn NodeTransport>,
    program: String,
}

impl ClaudeCliRuntime {
    #[must_use]
    pub fn new(transport: Arc<dyn NodeTransport>) -> Self {
        Self {
            transport,
            program: "claude".to_owned(),
        }
    }

    #[must_use]
    pub fn with_program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    fn build_spec(&self, spec: &SessionSpec, resume: Option<&ProviderSessionId>) -> ExecSpec {
        let mut e = ExecSpec::new(&self.program)
            .arg("-p")
            .arg(&spec.prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .cwd(&spec.cwd);

        if let Some(id) = resume {
            e = e.arg("--resume").arg(&id.0);
        }
        if let Some(model) = &spec.model {
            e = e.arg("--model").arg(model);
        }
        if let Some(mode) = &spec.permission_mode {
            e = e.arg("--permission-mode").arg(mode);
        }
        if !spec.allowed_tools.is_empty() {
            e = e.arg("--allowedTools").args(spec.allowed_tools.clone());
        }
        if !spec.disallowed_tools.is_empty() {
            e = e
                .arg("--disallowedTools")
                .args(spec.disallowed_tools.clone());
        }
        for (k, v) in &spec.env {
            e = e.env(k, v);
        }
        e
    }

    async fn run(
        &self,
        spec: &SessionSpec,
        resume: Option<&ProviderSessionId>,
    ) -> Result<SessionHandle, RuntimeError> {
        let exec = self.build_spec(spec, resume);
        let mut lines = self
            .transport
            .spawn_lines(exec)
            .await
            .map_err(|e| RuntimeError::Transport(e.to_string()))?;

        let killer = lines.killer.clone();

        let (tx, rx) = tokio::sync::mpsc::channel::<NormalizedEntry>(1024);

        tokio::spawn(async move {
            let mut seq: u64 = 1;
            while let Some(line) = lines.stdout.next().await {
                if line.trim().is_empty() {
                    continue;
                }
                let Some(parsed) = parse::parse_line(&line) else {
                    tracing::warn!(target: "blazar::claude", "无法解析的输出行，已跳过");
                    continue;
                };
                for kind in parsed.entries {
                    let entry = NormalizedEntry {
                        seq,
                        ts: Utc::now(),
                        parent_tool_use_id: parsed.parent_tool_use_id.clone(),
                        kind,
                    };
                    seq += 1;
                    if tx.send(entry).await.is_err() {
                        return;
                    }
                }
            }
            drop(lines);
        });

        Ok(SessionHandle {
            events: ReceiverStream::new(rx).boxed(),
            killer: Some(killer),
        })
    }
}

#[async_trait]
impl AgentRuntime for ClaudeCliRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::ClaudeCli
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
