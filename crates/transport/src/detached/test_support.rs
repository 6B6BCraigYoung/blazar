use std::sync::Arc;

use crate::{ExecSpec, LineStream, LocalTransport, NodeTransport, Result};

use super::{DetachedRun, RunMode, RunState};

pub(super) struct TestRun {
    pub(super) run: DetachedRun,
    pub(super) _root: tempfile::TempDir,
}

impl std::ops::Deref for TestRun {
    type Target = DetachedRun;

    fn deref(&self) -> &Self::Target {
        &self.run
    }
}

pub(super) async fn launch(
    transport: Arc<dyn NodeTransport>,
    id: &str,
    spec: &ExecSpec,
    input: Option<&str>,
    mode: RunMode,
) -> Result<TestRun> {
    let root = tempfile::tempdir()?;
    let run = DetachedRun::launch_in(transport, root.path(), id, spec, input, mode).await?;
    Ok(TestRun { run, _root: root })
}

struct TestLocal;

pub(super) fn without_login(mut spec: ExecSpec) -> ExecSpec {
    if spec.program == "bash" && spec.args.first().is_some_and(|a| a == "-lc") {
        spec.args[0] = "-c".into();
    }
    spec.env("BASH_ENV", "/dev/null")
}

#[async_trait::async_trait]
impl NodeTransport for TestLocal {
    fn kind(&self) -> crate::TransportKind {
        crate::TransportKind::Local
    }

    fn target(&self) -> &str {
        "local"
    }

    async fn exec(&self, spec: ExecSpec) -> Result<crate::ExecOutput> {
        LocalTransport.exec(without_login(spec)).await
    }

    async fn spawn_lines(&self, spec: ExecSpec) -> Result<LineStream> {
        LocalTransport.spawn_lines(without_login(spec)).await
    }
}

pub(super) fn local() -> Arc<dyn NodeTransport> {
    Arc::new(TestLocal)
}

pub(super) fn rid(tag: &str) -> String {
    format!(
        "t-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

pub(super) async fn wait_exit(run: &DetachedRun) -> RunState {
    for _ in 0..60 {
        let st = run.probe().await.unwrap();
        if matches!(st, RunState::Exited { .. }) {
            return st;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    run.probe().await.unwrap()
}
