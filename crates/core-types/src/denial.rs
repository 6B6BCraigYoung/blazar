use serde_json::Value;

pub const SHELL_TOOLS: [&str; 5] = ["Bash", "Shell", "shell", "exec", "command_execution"];

const REASON_MARK: &str = "Reason: [";

#[must_use]
pub fn reason_of(content: &str) -> Option<String> {
    let start = content.find(REASON_MARK)? + REASON_MARK.len();
    let rest = &content[start..];
    let end = rest.find(']')?;
    let reason = rest[..end].trim();
    (!reason.is_empty()).then(|| reason.to_owned())
}

const REASON_NAMES: &[(&str, &str)] = &[
    ("remote shell writes", "远程命令写操作"),
    ("remote shell", "远程命令"),
    ("destructive git", "破坏性的 git 操作"),
    ("destructive operations", "破坏性操作"),
    ("destructive", "破坏性操作"),
    ("credential access", "读取凭据"),
    ("credential", "凭据"),
    ("data exfiltration", "数据外传"),
    ("exfiltration", "数据外传"),
    ("privilege escalation", "提权"),
    ("package installation", "安装软件包"),
    ("network requests", "网络请求"),
    ("system modification", "改系统设置"),
    ("file deletion", "删除文件"),
];

const REASON_WORDS: &[(&str, &str)] = &[
    ("remote", "远程"),
    ("shell", "命令"),
    ("writes", "写操作"),
    ("write", "写操作"),
    ("reads", "读取"),
    ("read", "读取"),
    ("deletion", "删除"),
    ("delete", "删除"),
    ("destructive", "破坏性"),
    ("git", "git"),
    ("credential", "凭据"),
    ("credentials", "凭据"),
    ("secret", "密钥"),
    ("secrets", "密钥"),
    ("network", "网络"),
    ("exfiltration", "外传"),
    ("install", "安装"),
    ("installation", "安装"),
    ("package", "软件包"),
    ("packages", "软件包"),
    ("sudo", "sudo"),
    ("privilege", "权限"),
    ("escalation", "提升"),
    ("system", "系统"),
    ("config", "配置"),
    ("configuration", "配置"),
    ("modification", "修改"),
    ("file", "文件"),
    ("files", "文件"),
    ("access", "访问"),
    ("operations", "操作"),
    ("operation", "操作"),
    ("execution", "执行"),
    ("commands", "命令"),
    ("command", "命令"),
];

#[must_use]
pub fn reason_label(reason: &str) -> String {
    let lower = reason.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return "安全检查".to_owned();
    }
    if let Some((_, zh)) = REASON_NAMES.iter().find(|(en, _)| *en == lower) {
        return (*zh).to_owned();
    }
    let words: Vec<String> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            REASON_WORDS
                .iter()
                .find(|(en, _)| *en == w)
                .map_or_else(|| w.to_owned(), |(_, zh)| (*zh).to_owned())
        })
        .collect();
    if words.iter().all(|w| w.is_ascii()) {
        return format!("安全规则「{}」", reason.trim());
    }
    words.join("")
}

#[must_use]
pub fn reason_label_en(reason: &str) -> String {
    let words: Vec<String> = reason
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase()
                    .chain(c.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect();
    if words.is_empty() {
        "Safety check".to_owned()
    } else {
        words.join(" ")
    }
}

#[must_use]
pub fn is_shell(tool_name: &str) -> bool {
    let short = tool_name
        .trim_start_matches("mcp__blazar-fleet__")
        .trim_start_matches("mcp__blazar__");
    SHELL_TOOLS.contains(&short)
}

const SSH_VALUE_FLAGS: [&str; 20] = [
    "-o", "-p", "-i", "-l", "-F", "-J", "-b", "-c", "-D", "-E", "-e", "-I", "-L", "-m", "-O", "-Q",
    "-R", "-S", "-W", "-w",
];

#[must_use]
pub fn ssh_target(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    words.by_ref().find(|w| *w == "ssh")?;
    while let Some(w) = words.next() {
        if SSH_VALUE_FLAGS.contains(&w) {
            words.next();
            continue;
        }
        if w.starts_with('-') {
            continue;
        }
        let host = w
            .rsplit('@')
            .next()
            .unwrap_or(w)
            .trim_matches(|c| c == '\'' || c == '"' || c == ';');
        return (!host.is_empty()).then(|| host.to_owned());
    }
    None
}

#[must_use]
pub fn what_of(tool_name: &str, input: &Value) -> String {
    let picked = ["command", "file_path", "path", "url", "pattern"]
        .iter()
        .find_map(|k| input.get(*k)?.as_str())
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_owned);
    if let Some(w) = picked {
        return w;
    }
    if input.is_null() || input.as_object().is_some_and(serde_json::Map::is_empty) {
        return tool_name.to_owned();
    }
    serde_json::to_string_pretty(input).unwrap_or_default()
}

#[must_use]
pub fn summary(tool_name: &str, input: &Value) -> String {
    let what = what_of(tool_name, input);
    if what == tool_name {
        what
    } else {
        format!("{tool_name}: {what}")
    }
}

#[must_use]
pub fn retry_message(tool_name: &str, what: &str) -> String {
    let noun = if is_shell(tool_name) {
        "command"
    } else {
        "action"
    };
    let longest = what
        .lines()
        .map(|l| l.trim_start().chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!(
        "I approve the {noun} that was just blocked. Please retry it once, exactly as \
        written:\n{fence}\n{what}\n{fence}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_reason_is_the_bracketed_category() {
        let text = "Permission for this action was denied by the Claude Code auto mode classifier. Reason: [Remote Shell Writes]. If you have other tasks";
        assert_eq!(reason_of(text).as_deref(), Some("Remote Shell Writes"));
        assert_eq!(reason_of("Permission denied"), None);
        assert_eq!(reason_of("Reason: []"), None);
    }

    #[test]
    fn classifier_rules_read_in_chinese() {
        assert_eq!(reason_label("Remote Shell Writes"), "远程命令写操作");
        assert_eq!(reason_label("Credential Access"), "读取凭据");
        assert_eq!(reason_label("Network Secret Reads"), "网络密钥读取");
        assert_eq!(reason_label("Foo Bar"), "安全规则「Foo Bar」");
        assert_eq!(reason_label(""), "安全检查");
        assert_eq!(
            reason_label_en("remote shell writes"),
            "Remote Shell Writes"
        );
        assert_eq!(reason_label_en("  "), "Safety check");
    }

    #[test]
    fn what_prefers_the_command_then_falls_back_to_the_input() {
        assert_eq!(
            what_of(
                "Bash",
                &json!({"command": " ssh edge.invalid 'systemctl restart x' ", "description": "d"})
            ),
            "ssh edge.invalid 'systemctl restart x'"
        );
        assert_eq!(
            what_of("Write", &json!({"file_path": "/etc/hosts", "content": "x"})),
            "/etc/hosts"
        );
        assert_eq!(what_of("WebSearch", &json!({})), "WebSearch");
        assert_eq!(what_of("WebSearch", &Value::Null), "WebSearch");
        assert!(what_of("X", &json!({"q": 1})).contains("\"q\": 1"));
        assert_eq!(summary("Bash", &json!({"command": "ls"})), "Bash: ls");
        assert_eq!(summary("WebSearch", &json!({})), "WebSearch");
    }

    #[test]
    fn the_retry_message_quotes_the_command_verbatim() {
        let m = retry_message("Bash", "ssh edge.invalid 'systemctl restart demo-agent'");
        assert_eq!(
            m,
            "I approve the command that was just blocked. Please retry it once, exactly as \
            written:\n```\nssh edge.invalid 'systemctl restart demo-agent'\n```"
        );
        assert!(
            retry_message("Write", "/tmp/x")
                .starts_with("I approve the action that was just blocked")
        );
        let nested = retry_message("Bash", "cat <<'EOF'\n```\nx\n```\nEOF");
        assert!(nested.contains("\n````\ncat <<'EOF'"));
        assert!(nested.ends_with("EOF\n````"));
        let indented = retry_message("Bash", "cat <<'EOF'\n  ```sh\n  x\n  ```\nEOF");
        assert!(indented.contains("\n````\ncat <<'EOF'"));
        assert!(is_shell("mcp__blazar__Bash"));
        assert!(!is_shell("Edit"));
    }

    #[test]
    fn the_ssh_target_is_read_off_the_command() {
        assert_eq!(
            ssh_target("ssh edge.invalid 'systemctl restart demo-agent'").as_deref(),
            Some("edge.invalid")
        );
        assert_eq!(
            ssh_target("ssh -o BatchMode=yes -p 2222 ops@gpu-a.invalid ls").as_deref(),
            Some("gpu-a.invalid")
        );
        assert_eq!(
            ssh_target("cd /srv && ssh -tt lab.invalid; echo done").as_deref(),
            Some("lab.invalid")
        );
        assert_eq!(ssh_target("ls -la"), None);
        assert_eq!(ssh_target("ssh -p 22"), None);
    }
}
