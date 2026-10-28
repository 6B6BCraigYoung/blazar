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
        out.push_str(&format!("! {} ({n} lines of output)", after[..end].trim()));
        rest = tail[stop..].trim_start_matches('\n');
        if !rest.is_empty() {
            out.push('\n');
        }
    }
    out.push_str(rest);
    out
}

const FLEET_WS: [&str; 7] = [
    "send_prompt",
    "wait_workspace",
    "get_history",
    "read_file",
    "get_diff",
    "list_files",
    "job_spawn",
];

pub fn is_fleet(raw_name: &str) -> bool {
    raw_name.starts_with("mcp__blazar-fleet__")
}

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

pub fn fleet_words(name: &str, input: &Value) -> Option<(&'static str, &'static str)> {
    fleet_target(name, input)?;
    Some(match name {
        "send_prompt" => ("Prompt", ""),
        "wait_workspace" => ("Wait for", ""),
        "get_history" => ("History of", ""),
        "get_diff" => ("Diff of", ""),
        "read_file" => ("Read from", ""),
        "list_files" => ("List files in", ""),
        "copy_files" => ("Copy to", ""),
        "job_spawn" => ("Start job on", ""),
        _ => return None,
    })
}

const RAW_MAX: usize = 6000;

pub fn raw_input(input: &Value) -> String {
    let pretty = if input.as_object().is_some_and(serde_json::Map::is_empty) {
        "{}".to_owned()
    } else {
        serde_json::to_string_pretty(input).unwrap_or_default()
    };
    format!("Input\n{}", short(&pretty, RAW_MAX))
}

pub fn raw_output(content: &str) -> String {
    let text = content.trim();
    let text = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| text.to_owned());
    let text = if text.is_empty() {
        "(No content)".to_owned()
    } else {
        short(&text, RAW_MAX)
    };
    format!("\nOutput\n{text}")
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
            "job_kill" | "job_log" => short(s(input, "job_id"), 40),
            "wait_workspace" | "get_history" | "get_diff" | "list_workspaces" | "list_nodes"
            | "list_jobs" | "fleet_status" => String::new(),
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
            "probe_node" | "discover_agents" => crate::vocab_en::machine(s(input, "node")),
            "create_workspace" => {
                format!(
                    "{} · {}",
                    crate::vocab_en::machine(s(input, "node")),
                    s(input, "path")
                )
            }
            _ => short(&input.to_string(), 100),
        }
    };
    let label = fleet_words(name, input).map_or(plain_label(name), |(lead, _)| lead);
    (label.to_owned(), arg)
}

pub fn plain_label(name: &str) -> &str {
    match name {
        "TodoWrite" => "Update Todos",
        "Shell" => "Bash",
        "Agent" => "Task",
        "ExitPlanMode" => "Plan",
        "send_prompt" => "Dispatch",
        "wait_workspace" => "Wait",
        "get_history" => "History",
        "read_file" => "Read",
        "list_files" => "List",
        "get_diff" => "Diff",
        "copy_files" => "Copy",
        "search_code" => "Search",
        "probe_node" => "Probe",
        "discover_agents" => "Agents on",
        "create_workspace" => "Create workspace",
        "list_workspaces" => "Workspaces",
        "list_nodes" => "Machines",
        "fleet_status" => "Fleet status",
        "ask_user" => "Ask user",
        "job_spawn" => "Start job",
        "job_register" => "Register job",
        "job_kill" => "Kill job",
        "job_log" => "Job log",
        "list_jobs" => "Jobs",
        other => other,
    }
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
        start: Some(1),
    }
}

pub fn edit_start(result: &str, rows: &[(char, String)]) -> Option<u32> {
    let first = rows.iter().find(|r| r.0 == '+').map(|r| r.1.as_str())?;
    let numbered = result.lines().filter_map(|l| {
        let (n, text) = l.split_once('\t')?;
        Some((n.trim().parse::<u32>().ok()?, text))
    });
    let mut hit = None;
    for (n, text) in numbered {
        if text == first {
            if hit.is_some() {
                return None;
            }
            hit = Some(n);
        }
    }
    hit
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
        start: None,
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
        wss: Vec::new(),
    }
}

fn count(n: usize, one: &str, none: &str) -> String {
    if n == 0 {
        none.to_owned()
    } else {
        one.replace("{n}", &n.to_string())
            .replace("{s}", if n == 1 { "" } else { "s" })
            .replace("{es}", if n == 1 { "" } else { "es" })
            .replace("{y}", if n == 1 { "y" } else { "ies" })
    }
}

fn names(list: &[Value], key: &str, max: usize) -> String {
    let all: Vec<&str> = list
        .iter()
        .map(|v| s(v, key))
        .filter(|x| !x.is_empty())
        .collect();
    let mut out = all
        .iter()
        .take(max)
        .map(|n| crate::vocab_en::machine(n))
        .collect::<Vec<_>>()
        .join(", ");
    if all.len() > max {
        out.push_str(&format!(" and {} more", all.len() - max));
    }
    out
}

fn probe_summary(v: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(c) = v["cpus"].as_u64() {
        parts.push(format!("{c} CPUs"));
    }
    if let Some(m) = v["mem_gb"].as_f64() {
        parts.push(format!("{m:.0} GB RAM"));
    }
    if let Some(d) = v["disk_free_gb"].as_f64() {
        parts.push(format!("{d:.0} GB disk free"));
    }
    if let Some(g) = v["gpus"].as_array() {
        parts.push(count(g.len(), "{n} GPU{s}", "no GPU"));
    }
    if let Some(e) = v["has_ai_egress"].as_bool() {
        parts.push(
            if e {
                "model API reachable"
            } else {
                "model API unreachable"
            }
            .to_owned(),
        );
    }
    if parts.is_empty() {
        "Probed".to_owned()
    } else {
        format!("Probed: {}", parts.join(" · "))
    }
}

fn fleet_result(name: &str, text: &str) -> Option<Res> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(match name {
        "list_workspaces" => {
            let list = v.as_array()?;
            let mut r = res(
                &count(list.len(), "Listed {n} workspace{s}", "No workspaces yet"),
                "sum",
                1,
            );
            r.wss = list
                .iter()
                .map(|w| s(w, "id"))
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
            r
        }
        "list_nodes" => {
            let list = v.as_array()?;
            let shown = names(list, "name", 6);
            res(
                &if shown.is_empty() {
                    count(list.len(), "Listed {n} machine{s}", "No machines yet")
                } else {
                    format!(
                        "Listed {} machine{}: {shown}",
                        list.len(),
                        if list.len() == 1 { "" } else { "s" }
                    )
                },
                "sum",
                1,
            )
        }
        "probe_node" => res(&probe_summary(&v), "sum", 1),
        "copy_files" => {
            let n = v["copied"].as_array().map_or(0, Vec::len);
            res(&count(n, "Copied {n} item{s}", "Nothing to copy"), "sum", 1)
        }
        "job_spawn" => res("Started", "sum", 1),
        "job_register" => res("Registered", "sum", 1),
        "job_kill" => res(
            if v["status"] == "killed" {
                "Killed"
            } else {
                "Not running"
            },
            "sum",
            1,
        ),
        "job_log" => {
            let log = s(&v, "log").trim_end();
            let n = if log.is_empty() {
                0
            } else {
                log.split('\n').count()
            };
            res(
                &if n == 0 {
                    "Log is empty".to_owned()
                } else {
                    format!("Last {n} line{}\n{log}", if n == 1 { "" } else { "s" })
                },
                "",
                4,
            )
        }
        "list_jobs" => {
            let list = v["jobs"].as_array()?;
            let lines: Vec<String> = list
                .iter()
                .map(|j| {
                    format!(
                        "{} · {}",
                        s(j, "label"),
                        crate::vocab_en::state(s(j, "status"))
                    )
                })
                .collect();
            res(
                &if lines.is_empty() {
                    "No background jobs".to_owned()
                } else {
                    format!(
                        "Listed {} job{}\n{}",
                        lines.len(),
                        if lines.len() == 1 { "" } else { "s" },
                        lines.join("\n")
                    )
                },
                "sum",
                4,
            )
        }
        _ => return None,
    })
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
                "Error"
            } else {
                text.trim_start()
            },
            "err",
            1,
        ));
    }
    Some(match name {
        "Read" => res(
            &format!("Read {n} line{}", if n == 1 { "" } else { "s" }),
            "sum",
            4,
        ),
        "TodoWrite" => return None,
        "ExitPlanMode" => res("User approved the plan", "sum", 4),
        "Edit" | "MultiEdit" | "Write" => return None,
        "Glob" | "LS" => res(
            &if n > 0 {
                format!("Found {n} file{}\n{text}", if n == 1 { "" } else { "s" })
            } else {
                "No matches found".to_owned()
            },
            "sum",
            1,
        ),
        "Grep" => res(
            &if n > 0 {
                format!("Found {n} match{}\n{text}", if n == 1 { "" } else { "es" })
            } else {
                "No matches found".to_owned()
            },
            "sum",
            1,
        ),
        "Task" | "Agent" => res(if text.is_empty() { "Done" } else { text }, "", 3),
        "send_prompt" => {
            let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            if v["admitted"] == false {
                let why = s(&v, "reason");
                res(
                    &if why.is_empty() {
                        "Not sent".to_owned()
                    } else {
                        format!("Not sent\n{why}")
                    },
                    "err",
                    4,
                )
            } else {
                res("Dispatched", "sum", 4)
            }
        }
        "wait_workspace" => {
            let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            let act = s(&v, "activity");
            let head = match act {
                "completed" | "idle" => "Finished",
                "errored" => "Errored",
                "awaiting_approval" => "Waiting for approval",
                "running" => "Still running",
                _ if v.is_null() => {
                    return Some(res(
                        if text.is_empty() {
                            "(No content)"
                        } else {
                            text
                        },
                        "",
                        4,
                    ));
                }
                _ => "Stopped",
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
            Ok(Value::Array(rows)) => res(
                &count(rows.len(), "Read {n} entr{y}", "No history yet"),
                "sum",
                4,
            ),
            _ => res(
                if text.is_empty() {
                    "(No content)"
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
                        "No changes".to_owned()
                    } else {
                        format!("{n} file{} changed", if n == 1 { "" } else { "s" })
                    },
                    "sum",
                    4,
                )
            }
            _ => res(
                if text.is_empty() {
                    "(No content)"
                } else {
                    text
                },
                "",
                4,
            ),
        },
        _ => {
            if let Some(r) = fleet_result(name, text) {
                return Some(r);
            }
            res(
                if text.is_empty() {
                    "(No content)"
                } else {
                    text
                },
                if text.is_empty() { "sum" } else { "" },
                4,
            )
        }
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
            format!(" · {:.1}s", ms as f64 / 1000.0)
        } else {
            format!(" · {}", crate::vocab_en::duration(ms as f64 / 1000.0))
        }
    });
    let status = match (ok, code) {
        (true, _) => "Success".to_owned(),
        (false, Some(c)) => format!("Exit code {c}"),
        (false, None) => "Failed".to_owned(),
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
        wss: Vec::new(),
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
            "! ls -la (2 lines of output)\nwhat is in here?"
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
            Some("Read 2 entries".into())
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
            Some("Not sent\n该工作区已有会话在运行".into())
        );
    }

    #[test]
    fn fleet_calls_read_as_sentences() {
        let send = json!({"workspace_id": "w1", "text": "收尾后把按钮放到顶栏\n再补截图"});
        assert_eq!(
            tool_title("send_prompt", &send, ""),
            ("Prompt".to_owned(), "收尾后把按钮放到顶栏".to_owned())
        );
        assert_eq!(fleet_words("send_prompt", &send), Some(("Prompt", "")));
        assert_eq!(plain_label("send_prompt"), "Dispatch");
        assert_eq!(tool_title("send_prompt", &json!({}), "").0, "Dispatch");
        assert_eq!(
            fleet_words("get_history", &json!({"workspace_id": "w1"})),
            Some(("History of", ""))
        );
        assert_eq!(
            fleet_words("wait_workspace", &json!({"workspace_id": "w1"})),
            Some(("Wait for", ""))
        );
        assert_eq!(fleet_words("list_workspaces", &json!({})), None);
        assert!(is_fleet("mcp__blazar-fleet__list_nodes") && !is_fleet("Bash"));
        assert_eq!(raw_input(&json!({})), "Input\n{}");
        assert_eq!(raw_output("{\"a\":1}"), "\nOutput\n{\n  \"a\": 1\n}");
        assert_eq!(raw_output(" "), "\nOutput\n(No content)");
    }

    #[test]
    fn fleet_results_read_as_sentences() {
        let listed = result_of(
            "list_workspaces",
            true,
            r#"[{"id":"a","name":"atlas"},{"id":"b","name":"nebula"}]"#,
        )
        .unwrap();
        assert_eq!(listed.shown, "Listed 2 workspaces");
        assert_eq!(listed.wss, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(
            result_of("list_workspaces", true, "[]").map(|r| r.shown),
            Some("No workspaces yet".into())
        );
        assert_eq!(
            result_of("list_nodes", true, r#"[{"name":"local"},{"name":"gpu-a"}]"#)
                .map(|r| r.shown),
            Some("Listed 2 machines: this machine, gpu-a".into())
        );
        assert_eq!(
            result_of(
                "probe_node",
                true,
                r#"{"cpus":8,"mem_gb":32.0,"disk_free_gb":120.4,"gpus":["A100"],"has_ai_egress":true}"#
            )
            .map(|r| r.shown),
            Some("Probed: 8 CPUs · 32 GB RAM · 120 GB disk free · 1 GPU · model API reachable".into())
        );
        assert_eq!(
            result_of(
                "copy_files",
                true,
                r#"{"copied":[{"path":"a"},{"path":"b"}]}"#
            )
            .map(|r| r.shown),
            Some("Copied 2 items".into())
        );
        assert_eq!(
            result_of("job_kill", true, r#"{"status":"killed"}"#).map(|r| r.shown),
            Some("Killed".into())
        );
        assert_eq!(
            result_of("job_log", true, r#"{"log":"a\nb\n","lines":50}"#).map(|r| r.shown),
            Some("Last 2 lines\na\nb".into())
        );
        assert_eq!(
            result_of(
                "list_jobs",
                true,
                r#"{"jobs":[{"label":"eval","status":"running"}]}"#
            )
            .map(|r| r.shown),
            Some("Listed 1 job\neval · Running".into())
        );
        assert_eq!(
            result_of("get_history", true, "[]").map(|r| r.shown),
            Some("No history yet".into())
        );
        assert_eq!(
            result_of("job_spawn", false, "没有这个工作区").map(|r| (r.shown, r.cls)),
            Some(("没有这个工作区".into(), "err"))
        );
    }

    #[test]
    fn shell_results_lead_with_the_outcome() {
        let ok = shell_result(true, "a\nb\n", Some(400));
        assert_eq!(
            (ok.shown.as_str(), ok.more, ok.cls),
            ("Success · 0.4s", 2, "sum")
        );
        assert_eq!(ok.full, "Success · 0.4s\na\nb");
        let bad = shell_result(false, "Exit code 2\n\nno such file: x\nmore", None);
        assert_eq!(
            (bad.shown.as_str(), bad.cls),
            ("Exit code 2\nno such file: x", "err")
        );
        assert_eq!(shell_result(false, "", None).shown, "Failed");
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
                start: Some(1),
                ..
            }
        ));
        let rows = [('-', "old".to_owned()), ('+', "new line".to_owned())];
        let snippet = "The file a.py has been updated. Here's the result of running `cat -n` on a snippet of the edited file:\n    11\tctx\n    12\tnew line\n    13\tafter";
        assert_eq!(edit_start(snippet, &rows), Some(12));
        assert_eq!(edit_start("The file a.py has been updated.", &rows), None);
        assert_eq!(
            edit_start("     1\tnew line\n     2\tnew line", &rows),
            None
        );
        let (_, arg) = tool_title(
            "Bash",
            &json!({"command":"cd a\nmake","description":"build"}),
            "",
        );
        assert_eq!(arg, "cd a …");
    }
}
