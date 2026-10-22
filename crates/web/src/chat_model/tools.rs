use serde_json::Value;

use super::{Extra, Res, Todo};

pub const SHELL_TOOLS: [&str; 5] = ["Bash", "Shell", "shell", "exec", "command_execution"];

pub fn tool_name(n: &str) -> String {
    n.trim_start_matches("mcp__blazar-fleet__")
        .trim_start_matches("mcp__blazar__")
        .to_owned()
}

pub(super) fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        s.to_owned()
    }
}

pub fn rel_path(p: &str, root: &str) -> String {
    let root = root.trim_end_matches('/');
    match p.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
        Some(r) if !root.is_empty() => r.to_owned(),
        _ => p.to_owned(),
    }
}

pub(super) fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn command_of(input: &Value) -> String {
    match input.get("command") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        Some(Value::String(c)) => c.clone(),
        _ => String::new(),
    }
}

pub fn shell_display(text: &str) -> String {
    if !text.contains("<bash-input>") {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("<bash-input>") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "<bash-input>".len()..];
        let Some(end) = after.find("</bash-input>") else {
            out.push_str(&rest[at..]);
            return out;
        };
        let lines = |tag: &str, s: &str| -> usize {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            s.find(&open)
                .and_then(|a| {
                    s[a + open.len()..]
                        .find(&close)
                        .map(|b| s[a + open.len()..a + open.len() + b].lines().count())
                })
                .unwrap_or(0)
        };
        let tail = &after[end + "</bash-input>".len()..];
        let stop = tail
            .find("</bash-stderr>")
            .map(|i| i + "</bash-stderr>".len())
            .unwrap_or(0);
        let n = lines("bash-stdout", &tail[..stop]) + lines("bash-stderr", &tail[..stop]);
        out.push_str(&format!("! {}（输出 {n} 行）", after[..end].trim()));
        rest = tail[stop..].trim_start_matches('\n');
        if !rest.is_empty() {
            out.push('\n');
        }
    }
    out.push_str(rest);
    out
}

const FLEET_WS: [&str; 6] = [
    "send_prompt",
    "wait_workspace",
    "get_history",
    "read_file",
    "get_diff",
    "list_files",
];

pub fn fleet_target(name: &str, input: &Value) -> Option<String> {
    let id = if FLEET_WS.contains(&name) {
        s(input, "workspace_id")
    } else if name == "copy_files" {
        s(input, "to_workspace_id")
    } else {
        ""
    };
    (!id.is_empty()).then(|| id.to_owned())
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

pub fn tool_title(name: &str, input: &Value, root: &str) -> (String, String) {
    let rp = |k: &str| rel_path(s(input, k), root);
    let arg = if SHELL_TOOLS.contains(&name) {
        let c = command_of(input);
        let c = c.trim();
        match c.split_once('\n') {
            Some((head, _)) => format!("{} …", short(head.trim_end(), 140)),
            None if c.is_empty() => short(s(input, "description"), 140),
            None => short(c, 140),
        }
    } else {
        match name {
            "Read" => {
                let off = input
                    .get("offset")
                    .and_then(Value::as_i64)
                    .filter(|o| *o > 0);
                format!(
                    "{}{}",
                    rp("file_path"),
                    off.map(|o| format!(":{o}")).unwrap_or_default()
                )
            }
            "Write" | "Edit" | "MultiEdit" => rp("file_path"),
            "NotebookEdit" => rp("notebook_path"),
            "Glob" => {
                let p = s(input, "path");
                format!(
                    "{}{}",
                    s(input, "pattern"),
                    if p.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", rel_path(p, root))
                    }
                )
            }
            "Grep" => {
                let p = s(input, "path");
                format!(
                    "\"{}\"{}",
                    short(s(input, "pattern"), 60),
                    if p.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", rel_path(p, root))
                    }
                )
            }
            "LS" => rp("path"),
            "WebFetch" => s(input, "url").to_owned(),
            "WebSearch" => s(input, "query").to_owned(),
            "Task" | "Agent" | "Install" => s(input, "description").to_owned(),
            "TodoWrite" | "ExitPlanMode" => String::new(),
            "send_prompt" => short(first_line(s(input, "text")), 140),
            "job_spawn" | "job_register" => short(s(input, "label"), 100),
            "wait_workspace" | "get_history" | "get_diff" | "list_workspaces" | "list_nodes"
            | "fleet_status" => String::new(),
            "read_file" | "list_files" => s(input, "path").to_owned(),
            "search_code" => format!("\"{}\"", short(s(input, "query"), 60)),
            "copy_files" => short(
                &input["paths"]
                    .as_array()
                    .map(|p| {
                        p.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
                100,
            ),
            "probe_node" | "discover_agents" => {
                blazar_core_types::api::machine_label(s(input, "node")).to_owned()
            }
            "create_workspace" => format!(
                "{} · {}",
                blazar_core_types::api::machine_label(s(input, "node")),
                s(input, "path")
            ),
            _ => short(&input.to_string(), 100),
        }
    };
    let label = match name {
        "TodoWrite" => "更新待办",
        "Shell" => "Bash",
        "Agent" => "Task",
        "ExitPlanMode" => "计划",
        "send_prompt" => "派活",
        "wait_workspace" => "等待",
        "get_history" => "读取记录",
        "read_file" => "Read",
        "list_files" => "列出文件",
        "get_diff" => "查看改动",
        "copy_files" => "复制文件",
        "search_code" => "搜索代码",
        "probe_node" => "体检机器",
        "discover_agents" => "查找智能体",
        "create_workspace" => "新建工作区",
        "list_workspaces" => "列出工作区",
        "list_nodes" => "列出机器",
        "job_spawn" => "启动后台任务",
        "job_register" => "登记后台任务",
        "job_kill" => "结束后台任务",
        "job_log" => "查看后台任务日志",
        "list_jobs" => "列出后台任务",
        other => other,
    };
    (label.to_owned(), arg)
}

pub fn todos_of(input: &Value) -> Vec<Todo> {
    input
        .get("todos")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|t| Todo {
                    content: [s(t, "content"), s(t, "activeForm")]
                        .into_iter()
                        .find(|x| !x.is_empty())
                        .unwrap_or("")
                        .to_owned(),
                    status: [s(t, "status")]
                        .into_iter()
                        .find(|x| !x.is_empty())
                        .unwrap_or("pending")
                        .to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

const DIFF_MAX: usize = 400;

pub fn write_preview(input: &Value) -> Extra {
    let content = s(input, "content");
    if content.is_empty() {
        return Extra::None;
    }
    let lines: Vec<&str> = content.trim_end_matches('\n').split('\n').collect();
    Extra::Diff {
        rows: lines
            .iter()
            .take(DIFF_MAX)
            .map(|l| ('+', (*l).to_owned()))
            .collect(),
        more: lines.len().saturating_sub(DIFF_MAX),
        added: lines.len(),
        removed: 0,
    }
}

pub fn edit_diff(input: &Value, root: &str) -> Extra {
    let edits: Vec<&Value> = match input.get("edits").and_then(Value::as_array) {
        Some(a) => a.iter().collect(),
        None if input.get("old_string").is_some() || input.get("new_string").is_some() => {
            vec![input]
        }
        None => {
            let files: Vec<String> = input
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            [s(c, "path"), s(c, "file")]
                                .into_iter()
                                .find(|x| !x.is_empty())
                                .unwrap_or("")
                        })
                        .filter(|x| !x.is_empty())
                        .map(|f| rel_path(f, root))
                        .collect()
                })
                .unwrap_or_default();
            return if files.is_empty() {
                Extra::None
            } else {
                Extra::Files(files)
            };
        }
    };
    let mut rows = Vec::new();
    for e in &edits {
        let old = s(e, "old_string");
        let new = s(e, "new_string");
        if !old.is_empty() {
            rows.extend(old.split('\n').map(|l| ('-', l.to_owned())));
        }
        if !new.is_empty() {
            rows.extend(new.split('\n').map(|l| ('+', l.to_owned())));
        }
        if edits.len() > 1 {
            rows.push(('⋯', String::new()));
        }
    }
    let count = |c: char| rows.iter().filter(|r| r.0 == c).count();
    let (added, removed) = (count('+'), count('-'));
    let more = rows.len().saturating_sub(DIFF_MAX);
    rows.truncate(DIFF_MAX);
    Extra::Diff {
        rows,
        more,
        added,
        removed,
    }
}

fn res(text: &str, cls: &'static str, max: usize) -> Res {
    let lines: Vec<&str> = text.split('\n').collect();
    Res {
        shown: lines
            .iter()
            .take(max)
            .copied()
            .collect::<Vec<_>>()
            .join("\n"),
        full: text.to_owned(),
        more: lines.len().saturating_sub(max),
        cls,
    }
}

pub fn result_of(name: &str, ok: bool, content: &str) -> Option<Res> {
    let text = content.trim_end();
    let n = if text.is_empty() {
        0
    } else {
        text.split('\n').count()
    };
    if !ok {
        return Some(res(
            if text.is_empty() {
                "出错了"
            } else {
                text.trim_start()
            },
            "err",
            1,
        ));
    }
    Some(match name {
        "Read" => res(&format!("读取了 {n} 行"), "sum", 4),
        "TodoWrite" => return None,
        "ExitPlanMode" => res("你已批准这个计划", "sum", 4),
        "Edit" | "MultiEdit" | "Write" => return None,
        "Glob" | "LS" => res(
            &if n > 0 {
                format!("找到 {n} 个文件\n{text}")
            } else {
                "没有找到匹配".to_owned()
            },
            "sum",
            1,
        ),
        "Grep" => res(
            &if n > 0 {
                format!("找到 {n} 处匹配\n{text}")
            } else {
                "没有找到匹配".to_owned()
            },
            "sum",
            1,
        ),
        "Task" | "Agent" => res(if text.is_empty() { "已完成" } else { text }, "", 3),
        "send_prompt" => {
            let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            if v["admitted"] == false {
                let why = s(&v, "reason");
                res(
                    &if why.is_empty() {
                        "没有送达".to_owned()
                    } else {
                        format!("没有送达\n{why}")
                    },
                    "err",
                    4,
                )
            } else {
                res("已派出", "sum", 4)
            }
        }
        "wait_workspace" => {
            let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            let act = s(&v, "activity");
            let head = match act {
                "completed" | "idle" => "已完成",
                "errored" => "失败",
                "awaiting_approval" => "等你批准",
                "running" => "仍在运行",
                _ if v.is_null() => {
                    return Some(res(
                        if text.is_empty() {
                            "（没有内容）"
                        } else {
                            text
                        },
                        "",
                        4,
                    ));
                }
                _ => "已停止",
            };
            let reply = s(&v, "last_reply").trim();
            res(
                &if reply.is_empty() {
                    head.to_owned()
                } else {
                    format!("{head}\n{reply}")
                },
                if act == "errored" { "err" } else { "" },
                4,
            )
        }
        "get_history" => match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(rows)) => res(&format!("读取了 {} 条记录", rows.len()), "sum", 4),
            _ => res(
                if text.is_empty() {
                    "（没有内容）"
                } else {
                    text
                },
                "",
                4,
            ),
        },
        "get_diff" => match serde_json::from_str::<Value>(text) {
            Ok(v) if v["files"].is_array() => {
                let n = v["files"].as_array().map_or(0, Vec::len);
                res(
                    &if n == 0 {
                        "没有改动".to_owned()
                    } else {
                        format!("{n} 个文件有改动")
                    },
                    "sum",
                    4,
                )
            }
            _ => res(
                if text.is_empty() {
                    "（没有内容）"
                } else {
                    text
                },
                "",
                4,
            ),
        },
        "copy_files" => res("已复制", "sum", 4),
        _ => res(
            if text.is_empty() {
                "（没有内容）"
            } else {
                text
            },
            if text.is_empty() { "sum" } else { "" },
            4,
        ),
    })
}

fn exit_code(line: &str) -> Option<i32> {
    let l = line.trim().to_ascii_lowercase();
    l.strip_prefix("exit code")
        .map(|r| r.trim_start_matches([':', ' ']))
        .and_then(|r| r.parse().ok())
}

pub(super) fn shell_result(ok: bool, content: &str, ms: Option<u64>) -> Res {
    let text = content.trim_end();
    let mut lines: Vec<&str> = text.split('\n').collect();
    let code = lines.first().and_then(|l| exit_code(l));
    if code.is_some() {
        lines.remove(0);
    }
    let output = lines.join("\n").trim_matches('\n').to_owned();
    let n = if output.is_empty() {
        0
    } else {
        output.split('\n').count()
    };
    let took = ms.map(|ms| {
        if ms < 10_000 {
            format!(" · {:.1} 秒", ms as f64 / 1000.0)
        } else {
            format!(" · {}", crate::vocab::duration(ms as f64 / 1000.0))
        }
    });
    let status = match (ok, code) {
        (true, _) => "成功".to_owned(),
        (false, Some(c)) => format!("退出码 {c}"),
        (false, None) => "失败".to_owned(),
    } + took.as_deref().unwrap_or("");
    let shown = match first_line(&output) {
        line if !ok && !line.is_empty() => format!("{status}\n{line}"),
        _ => status,
    };
    let full = if output.is_empty() {
        shown.clone()
    } else {
        format!("{}\n{output}", shown.lines().next().unwrap_or_default())
    };
    Res {
        more: if output.is_empty() { 0 } else { n },
        shown,
        full,
        cls: if ok { "sum" } else { "err" },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shell_context_is_shown_as_a_command_line() {
        let text = "<bash-input>ls -la</bash-input>\n\n<bash-stdout>a\nb</bash-stdout><bash-stderr></bash-stderr>\n\nwhat is in here?";
        assert_eq!(
            shell_display(text),
            "! ls -la（输出 2 行）\nwhat is in here?"
        );
        assert_eq!(shell_display("plain"), "plain");
        assert_eq!(shell_display("<bash-input>broken"), "<bash-input>broken");
    }

    #[test]
    fn fleet_targets_only_workspace_tools() {
        assert_eq!(
            fleet_target(
                "copy_files",
                &json!({"from_workspace_id":"a","to_workspace_id":"b"})
            ),
            Some("b".into())
        );
        assert_eq!(fleet_target("search_code", &json!({"query":"x"})), None);
        assert_eq!(fleet_target("send_prompt", &json!({})), None);
        assert_eq!(
            result_of("get_history", true, "[{},{}]").map(|r| r.shown),
            Some("读取了 2 条记录".into())
        );
        assert_eq!(
            result_of("wait_workspace", true, "timed out").map(|r| r.shown),
            Some("timed out".into())
        );
        assert_eq!(
            result_of(
                "send_prompt",
                true,
                r#"{"admitted":false,"reason":"该工作区已有会话在运行"}"#
            )
            .map(|r| r.shown),
            Some("没有送达\n该工作区已有会话在运行".into())
        );
    }

    #[test]
    fn shell_results_lead_with_the_outcome() {
        let ok = shell_result(true, "a\nb\n", Some(400));
        assert_eq!(
            (ok.shown.as_str(), ok.more, ok.cls),
            ("成功 · 0.4 秒", 2, "sum")
        );
        assert_eq!(ok.full, "成功 · 0.4 秒\na\nb");
        let bad = shell_result(false, "Exit code 2\n\nno such file: x\nmore", None);
        assert_eq!(
            (bad.shown.as_str(), bad.cls),
            ("退出码 2\nno such file: x", "err")
        );
        assert_eq!(shell_result(false, "", None).shown, "失败");
        assert_eq!(shell_result(true, "", None).more, 0);
    }

    #[test]
    fn edits_count_what_they_add_and_remove() {
        let e = edit_diff(&json!({"old_string":"a\nb","new_string":"c"}), "");
        assert!(matches!(
            e,
            Extra::Diff {
                added: 1,
                removed: 2,
                more: 0,
                ..
            }
        ));
        let w = write_preview(&json!({"content":"x\ny\n"}));
        assert!(matches!(
            w,
            Extra::Diff {
                added: 2,
                removed: 0,
                ..
            }
        ));
        let (_, arg) = tool_title(
            "Bash",
            &json!({"command":"cd a\nmake","description":"build"}),
            "",
        );
        assert_eq!(arg, "cd a …");
    }
}
