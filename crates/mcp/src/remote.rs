use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use blazar_transport::{ExecSpec, LocalTransport, NodeTransport, SshTransport};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

const PROTOCOL_VERSION: &str = "2025-06-18";

const MAX_OUTPUT: usize = 30_000;
const MAX_EDIT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RemoteTarget {
    pub node: String,

    pub root: String,
}

pub struct Remote {
    target: RemoteTarget,
    transport: Arc<dyn NodeTransport>,
}

impl Remote {
    #[must_use]
    pub fn new(target: RemoteTarget) -> Self {
        let transport: Arc<dyn NodeTransport> = if target.node == "local" {
            Arc::new(LocalTransport)
        } else {
            let dir = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".blazar")
                .join("ssh-cm");
            Arc::new(SshTransport::new(&target.node).multiplexed(dir))
        };
        Self { target, transport }
    }

    async fn run(&self, script: String, timeout: Duration) -> Result<(i32, String)> {
        let spec = ExecSpec::new("bash").arg("-s").stdin(script.into_bytes());
        let out =
            tokio::time::timeout(timeout + Duration::from_secs(15), self.transport.exec(spec))
                .await
                .with_context(|| format!("{} 上的命令超时", self.target.node))??;
        let mut text = out.stdout;
        if !out.stderr.trim().is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&out.stderr);
        }
        Ok((out.code, text))
    }

    fn cd_root(&self) -> String {
        format!(
            "cd {} 2>/dev/null || {{ echo '工作区目录不存在：{}' >&2; exit 97; }}\n",
            q(&self.target.root),
            self.target.root.replace('\'', "")
        )
    }

    fn path(&self, p: &str) -> String {
        if let Some(rest) = p.strip_prefix("~/") {
            format!("\"$HOME\"/{}", q(rest))
        } else if p.starts_with('/') {
            q(p)
        } else {
            q(&format!(
                "{}/{}",
                self.target.root.trim_end_matches('/'),
                p.trim_start_matches("./")
            ))
        }
    }

    pub async fn call(&self, name: &str, args: &Value) -> Result<(bool, String)> {
        match name {
            "Bash" => self.bash(args).await,
            "Read" => self.read(args).await,
            "Write" => self.write(args).await,
            "Edit" => self.edit(args).await,
            "Glob" => self.glob(args).await,
            "Grep" => self.grep(args).await,
            other => anyhow::bail!("未知工具：{other}"),
        }
    }

    async fn bash(&self, a: &Value) -> Result<(bool, String)> {
        let cmd = req_str(a, "command")?;
        let secs = a
            .get("timeout")
            .and_then(Value::as_u64)
            .map_or(120, |ms| (ms / 1000).clamp(1, 600));

        let script = format!(
            "{cd}CMD=$(base64 -d <<'__BLAZAR_B64__'\n{b64}\n__BLAZAR_B64__\n)\n\
             if command -v timeout >/dev/null 2>&1; then\n  \
               timeout -k 5 {secs} bash -c \"$CMD\" </dev/null 2>&1; rc=$?\n\
             else\n  bash -c \"$CMD\" </dev/null 2>&1; rc=$?\nfi\n\
             [ $rc -eq 124 ] && echo \"（命令超过 {secs} 秒被终止）\"\nexit $rc\n",
            cd = self.cd_root(),
            b64 = b64_wrapped(cmd.as_bytes()),
        );
        let (code, out) = self.run(script, Duration::from_secs(secs)).await?;
        let mut text = clip(&out);
        if code != 0 {
            text.push_str(&format!("\n[退出码 {code}]"));
        }
        if text.trim().is_empty() {
            text = "（无输出）".into();
        }
        Ok((code == 0, text))
    }

    async fn read(&self, a: &Value) -> Result<(bool, String)> {
        let path = req_str(a, "file_path")?;
        let offset = a.get("offset").and_then(Value::as_u64).unwrap_or(1).max(1);
        let limit = a
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(2000)
            .clamp(1, 5000);
        let script = format!(
            "f={p}\n\
             [ -e \"$f\" ] || {{ echo \"文件不存在：$f\"; exit 3; }}\n\
             [ -d \"$f\" ] && {{ echo \"这是目录，不是文件：$f\"; ls -la \"$f\" | head -200; exit 4; }}\n\
             if [ -s \"$f\" ] && ! grep -Iq . \"$f\"; then echo \"二进制文件（$(wc -c < \"$f\") 字节），不显示内容\"; exit 5; fi\n\
             awk -v s={offset} -v n={limit} 'NR>=s && NR<s+n {{ l=$0; if (length(l)>2000) l=substr(l,1,2000) \"…\"; printf \"%6d\\t%s\\n\", NR, l }} NR>=s+n {{ exit }}' \"$f\"\n\
             echo \"__BLAZAR_TOTAL__ $(wc -l < \"$f\")\"\n",
            p = self.path(path),
        );
        let (code, out) = self.run(script, Duration::from_secs(60)).await?;
        if code != 0 {
            return Ok((false, clip(&out)));
        }
        let (body, total) = match out.rsplit_once("__BLAZAR_TOTAL__") {
            Some((b, t)) => (b.to_owned(), t.trim().parse::<u64>().unwrap_or(0)),
            None => (out.clone(), 0),
        };
        let mut text = clip(&body);
        let shown_end = offset + limit - 1;
        if total > shown_end {
            text.push_str(&format!(
                "\n（文件共 {total} 行，这里是第 {offset}–{shown_end} 行；用 offset 继续读）"
            ));
        }
        if body.trim().is_empty() {
            text = if total == 0 {
                "（空文件）".into()
            } else {
                format!("（offset 超出范围：文件共 {total} 行）")
            };
        }
        Ok((true, text))
    }

    async fn write_bytes(&self, path: &str, data: &[u8]) -> Result<(i32, String)> {
        let script = format!(
            "f={p}\nmkdir -p \"$(dirname \"$f\")\" && base64 -d > \"$f\" <<'__BLAZAR_B64__'\n{b64}\n__BLAZAR_B64__\n",
            p = self.path(path),
            b64 = b64_wrapped(data),
        );
        self.run(script, Duration::from_secs(120)).await
    }

    async fn write(&self, a: &Value) -> Result<(bool, String)> {
        let path = req_str(a, "file_path")?;
        let content = a.get("content").and_then(Value::as_str).unwrap_or_default();
        let (code, out) = self.write_bytes(path, content.as_bytes()).await?;
        if code != 0 {
            return Ok((false, format!("写入失败：{}", clip(&out))));
        }
        let lines = content.lines().count();
        Ok((true, format!("已写入 {path}（{lines} 行）")))
    }

    async fn edit(&self, a: &Value) -> Result<(bool, String)> {
        let path = req_str(a, "file_path")?;
        let old = a
            .get("old_string")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let new = a
            .get("new_string")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let all = a
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if old.is_empty() {
            return Ok((false, "old_string 不能为空；新建文件请用 Write".into()));
        }
        if old == new {
            return Ok((false, "old_string 与 new_string 相同，没有要改的".into()));
        }
        let script = format!(
            "f={p}\n[ -f \"$f\" ] || {{ echo \"文件不存在：$f\"; exit 3; }}\n\
             [ \"$(wc -c < \"$f\")\" -le {MAX_EDIT_BYTES} ] || {{ echo '文件过大，不适合整文件编辑'; exit 6; }}\n\
             base64 < \"$f\"\n",
            p = self.path(path),
        );
        let (code, out) = self.run(script, Duration::from_secs(60)).await?;
        if code != 0 {
            return Ok((false, clip(&out)));
        }
        let raw = B64
            .decode(out.split_whitespace().collect::<String>())
            .context("读取原文件失败")?;
        let Ok(text) = String::from_utf8(raw) else {
            return Ok((false, "文件不是 UTF-8 文本，不能用 Edit 修改".into()));
        };
        let n = text.matches(old).count();
        if n == 0 {
            return Ok((
                false,
                "old_string 在文件里没找到。先用 Read 看一下当前内容（注意缩进和空白要完全一致）"
                    .into(),
            ));
        }
        if n > 1 && !all {
            return Ok((
                false,
                format!(
                    "old_string 在文件里出现了 {n} 次。多给一些上下文让它唯一，或设置 replace_all"
                ),
            ));
        }
        let updated = if all {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        let (code, out) = self.write_bytes(path, updated.as_bytes()).await?;
        if code != 0 {
            return Ok((false, format!("写回失败：{}", clip(&out))));
        }
        Ok((
            true,
            format!("已修改 {path}（替换 {} 处）", if all { n } else { 1 }),
        ))
    }

    async fn glob(&self, a: &Value) -> Result<(bool, String)> {
        let pattern = req_str(a, "pattern")?.trim_start_matches("./");

        let Some(re) = glob_regex(pattern) else {
            return Ok((false, "pattern 无法解析".into()));
        };

        let literal: Vec<&str> = pattern
            .split('/')
            .take_while(|seg| !seg.contains(['*', '?', '[', '{']))
            .collect();
        let prefix = if literal.len() == pattern.split('/').count() {
            String::new()
        } else {
            literal.join("/")
        };
        let base = a.get("path").and_then(Value::as_str).unwrap_or(".");
        let start = if prefix.is_empty() {
            base.to_owned()
        } else {
            format!("{}/{prefix}", base.trim_end_matches('/'))
        };
        let script = format!(
            "{cd}cd {start} 2>/dev/null || {{ echo '目录不存在'; exit 3; }}\n\
             find . -name .git -prune -o -name node_modules -prune -o -type f -print 2>/dev/null | head -200000\n",
            cd = self.cd_root(),
            start = self.path(&start),
        );
        let (code, out) = self.run(script, Duration::from_secs(90)).await?;
        if code != 0 {
            return Ok((false, clip(&out)));
        }
        let mut files: Vec<String> = out
            .lines()
            .map(|l| l.trim_start_matches("./"))
            .map(|l| {
                if prefix.is_empty() {
                    l.to_owned()
                } else {
                    format!("{prefix}/{l}")
                }
            })
            .filter(|l| re.is_match(l))
            .take(1000)
            .collect();
        files.sort();
        Ok((
            true,
            if files.is_empty() {
                "没有匹配的文件".into()
            } else {
                clip(&files.join("\n"))
            },
        ))
    }

    async fn grep(&self, a: &Value) -> Result<(bool, String)> {
        let pattern = req_str(a, "pattern")?;
        let target = a
            .get("path")
            .and_then(Value::as_str)
            .map_or_else(|| ".".to_owned(), |p| self.path(p));
        let mode = a
            .get("output_mode")
            .and_then(Value::as_str)
            .unwrap_or("files_with_matches");
        let ci = a.get("-i").and_then(Value::as_bool).unwrap_or(false)
            || a.get("case_insensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let ctx = a.get("-C").and_then(Value::as_u64).unwrap_or(0).min(20);
        let glob = a.get("glob").and_then(Value::as_str);

        let mut rg = String::from("rg --no-heading --color never --hidden -g '!.git'");
        let mut gr = String::from("grep -rIE --exclude-dir=.git");
        match mode {
            "content" => {
                rg.push_str(" -n");
                gr.push_str(" -n");
                if ctx > 0 {
                    rg.push_str(&format!(" -C {ctx}"));
                    gr.push_str(&format!(" -C {ctx}"));
                }
            }
            "count" => {
                rg.push_str(" -c");
                gr.push_str(" -c");
            }
            _ => {
                rg.push_str(" -l");
                gr.push_str(" -l");
            }
        }
        if ci {
            rg.push_str(" -i");
            gr.push_str(" -i");
        }
        if let Some(g) = glob {
            rg.push_str(&format!(" -g {}", q(g)));
            gr.push_str(&format!(" --include={}", q(g)));
        }
        let script = format!(
            "{cd}if command -v rg >/dev/null 2>&1; then {rg} -e {pat} -- {target}; \
             else {gr} -e {pat} -- {target}; fi | head -c 200000\n",
            cd = self.cd_root(),
            pat = q(pattern),
        );
        let (_, out) = self.run(script, Duration::from_secs(90)).await?;
        Ok((
            true,
            if out.trim().is_empty() {
                "没有匹配".into()
            } else {
                clip(&out)
            },
        ))
    }
}

fn glob_regex(pattern: &str) -> Option<regex::Regex> {
    let mut re = String::from("^");
    let mut chars = pattern.chars().peekable();
    let mut in_brace = false;
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();

                if chars.peek() == Some(&'/') {
                    chars.next();
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '[' => {
                re.push('[');
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    if c == '\\' {
                        re.push_str("\\\\");
                    } else {
                        re.push(c);
                    }
                }
                re.push(']');
            }
            '{' => {
                in_brace = true;
                re.push_str("(?:");
            }
            '}' if in_brace => {
                in_brace = false;
                re.push(')');
            }
            ',' if in_brace => re.push('|'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re).ok()
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn req_str<'a>(a: &'a Value, k: &str) -> Result<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("缺少参数 {k}"))
}

fn b64_wrapped(data: &[u8]) -> String {
    let s = B64.encode(data);
    s.as_bytes()
        .chunks(76)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

fn clip(s: &str) -> String {
    if s.len() <= MAX_OUTPUT {
        return s.to_owned();
    }
    let half = MAX_OUTPUT / 2;
    let mut head_end = half;
    while !s.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = s.len() - half;
    while !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n…（中间省略 {} 字节）…\n{}",
        &s[..head_end],
        tail_start - head_end,
        &s[tail_start..]
    )
}

#[must_use]
pub fn target_from_args(args: &[String]) -> Option<RemoteTarget> {
    let get = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    Some(RemoteTarget {
        node: get("--node")?,
        root: get("--root")?,
    })
}

pub const SUBCOMMAND: &str = "__mcp-remote";

#[must_use]
pub fn manifest(t: &RemoteTarget) -> Value {
    let loc = format!("{}:{}", t.node, t.root);
    let p = |d: &str| json!({ "type": "string", "description": d });
    let obj = |props: Value, req: &[&str]| json!({ "type": "object", "properties": props, "required": req });
    json!({ "tools": [
        { "name": "Bash",
          "description": format!("在工作区所在机器 {loc} 上执行 bash 命令（工作目录为工作区根目录）。用于构建、测试、git、跑脚本等。命令不能交互（stdin 为空）。"),
          "inputSchema": obj(json!({
              "command": p("要执行的命令"),
              "description": p("5–10 个字说明这条命令做什么"),
              "timeout": { "type": "number", "description": "超时毫秒数，默认 120000，最大 600000" } }), &["command"]) },
        { "name": "Read", "annotations": { "readOnlyHint": true },
          "description": format!("读取 {loc} 上的文件，带行号输出。相对路径相对工作区根目录。"),
          "inputSchema": obj(json!({
              "file_path": p("文件路径"),
              "offset": { "type": "number", "description": "从第几行开始（1 起）" },
              "limit": { "type": "number", "description": "最多读多少行，默认 2000" } }), &["file_path"]) },
        { "name": "Write",
          "description": format!("在 {loc} 上写入（覆盖）文件，自动创建上级目录。修改已有文件优先用 Edit。"),
          "inputSchema": obj(json!({
              "file_path": p("文件路径"), "content": p("完整文件内容") }), &["file_path", "content"]) },
        { "name": "Edit",
          "description": format!("精确替换 {loc} 上某个文件里的一段文本。old_string 必须与文件内容逐字一致（含缩进），且在文件中唯一，除非 replace_all。"),
          "inputSchema": obj(json!({
              "file_path": p("文件路径"),
              "old_string": p("要被替换的原文"),
              "new_string": p("替换成的内容"),
              "replace_all": { "type": "boolean", "description": "替换所有出现处" } }), &["file_path", "old_string", "new_string"]) },
        { "name": "Glob", "annotations": { "readOnlyHint": true },
          "description": format!("在 {loc} 上按通配模式找文件（支持 **），如 \"src/**/*.rs\"。"),
          "inputSchema": obj(json!({ "pattern": p("通配模式"), "path": p("起始目录，默认工作区根目录") }), &["pattern"]) },
        { "name": "Grep", "annotations": { "readOnlyHint": true },
          "description": format!("在 {loc} 上按正则搜索文件内容（有 ripgrep 用 ripgrep）。"),
          "inputSchema": obj(json!({
              "pattern": p("正则表达式"),
              "path": p("搜索的文件或目录，默认工作区根目录"),
              "glob": p("只搜匹配这个通配的文件，如 \"*.rs\""),
              "output_mode": { "type": "string", "enum": ["content", "files_with_matches", "count"],
                               "description": "content 显示匹配行；files_with_matches（默认）只列文件；count 计数" },
              "-i": { "type": "boolean", "description": "忽略大小写" },
              "-C": { "type": "number", "description": "content 模式下的上下文行数" } }), &["pattern"]) },
    ]})
}

pub async fn handle(remote: &Remote, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned()?;
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result: Result<Value> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "blazar-remote", "version": env!("CARGO_PKG_VERSION") },
            "instructions": format!(
                "你的所有文件与命令操作都发生在远端机器 {}:{} 上。本机工作目录只是占位，不要在本机找代码。",
                remote.target.node, remote.target.root),
        })),
        "tools/list" => Ok(manifest(&remote.target)),
        "tools/call" => {
            let p = req.get("params").cloned().unwrap_or(json!({}));
            let name = p
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let args = p.get("arguments").cloned().unwrap_or(json!({}));

            Ok(match remote.call(&name, &args).await {
                Ok((ok, text)) => {
                    json!({ "content": [{ "type": "text", "text": text }], "isError": !ok })
                }
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true })
                }
            })
        }
        "ping" => Ok(json!({})),
        other => Err(anyhow::anyhow!("不支持的方法：{other}")),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": e.to_string() } })
        }
    })
}

pub async fn serve(target: RemoteTarget) -> Result<()> {
    let remote = Arc::new(Remote::new(target));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(v) = rx.recv().await {
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{v}").and_then(|()| out.flush()).is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (remote, tx) = (remote.clone(), tx.clone());
        tokio::spawn(async move {
            if let Some(resp) = handle(&remote, &req).await {
                let _ = tx.send(resp);
            }
        });
    }
    drop(tx);
    let _ = writer.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(root: &std::path::Path) -> Remote {
        Remote::new(RemoteTarget {
            node: "local".into(),
            root: root.display().to_string(),
        })
    }

    #[tokio::test]
    async fn bash_runs_in_workspace_root_and_keeps_quotes() {
        let d = tempfile::tempdir().unwrap();
        let r = local(d.path());
        let (ok, out) = r
            .call(
                "Bash",
                &json!({ "command": "pwd; echo \"it's $((1+1))\"; cat <<'X'\nheredoc $HOME\nX" }),
            )
            .await
            .unwrap();
        assert!(ok, "{out}");
        let canon = d.path().canonicalize().unwrap().display().to_string();
        assert!(
            out.contains(&canon) || out.contains(&d.path().display().to_string()),
            "{out}"
        );
        assert!(out.contains("it's 2"));
        assert!(
            out.contains("heredoc $HOME"),
            "heredoc 内容应原样保留：{out}"
        );
        let (ok, out) = r
            .call("Bash", &json!({ "command": "echo boom >&2; exit 3" }))
            .await
            .unwrap();
        assert!(!ok);
        assert!(out.contains("boom") && out.contains("[退出码 3]"), "{out}");
    }

    #[tokio::test]
    async fn write_read_edit_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let r = local(d.path());
        let content = "fn main() {\n    println!(\"hi '$USER' `x`\");\n}\n";
        let (ok, _) = r
            .call(
                "Write",
                &json!({ "file_path": "src/main.rs", "content": content }),
            )
            .await
            .unwrap();
        assert!(ok);
        assert_eq!(
            std::fs::read_to_string(d.path().join("src/main.rs")).unwrap(),
            content
        );

        let (ok, out) = r
            .call("Read", &json!({ "file_path": "src/main.rs" }))
            .await
            .unwrap();
        assert!(ok);
        assert!(out.contains("     2\t    println!"), "{out}");

        let (ok, out) = r
            .call(
                "Edit",
                &json!({ "file_path": "src/main.rs", "old_string": "hi", "new_string": "hello" }),
            )
            .await
            .unwrap();
        assert!(ok, "{out}");
        assert!(
            std::fs::read_to_string(d.path().join("src/main.rs"))
                .unwrap()
                .contains("hello '$USER' `x`")
        );

        std::fs::write(d.path().join("a.txt"), "x x").unwrap();
        let (ok, out) = r
            .call(
                "Edit",
                &json!({ "file_path": "a.txt", "old_string": "x", "new_string": "y" }),
            )
            .await
            .unwrap();
        assert!(!ok && out.contains("2 次"), "{out}");
        let (ok, _) = r
            .call(
                "Edit",
                &json!({ "file_path": "a.txt", "old_string": "zz", "new_string": "y" }),
            )
            .await
            .unwrap();
        assert!(!ok);
        let (ok, _) = r
            .call("Edit", &json!({ "file_path": "a.txt", "old_string": "x", "new_string": "y", "replace_all": true }))
            .await
            .unwrap();
        assert!(ok);
        assert_eq!(
            std::fs::read_to_string(d.path().join("a.txt")).unwrap(),
            "y y"
        );
    }

    #[tokio::test]
    async fn glob_and_grep() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("a/b/x.rs"), "needle here\n").unwrap();
        std::fs::write(d.path().join("y.md"), "nothing\n").unwrap();
        let r = local(d.path());
        let (ok, out) = r
            .call("Glob", &json!({ "pattern": "**/*.rs" }))
            .await
            .unwrap();
        assert!(
            ok && out.contains("a/b/x.rs") && !out.contains("y.md"),
            "{out}"
        );
        let (ok, out) = r
            .call(
                "Grep",
                &json!({ "pattern": "needle", "output_mode": "content" }),
            )
            .await
            .unwrap();
        assert!(ok && out.contains("needle here"), "{out}");
        let (ok, out) = r
            .call("Glob", &json!({ "pattern": "a/**/*.{rs,md}" }))
            .await
            .unwrap();
        assert!(ok && out.contains("a/b/x.rs"), "{out}");

        let (_, out) = r
            .call("Glob", &json!({ "pattern": "$(touch pwned)" }))
            .await
            .unwrap();
        assert!(!d.path().join("pwned").exists(), "{out}");
    }

    #[tokio::test]
    async fn read_reports_missing_and_binary() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("b.bin"), [0u8, 1, 2, 0, 255]).unwrap();
        let r = local(d.path());
        let (ok, out) = r
            .call("Read", &json!({ "file_path": "nope.txt" }))
            .await
            .unwrap();
        assert!(!ok && out.contains("不存在"));
        let (ok, out) = r
            .call("Read", &json!({ "file_path": "b.bin" }))
            .await
            .unwrap();
        assert!(!ok && out.contains("二进制"), "{out}");
    }

    #[test]
    fn glob_regex_semantics() {
        let m = |p: &str, f: &str| glob_regex(p).unwrap().is_match(f);
        assert!(m("**/*.rs", "main.rs"), "** 可匹配零层");
        assert!(m("**/*.rs", "a/b/c.rs"));
        assert!(!m("*.rs", "a/c.rs"), "* 不跨目录");
        assert!(m("src/*.{rs,toml}", "src/lib.toml"));
        assert!(m("file?.txt", "file1.txt"));
        assert!(m("a.b", "a.b") && !m("a.b", "axb"), ". 要按字面匹配");
    }

    #[test]
    fn clip_keeps_head_and_tail() {
        let s = format!("HEAD{}TAIL", "x".repeat(MAX_OUTPUT * 2));
        let c = clip(&s);
        assert!(c.starts_with("HEAD") && c.ends_with("TAIL") && c.len() < s.len());
    }

    #[tokio::test]
    async fn protocol_initialize_and_list() {
        let r = local(std::path::Path::new("/tmp"));
        let init = handle(&r, &json!({ "id": 1, "method": "initialize" }))
            .await
            .unwrap();
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("local:/tmp")
        );
        let list = handle(&r, &json!({ "id": 2, "method": "tools/list" }))
            .await
            .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Bash", "Read", "Write", "Edit", "Glob", "Grep"]);
        assert!(
            handle(&r, &json!({ "method": "notifications/initialized" }))
                .await
                .is_none()
        );
    }
}
