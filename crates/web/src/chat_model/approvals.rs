use blazar_core_types::ApprovalDecision;
use serde_json::Value;

use super::tools::{SHELL_TOOLS, command_of, rel_path, s, tool_name};

pub fn decision_text(d: &ApprovalDecision) -> String {
    match d {
        ApprovalDecision::AutoAllowed { rule } if !rule.is_empty() => {
            format!("已自动批准 · {rule}")
        }
        ApprovalDecision::AutoAllowed { .. } => "已自动批准".into(),
        ApprovalDecision::Allow => "已允许".into(),
        ApprovalDecision::Deny { .. } => "已拒绝".into(),
        ApprovalDecision::Cancelled => "智能体已撤回".into(),
        ApprovalDecision::Aborted => "会话已结束".into(),
    }
}

pub(super) fn appr_kind_label(request: &Value) -> String {
    let tn = tool_name(s(request, "tool_name"));
    if tn == "AskUserQuestion" {
        "提问".into()
    } else if SHELL_TOOLS.contains(&tn.as_str()) {
        "Bash".into()
    } else if tn.is_empty() {
        "工具".into()
    } else {
        tn
    }
}

pub fn approval_title(request: &Value, root: &str, remote_node: Option<&str>) -> String {
    let tn = tool_name(s(request, "tool_name"));
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    if SHELL_TOOLS.contains(&tn.as_str()) {
        return match remote_node {
            Some(n) if s(request, "tool_name").starts_with("mcp__blazar__") => {
                format!("在 {n} 上运行这条 Bash 命令？")
            }
            _ => "允许运行这条 Bash 命令？".to_owned(),
        };
    }
    if matches!(tn.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit") {
        let f = [s(&input, "file_path"), s(&input, "notebook_path")]
            .into_iter()
            .find(|x| !x.is_empty())
            .map(|f| rel_path(f, root));
        return match f {
            Some(f) => format!("允许编辑 {f}？"),
            None => "允许编辑这个文件？".to_owned(),
        };
    }
    if tn == "WebFetch" {
        return "允许读取这个网页？".to_owned();
    }
    let shown = [s(request, "display_name"), tn.as_str()]
        .into_iter()
        .find(|x| !x.is_empty())
        .unwrap_or("这个工具")
        .to_owned();
    format!("允许使用 {shown}？")
}

pub fn approval_what(request: &Value, root: &str) -> String {
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let cmd = command_of(&input);
    if !cmd.is_empty() {
        return cmd;
    }
    if !s(&input, "url").is_empty() {
        return s(&input, "url").to_owned();
    }
    if !s(&input, "file_path").is_empty() {
        return rel_path(s(&input, "file_path"), root);
    }
    let pretty = serde_json::to_string_pretty(&input).unwrap_or_default();
    pretty.chars().take(800).collect()
}

pub fn always_rule(request: &Value) -> (String, String) {
    let tn = tool_name(s(request, "tool_name"));
    let cmd = request
        .get("input")
        .and_then(|i| i.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let w: Vec<&str> = cmd.split_whitespace().collect();
    let pattern = match w.as_slice() {
        [a, b, ..] if !b.starts_with('-') => format!("{a} {b}"),
        [a, ..] => (*a).to_owned(),
        [] => String::new(),
    };
    (tn, pattern)
}

#[must_use]
pub fn approval_choices(plan: bool) -> ([&'static str; 3], &'static str) {
    if plan {
        (
            ["继续，自动接受编辑", "继续，逐个批准编辑", "不，继续规划"],
            "告诉 Claude 要改什么",
        )
    } else {
        (
            ["允许", "允许，并且这个工作区不再询问", "拒绝"],
            "告诉 Claude 换个做法",
        )
    }
}

pub fn first_open(
    pending: &[super::Pending],
    decided: impl Fn(&str) -> bool,
) -> Option<&super::Pending> {
    pending.iter().find(|p| !decided(&p.id))
}
