use crate::{Result, TransportError};

use super::{DetachedRun, IDENTITY_FNS, in_dir, sh};

impl DetachedRun {
    pub async fn stop_adopted(&self) -> Result<()> {
        let body = format!(
            r#"{IDENTITY_FNS}
alive || exit 0
read -r P B S < pid
builtin kill -TERM "$P" 2>/dev/null
i=0; while [ $i -lt 50 ]; do alive || exit 0; sleep 0.1; i=$((i+1)); done
builtin kill -KILL "$P" 2>/dev/null
i=0; while [ $i -lt 20 ]; do alive || exit 0; sleep 0.1; i=$((i+1)); done
echo '进程还在运行，无法确认已经停止' >&2; exit 15"#
        );
        self.transport
            .exec(sh(in_dir(&self.dir, &body)))
            .await?
            .ok()?;
        Ok(())
    }

    pub async fn hard_kill(&self) -> Result<()> {
        self.hard_kill_with(signal_local).await
    }

    async fn hard_kill_with(&self, local_signal: impl Fn(i32, bool) -> Result<()>) -> Result<()> {
        let initial = self.stop_status().await?;
        if let StopStatus::Alive(pid) = initial {
            self.signal(pid, false, &local_signal).await?;
        }
        let mut status = self.wait_stopped(initial).await?;
        if let StopStatus::Alive(pid) = status {
            self.signal(pid, true, &local_signal).await?;
            status = self.wait_stopped(status).await?;
        }
        if status != StopStatus::Stopped {
            return Err(stop_error("进程组仍在运行，无法确认已经停止"));
        }
        self.transport
            .exec(sh(in_dir(
                &self.dir,
                r#"[ -e exit.code ] && exit 0
printf '\n{"type":"blazar_exit","code":137}\n' >> out.jsonl &&
echo 137 > exit.code.tmp && mv exit.code.tmp exit.code"#,
            )))
            .await?
            .ok()?;
        Ok(())
    }

    async fn stop_status(&self) -> Result<StopStatus> {
        let body = format!(
            r#"{IDENTITY_FNS}
read -r P B S < pid 2>/dev/null || {{ echo '无法读取运行进程身份' >&2; exit 15; }}
case "$P" in ''|*[!0-9]*) echo '运行 PID 无效' >&2; exit 15;; esac
[ "$P" -gt 1 ] || exit 15
TABLE=$(ps -eo pid=,pgid=,stat=) || {{ echo '无法检查进程组' >&2; exit 15; }}
STATE=$(printf '%s\n' "$TABLE" | awk -v p="$P" '$1 == p {{print $3}}')
if [ -n "$STATE" ] && [ "${{STATE#Z}}" = "$STATE" ]; then
  alive || {{ echo '运行进程身份不匹配，无法确认已经停止' >&2; exit 15; }}
  PG=$(printf '%s\n' "$TABLE" | awk -v p="$P" '$1 == p {{print $2}}')
  [ "$PG" = "$P" ] || {{ echo '运行进程组身份不匹配' >&2; exit 15; }}
  echo "alive=$P"
elif printf '%s\n' "$TABLE" | awk -v p="$P" '$2 == p && $3 !~ /^Z/ {{found=1}} END {{exit !found}}'; then
  echo waiting
else
  echo stopped
fi"#
        );
        let out = self
            .transport
            .exec(sh(in_dir(&self.dir, &body)))
            .await?
            .ok()?;
        parse_stop_status(out.trim())
    }

    async fn wait_stopped(&self, mut status: StopStatus) -> Result<StopStatus> {
        for _ in 0..20 {
            if status == StopStatus::Stopped {
                return Ok(status);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            status = self.stop_status().await?;
        }
        Ok(status)
    }

    async fn signal(
        &self,
        pid: i32,
        force: bool,
        local_signal: &impl Fn(i32, bool) -> Result<()>,
    ) -> Result<()> {
        match self.stop_status().await? {
            StopStatus::Stopped => return Ok(()),
            StopStatus::Alive(current) if current == pid => {}
            _ => return Err(stop_error("运行身份已变化，请重新确认进程状态后重试")),
        }
        match self.transport.kind() {
            crate::TransportKind::Local => local_signal(pid, force),
            crate::TransportKind::Ssh => {
                let command = if force {
                    "builtin kill -KILL -- \"-$P\""
                } else {
                    "builtin kill -TERM \"$P\""
                };
                let body = format!(
                    r#"{IDENTITY_FNS}
alive || {{ echo '运行身份已变化' >&2; exit 15; }}
read -r P B S < pid
[ "$P" = '{pid}' ] || exit 15
{command}"#
                );
                self.transport
                    .exec(sh(in_dir(&self.dir, &body)))
                    .await?
                    .ok()?;
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopStatus {
    Alive(i32),
    Waiting,
    Stopped,
}

pub(super) fn stop_error(message: impl Into<String>) -> TransportError {
    TransportError::Command {
        code: 15,
        stderr: message.into(),
    }
}

fn parse_stop_status(raw: &str) -> Result<StopStatus> {
    match raw {
        "stopped" => Ok(StopStatus::Stopped),
        "waiting" => Ok(StopStatus::Waiting),
        _ => raw
            .strip_prefix("alive=")
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|pid| *pid > 1)
            .map(StopStatus::Alive)
            .ok_or_else(|| stop_error("无法确认运行进程组状态")),
    }
}

#[cfg(unix)]
fn signal_local(pid: i32, force: bool) -> Result<()> {
    use nix::sys::signal::{Signal, kill, killpg};
    use nix::unistd::Pid;
    if pid <= 1 {
        return Err(stop_error("运行 PID 无效"));
    }
    let pid = Pid::from_raw(pid);
    let result = if force {
        killpg(pid, Signal::SIGKILL)
    } else {
        kill(pid, Signal::SIGTERM)
    };
    match result {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(stop_error(format!("无法停止本机运行: {error}"))),
    }
}

#[cfg(not(unix))]
fn signal_local(_pid: i32, _force: bool) -> Result<()> {
    Err(stop_error(
        "此平台暂不支持安全停止本机后台进程组，请手动停止后重试",
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::detached::test_support::{local, rid};
    use crate::detached::{ProbeItem, RunState, probe_many};
    use crate::{ExecSpec, LineStream, LocalTransport, NodeTransport};

    #[tokio::test]
    async fn an_adopted_process_is_watched_and_can_be_stopped() {
        let root = tempfile::tempdir().unwrap();
        let spawn = |secs: u32| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("sleep {secs} >/dev/null 2>&1 & echo $!"))
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u32>()
                .unwrap()
        };
        let transport = local();
        let long = DetachedRun::adopt(transport.clone(), root.path(), &rid("ad-long"), spawn(30))
            .await
            .unwrap();
        let short = DetachedRun::adopt(transport.clone(), root.path(), &rid("ad-short"), spawn(1))
            .await
            .unwrap();
        let probe = |runs: Vec<String>| {
            let transport = transport.clone();
            async move {
                let items: Vec<ProbeItem> = runs
                    .into_iter()
                    .map(|dir| ProbeItem {
                        dir,
                        tail_of: None,
                        tail_lines: 1,
                        ..ProbeItem::default()
                    })
                    .collect();
                probe_many(&transport, &items).await.unwrap()
            }
        };
        let both = probe(vec![long.dir.clone(), short.dir.clone()]).await;
        assert!(
            both.iter()
                .all(|p| matches!(p.state, RunState::Alive { .. })),
            "{both:?}"
        );
        let mut ended = false;
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if matches!(
                probe(vec![short.dir.clone()]).await[0].state,
                RunState::Lost { .. }
            ) {
                ended = true;
                break;
            }
        }
        assert!(ended, "自己退出的进程要被看出来");
        long.stop_adopted().await.unwrap();
        assert!(matches!(
            probe(vec![long.dir.clone()]).await[0].state,
            RunState::Lost { .. }
        ));
        assert!(
            DetachedRun::adopt(transport, root.path(), &rid("ad-none"), 999_999)
                .await
                .is_err()
        );
    }

    struct FailedKillTransport;

    #[async_trait::async_trait]
    impl NodeTransport for FailedKillTransport {
        fn kind(&self) -> crate::TransportKind {
            crate::TransportKind::Ssh
        }

        fn target(&self) -> &str {
            "gpu-1"
        }

        async fn exec(&self, spec: ExecSpec) -> Result<crate::ExecOutput> {
            let body = spec
                .args
                .last()
                .unwrap()
                .replace(IDENTITY_FNS, "alive() { return 0; }\n")
                .replace("builtin kill ", "fixture_kill ")
                .replace("kill ", "fixture_kill ");
            let stub = "fixture_kill() { echo 'fixture signal refused' >&2; return 1; }; fixture_fixture_kill() { fixture_kill; }; sleep() { :; }; grep() { return 1; }; ps() { printf '12345 12345 S\\n'; }; ";
            LocalTransport
                .exec(
                    ExecSpec::new("bash")
                        .arg("--noprofile")
                        .arg("--norc")
                        .arg("-c")
                        .arg(format!("{stub}{body}"))
                        .env("BASH_ENV", "/dev/null"),
                )
                .await
        }

        async fn spawn_lines(&self, _spec: ExecSpec) -> Result<LineStream> {
            panic!("hard kill must not launch a stream")
        }
    }

    #[tokio::test]
    async fn hard_kill_reports_signal_failure_without_forging_exit() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("pid"), "12345 fixture fixture\n").unwrap();
        std::fs::write(root.path().join("out.jsonl"), "").unwrap();
        let run = DetachedRun::attach(
            Arc::new(FailedKillTransport),
            root.path().display().to_string(),
            "fixture".into(),
        );
        let error = run.hard_kill().await.unwrap_err();
        assert!(
            matches!(error, TransportError::Command { code: 1, ref stderr } if stderr.contains("fixture signal refused"))
        );
        assert!(!root.path().join("exit.code").exists());
    }

    struct LocalSignalTransport {
        stopped: std::sync::atomic::AtomicBool,
        finalized: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl NodeTransport for LocalSignalTransport {
        fn kind(&self) -> crate::TransportKind {
            crate::TransportKind::Local
        }

        fn target(&self) -> &str {
            "local"
        }

        async fn exec(&self, spec: ExecSpec) -> Result<crate::ExecOutput> {
            use std::sync::atomic::Ordering;
            let body = spec.args.last().unwrap();
            let stdout = if body.contains("TABLE=$(ps") {
                if self.stopped.load(Ordering::SeqCst) {
                    "stopped"
                } else {
                    "alive=12345"
                }
            } else {
                assert!(!body.contains("kill -"));
                self.finalized.store(true, Ordering::SeqCst);
                ""
            };
            Ok(crate::ExecOutput {
                code: 0,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        }

        async fn spawn_lines(&self, _spec: ExecSpec) -> Result<LineStream> {
            panic!("hard kill must not launch a stream")
        }
    }

    #[tokio::test]
    async fn hard_kill_local_confirms_injected_signal_without_executing_shell_kill() {
        use std::sync::atomic::Ordering;
        let fake = Arc::new(LocalSignalTransport {
            stopped: std::sync::atomic::AtomicBool::new(false),
            finalized: std::sync::atomic::AtomicBool::new(false),
        });
        let run = DetachedRun::attach(fake.clone(), "/fixture/run".into(), "fixture".into());
        run.hard_kill_with(|pid, force| {
            assert_eq!(pid, 12345);
            assert!(!force);
            fake.stopped.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert!(fake.finalized.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn hard_kill_local_signal_error_does_not_finalize_the_run() {
        use std::sync::atomic::Ordering;
        let fake = Arc::new(LocalSignalTransport {
            stopped: std::sync::atomic::AtomicBool::new(false),
            finalized: std::sync::atomic::AtomicBool::new(false),
        });
        let run = DetachedRun::attach(fake.clone(), "/fixture/run".into(), "fixture".into());
        assert!(
            run.hard_kill_with(|_, _| Err(stop_error("fixture signal refused")))
                .await
                .is_err()
        );
        assert!(!fake.finalized.load(Ordering::SeqCst));
    }

    #[test]
    fn hard_kill_rejects_reserved_or_invalid_process_ids() {
        for value in [
            "alive=-1",
            "alive=0",
            "alive=1",
            "alive=2147483648",
            "exited=0",
            "",
        ] {
            assert!(parse_stop_status(value).is_err(), "{value}");
        }
        assert_eq!(
            parse_stop_status("alive=12345").unwrap(),
            StopStatus::Alive(12345)
        );
    }

    #[tokio::test]
    async fn hard_kill_does_not_trust_exit_markers_when_processes_are_alive() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("pid"), "12345 fixture fixture\n").unwrap();
        std::fs::write(root.path().join("out.jsonl"), "").unwrap();
        std::fs::write(root.path().join("exit.code"), "137").unwrap();
        let run = DetachedRun::attach(
            Arc::new(FailedKillTransport),
            root.path().display().to_string(),
            "fixture".into(),
        );
        assert!(run.hard_kill().await.is_err());
    }

    #[cfg(unix)]
    struct OwnedProcess(std::process::Child);

    #[cfg(unix)]
    impl OwnedProcess {
        fn sleep(own_group: bool) -> Self {
            use std::os::unix::process::CommandExt;
            let mut command = std::process::Command::new("sleep");
            command
                .arg("30")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if own_group {
                command.process_group(0);
            }
            Self(command.spawn().unwrap())
        }
    }

    #[cfg(unix)]
    impl Drop for OwnedProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(unix)]
    async fn mismatched_identity_is_not_stopped(newline: bool) {
        let root = tempfile::tempdir().unwrap();
        let original = OwnedProcess::sleep(true);
        let mut bystander = OwnedProcess::sleep(false);
        let script = format!(
            "{IDENTITY_FNS}printf '%s %s %s\\n' '{pid}' \"$(boot_of)\" \"$(start_of {pid})\"",
            pid = original.0.id(),
        );
        let identity = LocalTransport
            .exec(
                ExecSpec::new("bash")
                    .arg("--noprofile")
                    .arg("--norc")
                    .arg("-c")
                    .arg(script)
                    .env("BASH_ENV", "/dev/null"),
            )
            .await
            .unwrap()
            .ok()
            .unwrap();
        let pidfile = root.path().join("pid");
        let invalid = format!(
            "{} {} not-the-same-start{}",
            bystander.0.id(),
            identity.split_whitespace().nth(1).unwrap(),
            if newline { "\n" } else { "" },
        );
        std::fs::write(&pidfile, invalid).unwrap();
        std::fs::write(root.path().join("out.jsonl"), "").unwrap();
        let run = DetachedRun::attach(local(), root.path().display().to_string(), rid("identity"));
        let probe = run.probe().await.unwrap();
        let signals = std::cell::Cell::new(0);
        let result = run
            .hard_kill_with(|_, _| {
                signals.set(signals.get() + 1);
                Err(stop_error(
                    "fixture forbids signaling a mismatched identity",
                ))
            })
            .await;
        let forged_exit = root.path().join("exit.code").exists();
        let preserved_log = std::fs::read(root.path().join("out.jsonl"))
            .unwrap()
            .is_empty();
        let untouched = bystander.0.try_wait().unwrap().is_none();
        std::fs::write(&pidfile, identity).unwrap();
        let cleanup = run.hard_kill().await;
        assert!(cleanup.is_ok(), "owned process cleanup failed: {cleanup:?}");
        assert!(result.is_err(), "a changed identity cannot confirm exit");
        assert!(matches!(probe, RunState::Lost { .. }));
        assert!(
            preserved_log,
            "identity mismatch must not forge an exit event"
        );
        assert_eq!(
            signals.get(),
            0,
            "a mismatched identity must not be signaled"
        );
        assert!(
            !forged_exit,
            "identity mismatch must not forge an exit marker"
        );
        assert!(untouched, "the unrelated owned process must still be alive");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn identity_mismatch_with_newline_cannot_confirm_exit() {
        mismatched_identity_is_not_stopped(true).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn identity_mismatch_without_newline_cannot_confirm_exit() {
        mismatched_identity_is_not_stopped(false).await;
    }
}
