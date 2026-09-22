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

const RUN_SH: &str = r#"#!/bin/bash
# blazar-run v2 —— bash run.sh <rundir> <relay|null> <run-id>
D=$1; MODE=$2; RID=$3
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
# 按环境标记收掉被 init 收养的孤儿（只在有 /proc 的系统上有效）
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

fn render_cmd(spec: &ExecSpec) -> String {
    let mut s = String::from("#!/bin/bash\n");
    if let Some(cwd) = &spec.cwd {
        s.push_str(&format!(
            "cd {} || exit 96\n",
            shell_quote(&cwd.display().to_string())
        ));
    }
    for (k, v) in &spec.env {
        s.push_str(&format!("export {k}={}\n", shell_quote(v)));
    }
    s.push_str("exec ");
    s.push_str(&shell_quote(&spec.program));
    for a in &spec.args {
        s.push(' ');
        s.push_str(&shell_quote(a));
    }
    s.push('\n');
    s
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
        check_run_id(run_id)?;
        let run_sh = RUN_SH.replace("__IDENTITY__", IDENTITY_FNS);
        let payload = format!(
            "{}\n{}\n{}\n",
            b64(run_sh.as_bytes()),
            b64(render_cmd(cmd).as_bytes()),
            b64(first_input
                .filter(|_| mode == RunMode::Relay)
                .map(|l| format!("{l}\n"))
                .unwrap_or_default()
                .as_bytes()),
        );
        let script = format!(
            r#"set -e; umask 077
D="{root}/{rid}"
mkdir -p "$D"; cd "$D"
IFS= read -r A; IFS= read -r B; IFS= read -r C
printf '%s' "$A" | base64 --decode > run.sh
printf '%s' "$B" | base64 --decode > cmd.sh
printf '%s' "$C" | base64 --decode > in.jsonl
# inode 在这里同步记下，不留给 run.sh：launch 一返回调用方就可能追加输入，
# 那时 run.sh 可能还没跑到那一行，会被误判成"输入文件被换过"
(stat -c %i in.jsonl 2>/dev/null || stat -f %i in.jsonl) > in.inode
set +e
set -m
bash "$D/run.sh" "$D" {mode} {rid} </dev/null >/dev/null 2>&1 &
# 等 run.sh 写出 pid 再返回：否则紧接着的探活会因为没有 pid 文件而判成 lost
i=0; while [ ! -s pid ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
[ -s pid ] || {{ echo "run.sh 5 秒内没有起来" >&2; exit 98; }}
echo "__BLAZAR_LAUNCHED__ $D"
"#,
            root = RUNS_ROOT,
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
            r#"I=$(stat -c %i in.jsonl 2>/dev/null || stat -f %i in.jsonl 2>/dev/null)
# 文件被删了重建的话，中继还攥着旧文件的 fd，写进新文件的输入永远到不了 agent
[ -n "$I" ] && [ "$I" = "$(cat in.inode 2>/dev/null)" ] || {{ echo "in.jsonl 已被替换，输入无法送达" >&2; exit 12; }}
[ -e exit.code ] && {{ echo "agent 已经结束" >&2; exit 13; }}
awk -F'\t' -v id={id} '$1==id{{f=1;exit}} END{{exit !f}}' in.jsonl && {{ echo dup; exit 0; }}
cat >> in.jsonl
echo ok"#,
            id = shell_quote(msg_id),
        );
        let out = self
            .transport
            .exec(sh(in_dir(&self.dir, &body)).stdin(format!("{msg_id}\t{json_line}\n")))
            .await?;
        match (out.code, out.stdout.trim()) {
            (0, "dup") => Ok(Appended::Duplicate),
            (0, _) => Ok(Appended::Written),
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
# 给 tail 一点时间把最后几行吐完
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
        Ok(out.stdout)
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
        let body = format!(
            r#"{IDENTITY_FNS}
[ -e exit.code ] && exit 0
alive || exit 0
read -r P B S < pid
kill -TERM "$P" 2>/dev/null
i=0; while [ $i -lt 20 ] && [ ! -e exit.code ]; do sleep 0.5; i=$((i+1)); done
[ -e exit.code ] && exit 0
# TERM 没用：整组 KILL。run.sh 自己也在这组里，它写不出退出码了，这里代写
kill -KILL -"$P" 2>/dev/null
L=$(grep -lz "^BLAZAR_RUN={rid}\$" /proc/[0-9]*/environ 2>/dev/null | sed 's#/proc/\([0-9]*\)/environ#\1#' | tr '\n' ' ')
[ -n "$L" ] && kill -KILL $L 2>/dev/null
printf '\n{{"type":"blazar_exit","code":137}}\n' >> out.jsonl
echo 137 > exit.code.tmp && mv exit.code.tmp exit.code"#,
            rid = self.run_id,
        );
        let out = self.transport.exec(sh(in_dir(&self.dir, &body))).await?;
        if out.code != 0 && out.code != 94 {
            return Err(TransportError::Command {
                code: out.code,
                stderr: out.stderr.trim().to_owned(),
            });
        }
        Ok(())
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

    fn local() -> Arc<dyn NodeTransport> {
        Arc::new(LocalTransport)
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
        let run = DetachedRun::launch(local(), &rid("rm"), &spec, None, RunMode::Null)
            .await
            .unwrap();
        assert!(run.remove().await.is_err(), "还在跑的不能删");
        run.hard_kill().await.unwrap();
        run.remove().await.unwrap();
        assert!(!std::path::Path::new(&run.dir).exists());
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
        );
        assert!(c.contains("cd '/w x' || exit 96"));
        assert!(c.contains("export K='v 1'"));
        assert!(
            c.contains("exec 'claude' '-p' 'it'\\''s'"),
            "要 exec 替换掉 bash，否则 TERM 只杀到 bash: {c}"
        );
    }

    #[tokio::test]
    async fn relay_delivers_input_and_eof_ends_the_agent() {
        let run = DetachedRun::launch(
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
        let run = DetachedRun::launch(local(), &rid("inject"), &spec, None, RunMode::Null)
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
    async fn the_agent_outlives_whoever_launched_it() {
        let spec = ExecSpec::new("bash").arg("-c").arg("sleep 30");
        let run = DetachedRun::launch(local(), &rid("outlive"), &spec, None, RunMode::Null)
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
        let run = DetachedRun::launch(
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

    #[tokio::test]
    async fn a_dead_pid_that_got_reused_is_not_mistaken_for_the_agent() {
        let spec = ExecSpec::new("bash").arg("-c").arg("sleep 30");
        let run = DetachedRun::launch(local(), &rid("reuse"), &spec, None, RunMode::Null)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let mut bystander = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pidfile = std::path::Path::new(&run.dir).join("pid");
        let orig = std::fs::read_to_string(&pidfile).unwrap();
        let mut parts: Vec<&str> = orig.split_whitespace().collect();
        let b = bystander.id().to_string();
        parts[0] = &b;
        parts[2] = "not-the-same-start";
        std::fs::write(&pidfile, parts.join(" ")).unwrap();

        assert!(
            matches!(run.probe().await.unwrap(), RunState::Lost { .. }),
            "身份对不上就必须判 Lost，而不是 Alive"
        );
        run.hard_kill().await.unwrap();
        let still = std::process::Command::new("kill")
            .args(["-0", &b])
            .status()
            .unwrap()
            .success();
        assert!(still, "身份对不上时绝不能 kill —— 那是别人的进程");
        std::process::Command::new("kill").arg(&b).status().ok();
        let _ = bystander.wait();

        std::fs::write(&pidfile, orig).unwrap();
        run.hard_kill().await.unwrap();
    }

    #[tokio::test]
    async fn a_replaced_input_file_is_detected_instead_of_silently_dropping_input() {
        let run = DetachedRun::launch(local(), &rid("inode"), &echo_agent(), None, RunMode::Relay)
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
