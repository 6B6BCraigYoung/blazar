use std::sync::Arc;

use crate::{ExecSpec, LineStream, NodeTransport, Result, TransportError, shell_quote};

pub const RUNS_ROOT: &str = "$HOME/.blazar/runs";

const IDENTITY_FNS: &str = r#"boot_of() { cat /proc/sys/kernel/random/boot_id 2>/dev/null || sysctl -n kern.boottime 2>/dev/null | sed 's/.*sec = \([0-9]*\).*/\1/'; }
start_of() { awk '{print $22}' "/proc/$1/stat" 2>/dev/null || ps -o lstart= -p "$1" 2>/dev/null | tr -s ' ' '_'; }
alive() {
  read -r P B S < pid 2>/dev/null || return 1
  kill -0 "$P" 2>/dev/null || return 1
  [ "$(boot_of)" = "$B" ] || return 1
  [ "$(start_of "$P")" = "$S" ] || return 1
}
"#;

const RUN_SH: &str = r#"D=$1; MODE=$2; RID=$3
cd "$D" || exit 97
trap '' HUP
__IDENTITY__
printf '%s %s %s\n' "$$" "$(boot_of)" "$(start_of $$)" > pid.tmp && mv pid.tmp pid
TAB=$(printf '\t')
relay() {
  exec 4< in.jsonl
  local l buf= idle=0
  while :; do
    if IFS= read -r l <&4; then
      l=$buf$l; buf=; idle=0
      case $l in "EOF$TAB"*) return 0;; esac
      printf '%s\n' "${l#*$TAB}" || return 0
    else
      buf=$buf$l
      idle=$((idle+1))
      if [ $idle -lt 25 ]; then sleep 0.2; else sleep 1; fi
    fi
  done
}
export BLAZAR_RUN=$RID
if [ "$MODE" = relay ]; then
  bash cmd.sh < <(relay) > out.jsonl 2> err.txt &
else
  bash cmd.sh < /dev/null > out.jsonl 2> err.txt &
fi
A=$!
trap 'kill -TERM $A 2>/dev/null' TERM INT
while :; do wait $A; E=$?; kill -0 $A 2>/dev/null || break; done
orphans() { grep -lz "^BLAZAR_RUN=$RID\$" /proc/[0-9]*/environ 2>/dev/null | sed 's#/proc/\([0-9]*\)/environ#\1#' | grep -vx "$$"; }
L=$(orphans | tr '\n' ' ')
if [ -n "$L" ]; then kill -TERM $L 2>/dev/null; sleep 2; L=$(orphans | tr '\n' ' '); [ -n "$L" ] && kill -KILL $L 2>/dev/null; fi
printf '\n{"type":"blazar_exit","code":%d}\n' "$E" >> out.jsonl
echo $E > exit.code.tmp && mv exit.code.tmp exit.code
trap '' TERM; kill -TERM 0 2>/dev/null
exit 0
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Relay,

    Null,
}

impl RunMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Relay => "relay",
            Self::Null => "null",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Alive { size: u64, input_replaced: bool },

    Exited { code: i32, size: u64 },

    Lost { size: u64 },

    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appended {
    Written,

    Duplicate,
}

#[derive(Clone)]
pub struct DetachedRun {
    transport: Arc<dyn NodeTransport>,

    pub dir: String,
    pub run_id: String,
}

impl std::fmt::Debug for DetachedRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetachedRun")
            .field("node", &self.transport.target())
            .field("dir", &self.dir)
            .finish()
    }
}

fn check_run_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(TransportError::Command {
            code: -1,
            stderr: format!("非法的 run id: {id:?}"),
        });
    }
    Ok(())
}

fn render_cmd(spec: &ExecSpec) -> Result<String> {
    spec.validate()?;
    let mut s = String::new();
    if !spec.program.contains('/') {
        s.push_str(crate::PATH_PRELUDE);
    }
    if let Some(cwd) = &spec.cwd {
        s.push_str(&format!(
            "cd {} || exit 96\n",
            shell_quote(&cwd.display().to_string())
        ));
    }
    for (k, v) in &spec.env {
        s.push_str(&format!("export {k}={}\n", shell_quote(v)));
    }
    for (k, p) in &spec.env_files {
        s.push_str(&format!(
            "export {k}=\"$(cat -- {})\"\n",
            shell_quote(&p.display().to_string())
        ));
    }
    s.push_str("exec ");
    s.push_str(&shell_quote(&spec.program));
    for a in &spec.args {
        s.push(' ');
        s.push_str(&shell_quote(a));
    }
    s.push('\n');
    Ok(s)
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn in_dir(dir: &str, body: &str) -> String {
    format!("cd {} 2>/dev/null || exit 94\n{body}", shell_quote(dir))
}

fn sh(script: String) -> ExecSpec {
    ExecSpec::new("bash").arg("-lc").arg(script)
}

impl DetachedRun {
    #[must_use]
    pub fn attach(transport: Arc<dyn NodeTransport>, dir: String, run_id: String) -> Self {
        Self {
            transport,
            dir,
            run_id,
        }
    }

    pub async fn launch(
        transport: Arc<dyn NodeTransport>,
        run_id: &str,
        cmd: &ExecSpec,
        first_input: Option<&str>,
        mode: RunMode,
    ) -> Result<Self> {
        Self::launch_at(
            transport,
            &format!("\"{RUNS_ROOT}\""),
            run_id,
            cmd,
            first_input,
            mode,
        )
        .await
    }

    pub async fn launch_in(
        transport: Arc<dyn NodeTransport>,
        root: &std::path::Path,
        run_id: &str,
        cmd: &ExecSpec,
        first_input: Option<&str>,
        mode: RunMode,
    ) -> Result<Self> {
        if !root.is_absolute() {
            return Err(TransportError::Command {
                code: -1,
                stderr: "运行目录必须是绝对路径".into(),
            });
        }
        Self::launch_at(
            transport,
            &shell_quote(&root.to_string_lossy()),
            run_id,
            cmd,
            first_input,
            mode,
        )
        .await
    }

    async fn launch_at(
        transport: Arc<dyn NodeTransport>,
        root: &str,
        run_id: &str,
        cmd: &ExecSpec,
        first_input: Option<&str>,
        mode: RunMode,
    ) -> Result<Self> {
        check_run_id(run_id)?;
        let run_sh = RUN_SH.replace("__IDENTITY__", IDENTITY_FNS);
        let payload = format!(
            "{}\n{}\n{}\n",
            b64(run_sh.as_bytes()),
            b64(render_cmd(cmd)?.as_bytes()),
            b64(first_input
                .filter(|_| mode == RunMode::Relay)
                .map(|l| format!("{l}\n"))
                .unwrap_or_default()
                .as_bytes()),
        );
        let script = format!(
            r#"set -e; umask 077
D={root}/{rid}
mkdir -p "$D"; cd "$D"
IFS= read -r A; IFS= read -r B; IFS= read -r C
printf '%s' "$A" | base64 --decode > run.sh
printf '%s' "$B" | base64 --decode > cmd.sh
printf '%s' "$C" | base64 --decode > in.jsonl
(stat -c %i in.jsonl 2>/dev/null || stat -f %i in.jsonl) > in.inode
set +e
set -m
bash "$D/run.sh" "$D" {mode} {rid} </dev/null >/dev/null 2>&1 &
i=0; while [ ! -s pid ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
[ -s pid ] || {{ echo "run.sh 5 秒内没有起来" >&2; exit 98; }}
echo "__BLAZAR_LAUNCHED__ $D"
"#,
            rid = run_id,
            mode = mode.as_str(),
        );
        let out = transport.exec(sh(script).stdin(payload)).await?;
        let dir = out
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("__BLAZAR_LAUNCHED__ "))
            .map(str::trim)
            .filter(|d| d.starts_with('/'))
            .ok_or_else(|| TransportError::Command {
                code: out.code,
                stderr: format!(
                    "启动常驻 agent 失败（退出码 {}）: {}",
                    out.code,
                    out.stderr.trim()
                ),
            })?
            .to_owned();
        Ok(Self {
            transport,
            dir,
            run_id: run_id.to_owned(),
        })
    }

    pub async fn append(&self, msg_id: &str, json_line: &str) -> Result<Appended> {
        if msg_id.contains(['\t', '\n', '\'', '"']) || json_line.contains('\n') {
            return Err(TransportError::Command {
                code: -1,
                stderr: "输入必须是单行，且 msg_id 不能含制表符/引号".into(),
            });
        }
        let body = format!(
            r#"export LC_ALL=C
umask 077
T=$(mktemp .blazar-input.XXXXXX) || exit 14
trap 'rm -f -- "$T"' EXIT
cat > "$T" || exit 14
[ "$(wc -l < "$T" | tr -d ' ')" = 1 ] && [ "$(tail -c 1 "$T" | od -An -tu1 | tr -d ' ')" = 10 ] || {{ echo "输入上传不完整，请重试" >&2; exit 14; }}
data=$(cat "$T") || exit 14
I=$(stat -c %i in.jsonl 2>/dev/null || stat -f %i in.jsonl 2>/dev/null)
[ -n "$I" ] && [ "$I" = "$(cat in.inode 2>/dev/null)" ] || {{ echo "in.jsonl 已被替换，输入无法送达" >&2; exit 12; }}
[ -e exit.code ] && {{ echo "agent 已经结束" >&2; exit 13; }}
if [ -s in.jsonl ] && [ "$(tail -c 1 in.jsonl | od -An -tu1 | tr -d ' ')" != 10 ]; then
  partial=$(tail -n 1 in.jsonl) || exit 14
  case "$data" in
    "$partial"*) printf '%s\n' "${{data#"$partial"}}" >> in.jsonl || exit 14; echo ok; exit 0 ;;
    *) echo "输入日志包含不匹配的未完成记录，已保留原数据；请恢复原输入后重试" >&2; exit 15 ;;
  esac
fi
existing=$(awk -F'\t' -v id={id} '$1==id{{print;exit}}' in.jsonl) || exit 14
if [ -n "$existing" ]; then
  [ "$existing" = "$data" ] || {{ echo "输入标识已对应不同内容，已保留原数据" >&2; exit 15; }}
  echo dup; exit 0
fi
cat "$T" >> in.jsonl || exit 14
echo ok"#,
            id = shell_quote(msg_id),
        );
        let out = self
            .transport
            .exec(sh(in_dir(&self.dir, &body)).stdin(format!("{msg_id}\t{json_line}\n")))
            .await?;
        match (out.code, out.stdout.trim()) {
            (0, "dup") => Ok(Appended::Duplicate),
            (0, "ok") => Ok(Appended::Written),
            (c, _) => Err(TransportError::Command {
                code: c,
                stderr: out.stderr.trim().to_owned(),
            }),
        }
    }

    pub async fn eof(&self) -> Result<Appended> {
        self.append("EOF", "").await
    }

    pub async fn probe(&self) -> Result<RunState> {
        let body = format!(
            r#"{IDENTITY_FNS}
echo "size=$(wc -c < out.jsonl 2>/dev/null | tr -d ' ')"
if [ -e exit.code ]; then echo "exited=$(cat exit.code)"
elif alive; then
  I=$(stat -c %i in.jsonl 2>/dev/null || stat -f %i in.jsonl 2>/dev/null)
  [ -n "$I" ] && [ "$I" = "$(cat in.inode 2>/dev/null)" ] && echo alive || echo alive-replaced
else echo lost; fi"#
        );
        let out = self.transport.exec(sh(in_dir(&self.dir, &body))).await?;
        if out.code == 94 {
            return Ok(RunState::Missing);
        }
        if out.code != 0 {
            return Err(TransportError::Command {
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
            });
        }
        Ok(parse_probe(&out.stdout))
    }

    pub async fn follow(&self, offset: u64) -> Result<LineStream> {
        let body = format!(
            r#"{IDENTITY_FNS}
tail -c +{start} -F out.jsonl 2>/dev/null & T=$!
while [ ! -e exit.code ] && alive; do sleep 1; done
sleep 1; kill $T 2>/dev/null; wait $T 2>/dev/null
exit 0"#,
            start = offset + 1,
        );
        self.transport
            .spawn_lines(sh(in_dir(&self.dir, &body)))
            .await
    }

    pub async fn drain(&self, offset: u64) -> Result<String> {
        let out = self
            .transport
            .exec(sh(in_dir(
                &self.dir,
                &format!("tail -c +{} out.jsonl", offset + 1),
            )))
            .await?;
        out.ok()
    }

    pub async fn line_at(&self, offset: u64) -> Result<Option<String>> {
        let out = self
            .transport
            .exec(sh(in_dir(
                &self.dir,
                &format!("tail -c +{} out.jsonl | head -n 1", offset + 1),
            )))
            .await?;
        let l = out.stdout.trim_end_matches('\n');
        Ok((!l.is_empty()).then(|| l.to_owned()))
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

    pub async fn remove(&self) -> Result<()> {
        let out = self
            .transport
            .exec(sh(format!(
                "[ -e {d}/exit.code ] || [ ! -e {d}/pid ] || {{ echo '还在运行，拒绝删除' >&2; exit 15; }}\nrm -rf {d}",
                d = shell_quote(&self.dir)
            )))
            .await?;
        if out.code != 0 {
            return Err(TransportError::Command {
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
            });
        }
        Ok(())
    }

    pub async fn stderr_tail(&self) -> Result<String> {
        let out = self
            .transport
            .exec(sh(in_dir(&self.dir, "tail -c 2000 err.txt 2>/dev/null")))
            .await?;
        Ok(out.stdout)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopStatus {
    Alive(i32),
    Waiting,
    Stopped,
}

fn stop_error(message: impl Into<String>) -> TransportError {
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

fn parse_probe(raw: &str) -> RunState {
    let size = raw
        .lines()
        .find_map(|l| l.strip_prefix("size="))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    for l in raw.lines() {
        let l = l.trim();
        if let Some(c) = l.strip_prefix("exited=") {
            return RunState::Exited {
                code: c.trim().parse().unwrap_or(-1),
                size,
            };
        }
        match l {
            "alive" => {
                return RunState::Alive {
                    size,
                    input_replaced: false,
                };
            }
            "alive-replaced" => {
                return RunState::Alive {
                    size,
                    input_replaced: true,
                };
            }
            "lost" => return RunState::Lost { size },
            _ => {}
        }
    }
    RunState::Lost { size }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalTransport;
    use futures::StreamExt;

    struct TestRun {
        run: DetachedRun,
        _root: tempfile::TempDir,
    }

    impl std::ops::Deref for TestRun {
        type Target = DetachedRun;

        fn deref(&self) -> &Self::Target {
            &self.run
        }
    }

    async fn launch(
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

    fn without_login(mut spec: ExecSpec) -> ExecSpec {
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

    #[tokio::test]
    async fn explicit_run_root_is_used_without_changing_home() {
        let root = tempfile::tempdir().unwrap();
        let home = std::env::var_os("HOME");
        let run = DetachedRun::launch_in(
            local(),
            root.path(),
            &rid("root"),
            &ExecSpec::new("/usr/bin/printf").arg("isolated"),
            None,
            RunMode::Null,
        )
        .await
        .unwrap();
        assert!(std::path::Path::new(&run.dir).starts_with(root.path()));
        assert_eq!(std::env::var_os("HOME"), home);
        assert!(matches!(
            wait_exit(&run).await,
            RunState::Exited { code: 0, .. }
        ));
        assert!(run.drain(0).await.unwrap().contains("isolated"));
        run.remove().await.unwrap();
    }

    fn local() -> Arc<dyn NodeTransport> {
        Arc::new(TestLocal)
    }

    #[cfg(unix)]
    fn input_fixture(transport: Arc<dyn NodeTransport>, initial: &[u8]) -> TestRun {
        use std::os::unix::fs::MetadataExt;
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("in.jsonl");
        std::fs::write(&input, initial).unwrap();
        std::fs::write(
            root.path().join("in.inode"),
            std::fs::metadata(input).unwrap().ino().to_string(),
        )
        .unwrap();
        let run = DetachedRun::attach(transport, root.path().display().to_string(), rid("input"));
        TestRun { run, _root: root }
    }

    #[cfg(unix)]
    struct TruncatedInput;

    #[cfg(unix)]
    #[async_trait::async_trait]
    impl NodeTransport for TruncatedInput {
        fn kind(&self) -> crate::TransportKind {
            crate::TransportKind::Local
        }
        fn target(&self) -> &str {
            "local"
        }
        async fn exec(&self, mut spec: ExecSpec) -> Result<crate::ExecOutput> {
            let input = spec.stdin.as_mut().unwrap();
            input.truncate(input.len() - 3);
            LocalTransport.exec(without_login(spec)).await
        }
        async fn spawn_lines(&self, _spec: ExecSpec) -> Result<LineStream> {
            unreachable!()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn append_truncated_upload_does_not_modify_input_log() {
        let run = input_fixture(Arc::new(TruncatedInput), b"");
        assert!(run.append("a-test", r#"{"answer":"A"}"#).await.is_err());
        assert_eq!(
            std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
            b""
        );
        let retry = DetachedRun::attach(local(), run.dir.clone(), run.run_id.clone());
        assert_eq!(
            retry.append("a-test", r#"{"answer":"A"}"#).await.unwrap(),
            Appended::Written
        );
        assert_eq!(
            retry.append("a-test", r#"{"answer":"A"}"#).await.unwrap(),
            Appended::Duplicate
        );
        assert_eq!(
            std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
            b"a-test\t{\"answer\":\"A\"}\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn append_completes_matching_partial_line_once() {
        let prior = b"u-1\t{}\n";
        let full = "a-test\t{\"answer\":\"选项甲\"}\n".as_bytes();
        for missing in [1, 3, 4] {
            let mut initial = prior.to_vec();
            initial.extend(&full[..full.len() - missing]);
            let run = input_fixture(local(), &initial);
            assert_eq!(
                run.append("a-test", r#"{"answer":"选项甲"}"#)
                    .await
                    .unwrap(),
                Appended::Written
            );
            assert_eq!(
                run.append("a-test", r#"{"answer":"选项甲"}"#)
                    .await
                    .unwrap(),
                Appended::Duplicate
            );
            let mut expected = prior.to_vec();
            expected.extend(full);
            assert_eq!(
                std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
                expected
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn append_refuses_inconsistent_partial_line_without_rewriting_it() {
        let initial = b"a-test\t{\"answer\":\"A";
        let run = input_fixture(local(), initial);
        assert!(run.append("a-test", r#"{"answer":"B"}"#).await.is_err());
        assert_eq!(
            std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
            initial
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn append_refuses_reused_id_with_a_different_complete_payload() {
        let initial = b"a-test\t{\"answer\":\"A\"}\n";
        let run = input_fixture(local(), initial);
        assert!(run.append("a-test", r#"{"answer":"B"}"#).await.is_err());
        assert_eq!(
            std::fs::read(run._root.path().join("in.jsonl")).unwrap(),
            initial
        );
    }

    fn echo_agent() -> ExecSpec {
        ExecSpec::new("bash").arg("-c").arg(
            r#"echo '{"type":"init"}'; while IFS= read -r l; do printf '{"echo":%s}\n' "$l"; done; echo '{"type":"done"}'"#,
        )
    }

    fn rid(tag: &str) -> String {
        format!(
            "t-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    async fn wait_exit(run: &DetachedRun) -> RunState {
        for _ in 0..60 {
            let st = run.probe().await.unwrap();
            if matches!(st, RunState::Exited { .. }) {
                return st;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        run.probe().await.unwrap()
    }

    #[tokio::test]
    async fn a_running_run_refuses_to_be_removed() {
        let spec = ExecSpec::new("bash").arg("-c").arg("sleep 30");
        let run = launch(local(), &rid("rm"), &spec, None, RunMode::Null)
            .await
            .unwrap();
        assert!(run.remove().await.is_err(), "还在跑的不能删");
        run.hard_kill().await.unwrap();
        run.remove().await.unwrap();
        assert!(!std::path::Path::new(&run.dir).exists());
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

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"hi"), "aGk=");
        assert_eq!(b64(b"hi!"), "aGkh");
        assert_eq!(b64("中文".as_bytes()), "5Lit5paH");
    }

    #[test]
    fn run_ids_that_could_escape_are_refused() {
        assert!(check_run_id("01a0b1f2-bc2a-7172").is_ok());
        for bad in ["", "../x", "a b", "a;rm -rf ~", "A", "a/b", "$HOME"] {
            assert!(check_run_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn cmd_sh_execs_the_agent_so_signals_reach_it() {
        let c = render_cmd(
            &ExecSpec::new("claude")
                .arg("-p")
                .arg("it's")
                .cwd("/w x")
                .env("K", "v 1"),
        )
        .unwrap();
        assert!(c.contains("cd '/w x' || exit 96"));
        assert!(c.contains("export K='v 1'"));
        assert!(
            c.contains("exec 'claude' '-p' 'it'\\''s'"),
            "要 exec 替换掉 bash，否则 TERM 只杀到 bash: {c}"
        );
        assert!(
            c.find(crate::PATH_PRELUDE)
                .is_some_and(|i| c.find("export K=").is_some_and(|j| i < j)),
            "按名字找的 CLI 先补 PATH，且在显式 env 之前: {c}"
        );
        let abs = render_cmd(&ExecSpec::new("/opt/claude/bin/claude")).unwrap();
        assert!(
            !abs.contains(crate::PATH_PRELUDE),
            "给了绝对路径就不用补 PATH"
        );
    }

    #[tokio::test]
    async fn invalid_environment_names_never_create_a_run_directory() {
        let root = tempfile::tempdir().unwrap();
        for spec in [
            ExecSpec::new("true").env("A-B", "test-value"),
            ExecSpec::new("true").env_file("A-B", root.path().join("absent")),
        ] {
            assert!(render_cmd(&spec).is_err());
            assert!(
                DetachedRun::launch_in(
                    local(),
                    root.path(),
                    "invalid-env",
                    &spec,
                    None,
                    RunMode::Null
                )
                .await
                .is_err()
            );
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn relay_delivers_input_and_eof_ends_the_agent() {
        let run = launch(
            local(),
            &rid("relay"),
            &echo_agent(),
            Some("u-1\t\"first\""),
            RunMode::Relay,
        )
        .await
        .unwrap();
        assert_eq!(
            run.append("u-2", "\"second\"").await.unwrap(),
            Appended::Written
        );
        assert_eq!(
            run.append("u-2", "\"second\"").await.unwrap(),
            Appended::Duplicate,
            "同一个 msg_id 重投不能写第二遍"
        );
        run.eof().await.unwrap();
        assert!(matches!(
            wait_exit(&run).await,
            RunState::Exited { code: 0, .. }
        ));

        let out = run.drain(0).await.unwrap();
        assert_eq!(out.matches("\"first\"").count(), 1, "{out}");
        assert_eq!(
            out.matches("\"second\"").count(),
            1,
            "重投的那条只能出现一次: {out}"
        );
        assert!(out.contains("blazar_exit"), "{out}");
    }

    #[tokio::test]
    async fn a_prompt_line_equal_to_a_heredoc_delimiter_is_just_text() {
        let marker = std::env::temp_dir().join(format!("pwned-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let evil = format!("fix bug\nBLZ_CMD_EOF\ntouch {}\nEOF\n'", marker.display());
        let spec = ExecSpec::new("printf").arg("%s").arg(&evil);
        let run = launch(local(), &rid("inject"), &spec, None, RunMode::Null)
            .await
            .unwrap();
        wait_exit(&run).await;
        assert!(!marker.exists(), "提示词里的文本被当成命令执行了");
        assert!(
            run.drain(0).await.unwrap().contains("BLZ_CMD_EOF"),
            "文本要原样到达 agent"
        );
    }

    #[tokio::test]
    async fn secrets_from_env_files_reach_the_agent_but_not_the_run_dir() {
        let secret = format!("sk-ant-oat01-test-{}", std::process::id());
        let file = std::env::temp_dir().join(format!("blz-secret-{}", std::process::id()));
        std::fs::write(&file, format!("{secret}\n")).unwrap();
        let spec = ExecSpec::new("bash")
            .arg("-c")
            .arg(r#"printf '[%s]' "$TOKEN""#)
            .env_file("TOKEN", &file);
        let run = launch(local(), &rid("secret"), &spec, None, RunMode::Null)
            .await
            .unwrap();
        wait_exit(&run).await;
        let out = run.drain(0).await.unwrap();
        assert!(out.contains(&format!("[{secret}]")), "{out}");
        let cmd = std::fs::read_to_string(std::path::Path::new(&run.dir).join("cmd.sh")).unwrap();
        assert!(cmd.contains("$(cat "), "{cmd}");
        for f in std::fs::read_dir(&run.dir).unwrap() {
            let path = f.unwrap().path();
            if path.ends_with("out.jsonl") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                assert!(
                    !text.contains(&secret),
                    "{} 里出现了明文 token",
                    path.display()
                );
            }
        }
        run.remove().await.unwrap();
        let _ = std::fs::remove_file(&file);
    }

    #[tokio::test]
    async fn the_agent_outlives_whoever_launched_it() {
        let spec = ExecSpec::new("bash").arg("-c").arg("sleep 30");
        let run = launch(local(), &rid("outlive"), &spec, None, RunMode::Null)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(matches!(run.probe().await.unwrap(), RunState::Alive { .. }));
        run.hard_kill().await.unwrap();
        assert!(matches!(
            run.probe().await.unwrap(),
            RunState::Exited { .. }
        ));
    }

    #[tokio::test]
    async fn following_resumes_exactly_from_the_byte_offset() {
        let run = launch(
            local(),
            &rid("follow"),
            &echo_agent(),
            Some("u-1\t\"a\""),
            RunMode::Relay,
        )
        .await
        .unwrap();
        run.append("u-2", "\"b\"").await.unwrap();
        run.eof().await.unwrap();
        wait_exit(&run).await;

        let mut s = run.follow(0).await.unwrap();
        let mut off = 0u64;
        let mut first = Vec::new();
        for _ in 0..2 {
            let l = s.stdout.next().await.unwrap();
            off += l.len() as u64 + 1;
            first.push(l);
        }
        drop(s);

        let rest = run.drain(off).await.unwrap();
        let all = format!("{}\n{rest}", first.join("\n"));
        assert_eq!(all, run.drain(0).await.unwrap(), "续读要逐字节一致");
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

    #[tokio::test]
    async fn a_replaced_input_file_is_detected_instead_of_silently_dropping_input() {
        let run = launch(local(), &rid("inode"), &echo_agent(), None, RunMode::Relay)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let p = std::path::Path::new(&run.dir).join("in.jsonl");
        std::fs::remove_file(&p).unwrap();
        std::fs::write(&p, "").unwrap();
        assert!(
            matches!(
                run.probe().await.unwrap(),
                RunState::Alive {
                    input_replaced: true,
                    ..
                }
            ),
            "要探测得出输入文件被换过"
        );
        assert!(
            run.append("u-9", "\"x\"").await.is_err(),
            "写进新文件等于没送达，必须报错"
        );
        run.hard_kill().await.unwrap();
    }

    #[test]
    fn probe_output_is_parsed() {
        assert_eq!(
            parse_probe("size=120\nexited=0\n"),
            RunState::Exited { code: 0, size: 120 }
        );
        assert_eq!(
            parse_probe("size=5\nalive\n"),
            RunState::Alive {
                size: 5,
                input_replaced: false
            }
        );
        assert_eq!(parse_probe("size=5\nlost\n"), RunState::Lost { size: 5 });
    }

    fn remote() -> Option<Arc<dyn NodeTransport>> {
        let h = std::env::var("BLAZAR_TEST_SSH_HOST").ok()?;
        Some(Arc::new(crate::SshTransport::new(h)))
    }

    #[tokio::test]
    #[ignore = "requires an explicitly authorized remote test machine"]
    async fn remote_dedup_holds_on_gnu_grep_hosts() {
        let Some(t) = remote() else { return };
        let run = DetachedRun::launch(t, &rid("rdedup"), &echo_agent(), None, RunMode::Relay)
            .await
            .unwrap();
        for _ in 0..3 {
            run.append("u-7", "\"once\"").await.unwrap();
        }
        run.eof().await.unwrap();
        wait_exit(&run).await;
        let out = run.drain(0).await.unwrap();
        assert_eq!(
            out.matches("\"once\"").count(),
            1,
            "Linux 上重投三次也只能送达一次: {out}"
        );
    }

    #[tokio::test]
    #[ignore = "requires an explicitly authorized remote test machine"]
    async fn remote_orphaned_tool_processes_are_reaped_after_an_agent_crash() {
        let Some(t) = remote() else { return };
        let tag = rid("orphan");
        let agent = ExecSpec::new("bash").arg("-c").arg(format!(
            "setsid bash -c 'sleep 300; echo {tag}' & sleep 1; kill -KILL $$"
        ));
        let run = DetachedRun::launch(t.clone(), &tag, &agent, None, RunMode::Null)
            .await
            .unwrap();
        let st = wait_exit(&run).await;
        assert!(matches!(st, RunState::Exited { code: 137, .. }), "{st:?}");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let left = t
            .exec(ExecSpec::new("bash").arg("-c").arg(format!(
                "grep -lz '^BLAZAR_RUN={tag}$' /proc/[0-9]*/environ 2>/dev/null | wc -l"
            )))
            .await
            .unwrap();
        assert_eq!(
            left.stdout.trim(),
            "0",
            "agent 崩溃后被收养的工具进程要被清掉"
        );
    }

    #[tokio::test]
    #[ignore = "requires an explicitly authorized remote test machine"]
    async fn remote_survives_launcher_and_resumes_from_offset() {
        let Some(t) = remote() else { return };
        let run = DetachedRun::launch(
            t,
            &rid("rfollow"),
            &echo_agent(),
            Some("u-1\t\"a\""),
            RunMode::Relay,
        )
        .await
        .unwrap();
        assert!(matches!(run.probe().await.unwrap(), RunState::Alive { .. }));
        run.append("u-2", "\"b\"").await.unwrap();
        run.eof().await.unwrap();
        wait_exit(&run).await;
        let mut s = run.follow(0).await.unwrap();
        let first = s.stdout.next().await.unwrap();
        drop(s);
        let rest = run.drain(first.len() as u64 + 1).await.unwrap();
        assert_eq!(format!("{first}\n{rest}"), run.drain(0).await.unwrap());
    }
}
