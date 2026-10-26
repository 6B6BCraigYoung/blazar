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

#[must_use]
pub fn is_shell(tool_name: &str) -> bool {
    let short = tool_name
        .trim_start_matches("mcp__blazar-fleet__")
        .trim_start_matches("mcp__blazar__");
    SHELL_TOOLS.contains(&short)
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
        "这条命令"
    } else {
        "这个操作"
    };
    let longest = what
        .lines()
        .map(|l| l.chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("我同意执行刚才被拦下的{noun}，请原样重试一次，不要改动：\n{fence}\n{what}\n{fence}")
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
            "我同意执行刚才被拦下的这条命令，请原样重试一次，不要改动：\n```\nssh edge.invalid 'systemctl restart demo-agent'\n```"
        );
        assert!(retry_message("Write", "/tmp/x").starts_with("我同意执行刚才被拦下的这个操作"));
        let nested = retry_message("Bash", "cat <<'EOF'\n```\nx\n```\nEOF");
        assert!(nested.contains("\n````\ncat <<'EOF'"));
        assert!(nested.ends_with("EOF\n````"));
        assert!(is_shell("mcp__blazar__Bash"));
        assert!(!is_shell("Edit"));
    }
}
