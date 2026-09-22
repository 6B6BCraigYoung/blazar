use std::io::{Read, Write};
use std::sync::Arc;

use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use tokio::sync::{Mutex, mpsc};

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("创建 PTY 失败: {0}")]
    Pty(String),

    #[error("终端会话已结束")]
    Closed,
}

pub type Result<T> = std::result::Result<T, TerminalError>;

pub const TERM: &str = "xterm-256color";

#[derive(Debug, Clone)]
pub enum TerminalTarget {
    Local {
        cwd: String,

        tmux_session: Option<String>,
    },

    Ssh {
        host: String,
        cwd: String,
        tmux_session: Option<String>,
    },
}

fn shell_command(cwd: &str, session: Option<&str>) -> String {
    let term = format!("export TERM={TERM}; ");
    let login = format!("{term}cd {} 2>/dev/null; exec $SHELL -l", shell_quote(cwd));
    match session {
        Some(name) => format!(
            "{term}if command -v tmux >/dev/null 2>&1; then \
               exec tmux new-session -A -s {name_q} -c {cwd_q} \\; \
                    set-option -t {name_q} status off; \
             else {login}; fi",
            name_q = shell_quote(name),
            cwd_q = shell_quote(cwd),
        ),
        None => login,
    }
}

#[doc(hidden)]
#[must_use]
pub fn debug_shell_command(cwd: &str, session: Option<&str>) -> String {
    shell_command(cwd, session)
}

impl TerminalTarget {
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        match self {
            Self::Local { tmux_session, .. } | Self::Ssh { tmux_session, .. } => {
                tmux_session.is_some()
            }
        }
    }

    fn command(&self) -> CommandBuilder {
        match self {
            Self::Local { cwd, tmux_session } => {
                let mut cmd = CommandBuilder::new("bash");
                cmd.args(["-lc", &shell_command(cwd, tmux_session.as_deref())]);
                cmd.cwd(cwd);
                cmd.env("TERM", TERM);
                cmd
            }
            Self::Ssh {
                host,
                cwd,
                tmux_session,
            } => {
                let mut cmd = CommandBuilder::new("ssh");

                cmd.args([
                    "-tt",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "NumberOfPasswordPrompts=0",
                    "-o",
                    "StrictHostKeyChecking=accept-new",
                    "-o",
                    "ServerAliveInterval=30",
                    host,
                    "--",
                    &shell_command(cwd, tmux_session.as_deref()),
                ]);
                cmd
            }
        }
    }

    fn detach_command(&self) -> Option<(String, Vec<String>)> {
        let (host, name) = match self {
            Self::Local { tmux_session, .. } => (None, tmux_session.as_deref()?),
            Self::Ssh {
                host, tmux_session, ..
            } => (Some(host.as_str()), tmux_session.as_deref()?),
        };
        let inner = format!("tmux detach-client -s {} 2>/dev/null", shell_quote(name));
        Some(match host {
            None => ("bash".to_owned(), vec!["-lc".to_owned(), inner]),
            Some(h) => (
                "ssh".to_owned(),
                vec![
                    "-o".to_owned(),
                    "BatchMode=yes".to_owned(),
                    "-o".to_owned(),
                    "NumberOfPasswordPrompts=0".to_owned(),
                    h.to_owned(),
                    "--".to_owned(),
                    inner,
                ],
            ),
        })
    }
}

#[must_use]
pub fn session_name(workspace_id: &str) -> String {
    let slug: String = workspace_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(24)
        .collect();
    format!("blazar-{}", if slug.is_empty() { "ws" } else { &slug })
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub struct TerminalHandle {
    pair: Arc<Mutex<PtyPair>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    target: TerminalTarget,
}

impl TerminalHandle {
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().await;
        w.write_all(data).map_err(|_| TerminalError::Closed)?;
        w.flush().map_err(|_| TerminalError::Closed)?;
        Ok(())
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.pair
            .lock()
            .await
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TerminalError::Pty(e.to_string()))
    }

    pub async fn close(&self) {
        let Some((program, args)) = self.target.detach_command() else {
            return;
        };
        let fut = tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), fut).await;
    }
}

pub struct TerminalSession {
    handle: Arc<TerminalHandle>,

    pub output: mpsc::Receiver<Vec<u8>>,
}

impl TerminalSession {
    pub fn open(target: &TerminalTarget, cols: u16, rows: u16) -> Result<Self> {
        let sys = NativePtySystem::default();
        let pair = sys
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TerminalError::Pty(e.to_string()))?;

        let mut child = pair
            .slave
            .spawn_command(target.command())
            .map_err(|e| TerminalError::Pty(e.to_string()))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| TerminalError::Pty(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| TerminalError::Pty(e.to_string()))?;

        let (tx, output) = mpsc::channel::<Vec<u8>>(256);

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }

            let _ = child.kill();
            let _ = child.wait();
        });

        Ok(Self {
            handle: Arc::new(TerminalHandle {
                pair: Arc::new(Mutex::new(pair)),
                writer: Arc::new(Mutex::new(writer)),
                target: target.clone(),
            }),
            output,
        })
    }

    #[must_use]
    pub fn handle(&self) -> Arc<TerminalHandle> {
        self.handle.clone()
    }

    #[must_use]
    pub fn split(self) -> (Arc<TerminalHandle>, mpsc::Receiver<Vec<u8>>) {
        (self.handle, self.output)
    }

    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.handle.write(data).await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.handle.resize(cols, rows).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_sessions_detach_on_close() {
        let local = TerminalTarget::Local {
            cwd: "/w".into(),
            tmux_session: Some("blazar-x".into()),
        };
        let (prog, args) = local.detach_command().expect("tmux 会话必须有 detach 命令");
        assert_eq!(prog, "bash");
        assert!(
            args.last().unwrap().contains("detach-client -s 'blazar-x'"),
            "要 detach 的是客户端，不是 kill-session: {args:?}"
        );

        let remote = TerminalTarget::Ssh {
            host: "gpu1".into(),
            cwd: "/w".into(),
            tmux_session: Some("blazar-y".into()),
        };
        let (prog, args) = remote.detach_command().unwrap();
        assert_eq!(prog, "ssh");
        assert!(args.contains(&"gpu1".to_owned()));
        assert!(args.contains(&"BatchMode=yes".to_owned()), "不能弹密码提示");

        assert!(
            TerminalTarget::Local {
                cwd: "/w".into(),
                tmux_session: None,
            }
            .detach_command()
            .is_none()
        );
    }

    #[test]
    fn session_name_is_tmux_safe() {
        let n = session_name("0192a7b0-1111-7222-8333-444455556666");
        assert!(n.starts_with("blazar-"));
        assert!(!n.contains('.') && !n.contains(':'));
        assert!(session_name("中文/名:字.x").is_ascii());
        assert!(!session_name("").is_empty(), "空 id 也要有个合法名字");
    }

    #[test]
    fn tmux_attaches_instead_of_creating_a_second_session() {
        let c = shell_command("/w", Some("blazar-x"));
        assert!(c.contains("new-session -A"), "必须用 -A 附着已有会话: {c}");
        assert!(
            c.contains("status off"),
            "面板里不该再挂 tmux 自己的状态栏: {c}"
        );
        assert!(c.contains("exec tmux"), "要 exec 掉，别留一层多余的 shell");

        assert!(c.contains("command -v tmux"), "必须探测 tmux 是否存在");
        assert!(c.contains("exec $SHELL -l"), "退化路径仍要是登录 shell");
    }

    #[test]
    fn no_session_means_plain_login_shell() {
        let c = shell_command("/w", None);
        assert!(!c.contains("tmux"));
        assert!(c.contains("exec $SHELL -l"));
    }

    #[test]
    fn term_is_always_exported() {
        for c in [shell_command("/w", Some("s")), shell_command("/w", None)] {
            assert!(
                c.contains("export TERM=xterm-256color"),
                "必须导出 TERM: {c}"
            );
        }
    }

    #[test]
    fn ssh_target_forces_remote_pty() {
        let t = TerminalTarget::Ssh {
            host: "gpu1".into(),
            cwd: "/home/me/my proj".into(),
            tmux_session: None,
        };
        let cmd = t.command();
        let args: Vec<_> = cmd
            .get_argv()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"-tt".to_owned()));

        assert!(args.iter().any(|a| a.contains(r"'/home/me/my proj'")));

        assert!(args.iter().any(|a| a.contains("exec $SHELL -l")));
    }

    #[tokio::test]
    async fn local_session_echoes() {
        let t = TerminalTarget::Local {
            cwd: std::env::temp_dir().display().to_string(),
            tmux_session: None,
        };
        let mut s = TerminalSession::open(&t, 80, 24).expect("应能开 PTY");
        s.write(b"echo blazar_pty_ok\n").await.unwrap();

        let mut seen = String::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_secs(2), s.output.recv()).await {
                Ok(Some(chunk)) => {
                    seen.push_str(&String::from_utf8_lossy(&chunk));
                    if seen.contains("blazar_pty_ok") {
                        return;
                    }
                }
                _ => break,
            }
        }
        panic!("PTY 未回显预期输出，实得: {seen:?}");
    }

    #[tokio::test]
    async fn resize_does_not_error() {
        let t = TerminalTarget::Local {
            cwd: std::env::temp_dir().display().to_string(),
            tmux_session: None,
        };
        let s = TerminalSession::open(&t, 80, 24).unwrap();
        s.resize(120, 40).await.expect("resize 应成功");
    }
}
