use std::sync::Arc;

use crate::{ExecSpec, LineStream, NodeTransport, Result, TransportError, shell_quote};

mod probe;
mod stop;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

use probe::parse_probe;
pub use probe::{ProbeItem, Probed, probe_many};
use stop::stop_error;

pub const RUNS_ROOT: &str = "$HOME/.blazar/runs";

const IDENTITY_FNS: &str = r#"boot_of() { cat /proc/sys/kernel/random/boot_id 2>/dev/null || sysctl -n kern.boottime 2>/dev/null | sed 's/^{ sec = \([0-9]*\),.*/\1/'; }
start_of() { awk '{print $22}' "/proc/$1/stat" 2>/dev/null || ps -o lstart= -p "$1" 2>/dev/null | tr -s ' ' '_'; }
alive() {
  read -r P B S < pid 2>/dev/null || return 1
  kill -0 "$P" 2>/dev/null || return 1
  case $S in *_*) ;; *) [ "$(boot_of)" = "$B" ] || return 1 ;; esac
  [ "$(start_of "$P")" = "$S" ] || return 1
}
"#;

const PROBE_BODY: &str = r#"echo "size=$(wc -c < out.jsonl 2>/dev/null | tr -d ' ')"
if [ -e exit.code ]; then echo "exited=$(cat exit.code)"
elif alive; then
  I=$(stat -c %i in.jsonl 2>/dev/null || stat -f %i in.jsonl 2>/dev/null)
  [ -n "$I" ] && [ "$I" = "$(cat in.inode 2>/dev/null)" ] && echo alive || echo alive-replaced
else echo lost; fi"#;

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
    for k in &spec.env_unset {
        s.push_str(&format!("unset {k}\n"));
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

    pub async fn adopt(
        transport: Arc<dyn NodeTransport>,
        root: &std::path::Path,
        run_id: &str,
        pid: u32,
    ) -> Result<Self> {
        check_run_id(run_id)?;
        if !root.is_absolute() {
            return Err(TransportError::Command {
                code: -1,
                stderr: "运行目录必须是绝对路径".into(),
            });
        }
        if pid <= 1 {
            return Err(stop_error("进程号无效"));
        }
        let script = format!(
            r#"{IDENTITY_FNS}
umask 077
P={pid}
kill -0 "$P" 2>/dev/null || ps -p "$P" >/dev/null 2>&1 || {{ echo "这台机器上没有进程 $P" >&2; exit 16; }}
D={root}/{run_id}
mkdir -p "$D" && cd "$D" || exit 17
printf '%s %s %s\n' "$P" "$(boot_of)" "$(start_of "$P")" > pid.tmp && mv pid.tmp pid || exit 17
echo "__BLAZAR_LAUNCHED__ $D"
"#,
            root = shell_quote(&root.to_string_lossy()),
        );
        let out = transport.exec(sh(script)).await?;
        let dir = out
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("__BLAZAR_LAUNCHED__ "))
            .map(str::trim)
            .filter(|d| d.starts_with('/'))
            .ok_or_else(|| TransportError::Command {
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
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
        let body = format!("{IDENTITY_FNS}\n{PROBE_BODY}");
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
