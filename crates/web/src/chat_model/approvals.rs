use blazar_core_types::ApprovalDecision;
use serde_json::Value;

use super::tools::{
    SHELL_TOOLS, command_of, fleet_target, is_fleet, rel_path, s, tool_name, tool_title,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetAsk {
    pub title: String,
    pub quote: String,
    pub ws: Option<String>,
}

pub fn fleet_ask(request: &Value, ws_name: Option<&str>) -> Option<FleetAsk> {
    let raw = s(request, "tool_name");
    if !is_fleet(raw) {
        return None;
    }
    let tn = tool_name(raw);
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let ws = fleet_target(&tn, &input);
    let name = ws_name
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            ws.as_ref()
                .map(|id| format!("workspace {}…", id.chars().take(8).collect::<String>()))
        })
        .unwrap_or_else(|| "this workspace".to_owned());
    let machine = match s(&input, "node") {
        "local" => "this machine".to_owned(),
        n => n.to_owned(),
    };
    let lines = |key: &str| {
        input[key]
            .as_array()
            .map(|p| {
                p.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    let (title, quote) = match tn.as_str() {
        "send_prompt" => (
            format!("Send a prompt to {name}?"),
            s(&input, "text").trim().to_owned(),
        ),
        "copy_files" => {
            let dest = s(&input, "dest").trim_matches('/');
            let paths = lines("paths");
            (
                format!("Copy files to {name}?"),
                if dest.is_empty() {
                    paths
                } else {
                    format!("{paths}\n→ {dest}/")
                },
            )
        }
        "job_spawn" => (
            format!("Start a background job on {name}?"),
            [s(&input, "label"), s(&input, "command")]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        "job_register" => (
            "Register a background job?".to_owned(),
            [
                s(&input, "label").to_owned(),
                input["pid"]
                    .as_u64()
                    .map(|p| format!("pid {p}"))
                    .unwrap_or_default(),
            ]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        ),
        "job_kill" => (
            "Kill the background job?".to_owned(),
            s(&input, "job_id").to_owned(),
        ),
        "create_workspace" => (
            format!("Create a workspace on {machine}?"),
            s(&input, "path").to_owned(),
        ),
        "wait_workspace" => (format!("Wait for {name} to finish?"), String::new()),
        "get_history" => (format!("Read the history of {name}?"), String::new()),
        "get_diff" => (format!("View the changes in {name}?"), String::new()),
        "read_file" => (
            format!("Read a file in {name}?"),
            s(&input, "path").to_owned(),
        ),
        "list_files" => (
            format!("List files in {name}?"),
            s(&input, "path").to_owned(),
        ),
        "search_code" => ("Search the code?".to_owned(), s(&input, "query").to_owned()),
        "probe_node" => (format!("Probe {machine}?"), String::new()),
        "list_workspaces" => ("List workspaces?".to_owned(), String::new()),
        "list_nodes" => ("List machines?".to_owned(), String::new()),
        other => {
            let (label, arg) = tool_title(other, &input, "");
            (format!("Allow {label}?"), arg)
        }
    };
    Some(FleetAsk { title, quote, ws })
}

pub fn decision_text(d: &ApprovalDecision) -> String {
    match d {
        ApprovalDecision::AutoAllowed { rule } if !rule.is_empty() => {
            format!("Auto-approved · {rule}")
        }
        ApprovalDecision::AutoAllowed { .. } => "Auto-approved".into(),
        ApprovalDecision::Allow => "Allowed".into(),
        ApprovalDecision::Deny { .. } => "Denied".into(),
        ApprovalDecision::Cancelled => "Withdrawn by the agent".into(),
        ApprovalDecision::Aborted => "Session ended".into(),
    }
}

pub(super) fn appr_kind_label(request: &Value) -> String {
    let tn = tool_name(s(request, "tool_name"));
    if tn == "AskUserQuestion" {
        "question".into()
    } else if SHELL_TOOLS.contains(&tn.as_str()) {
        "Bash".into()
    } else if tn.is_empty() {
        "tool".into()
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
                format!("Run this Bash command on {n}?")
            }
            _ => "Allow this Bash command?".to_owned(),
        };
    }
    if matches!(tn.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit") {
        let f = [s(&input, "file_path"), s(&input, "notebook_path")]
            .into_iter()
            .find(|x| !x.is_empty())
            .map(|f| rel_path(f, root));
        return format!(
            "Allow edits to {}?",
            f.unwrap_or_else(|| "this file".to_owned())
        );
    }
    if tn == "WebFetch" {
        return "Allow fetching this page?".to_owned();
    }
    let shown = [s(request, "display_name"), tn.as_str()]
        .into_iter()
        .find(|x| !x.is_empty())
        .unwrap_or("this tool")
        .to_owned();
    format!("Allow {shown}?")
}

pub fn approval_what(request: &Value, root: &str) -> String {
    if let Some(ask) = fleet_ask(request, None)
        && !ask.quote.is_empty()
    {
        return ask.quote;
    }
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
            [
                "Yes, and auto-accept edits",
                "Yes, and manually approve edits",
                "No, keep planning",
            ],
            "Tell Claude what to change",
        )
    } else {
        (
            ["Yes", "Yes, and don't ask again for this workspace", "No"],
            "Tell Claude what to do differently",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKey {
    Pick(usize),
    Move(i32),
    Confirm,
    Reject,
}

pub fn approval_key(key: &str, options: usize) -> Option<ApprovalKey> {
    match key {
        "ArrowDown" => Some(ApprovalKey::Move(1)),
        "ArrowUp" => Some(ApprovalKey::Move(-1)),
        "Enter" => Some(ApprovalKey::Confirm),
        "Escape" => Some(ApprovalKey::Reject),
        _ => key
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=options).contains(n))
            .map(|n| ApprovalKey::Pick(n - 1)),
    }
}

pub fn first_open(
    pending: &[super::Pending],
    decided: impl Fn(&str) -> bool,
) -> Option<&super::Pending> {
    pending.iter().find(|p| !decided(&p.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fleet_approvals_say_what_will_happen() {
        let send = json!({
            "tool_name": "mcp__blazar-fleet__send_prompt",
            "input": {"workspace_id": "w1", "text": " 收尾后把按钮放到顶栏 \n"}
        });
        let ask = fleet_ask(&send, Some("nebula-web")).unwrap();
        assert_eq!(ask.title, "Send a prompt to nebula-web?");
        assert_eq!(ask.quote, "收尾后把按钮放到顶栏");
        assert_eq!(ask.ws.as_deref(), Some("w1"));
        assert_eq!(
            fleet_ask(&send, None).unwrap().title,
            "Send a prompt to workspace w1…?"
        );
        assert_eq!(approval_what(&send, "/w"), "收尾后把按钮放到顶栏");
        let copy = json!({
            "tool_name": "mcp__blazar-fleet__copy_files",
            "input": {"from_workspace_id": "a", "to_workspace_id": "b", "paths": ["x.md", "docs"], "dest": "in/"}
        });
        let ask = fleet_ask(&copy, Some("orbit-docs")).unwrap();
        assert_eq!(ask.title, "Copy files to orbit-docs?");
        assert_eq!(ask.quote, "x.md\ndocs\n→ in/");
        let kill = json!({ "tool_name": "mcp__blazar-fleet__job_kill", "input": {"job_id": "j1"} });
        let ask = fleet_ask(&kill, None).unwrap();
        assert_eq!(
            (ask.title.as_str(), ask.quote.as_str(), ask.ws),
            ("Kill the background job?", "j1", None)
        );
        let spawn = json!({
            "tool_name": "mcp__blazar-fleet__job_spawn",
            "input": {"workspace_id": "w1", "label": "跑评测", "command": "make eval"}
        });
        let ask = fleet_ask(&spawn, Some("quasar-ml")).unwrap();
        assert_eq!(ask.title, "Start a background job on quasar-ml?");
        assert_eq!(ask.quote, "跑评测\nmake eval");
        assert!(fleet_ask(&json!({"tool_name": "Bash", "input": {}}), None).is_none());
        assert_eq!(
            approval_title(&json!({"tool_name": "WebSearch", "input": {}}), "", None),
            "Allow WebSearch?"
        );
    }

    #[test]
    fn approval_keys_follow_the_cli() {
        assert_eq!(approval_key("1", 3), Some(ApprovalKey::Pick(0)));
        assert_eq!(approval_key("3", 3), Some(ApprovalKey::Pick(2)));
        assert_eq!(approval_key("4", 3), None);
        assert_eq!(approval_key("0", 3), None);
        assert_eq!(approval_key("ArrowDown", 3), Some(ApprovalKey::Move(1)));
        assert_eq!(approval_key("ArrowUp", 3), Some(ApprovalKey::Move(-1)));
        assert_eq!(approval_key("Enter", 3), Some(ApprovalKey::Confirm));
        assert_eq!(approval_key("Escape", 3), Some(ApprovalKey::Reject));
        assert_eq!(approval_key("a", 3), None);
    }
}
