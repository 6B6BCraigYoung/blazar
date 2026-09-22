use blazar_core_types::{EntryKind, Outcome, ProviderSessionId, TokenUsage, ToolId};
use serde_json::Value;

#[must_use]
pub fn parse_line(line: &str) -> Vec<EntryKind> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    match v.get("type").and_then(Value::as_str).unwrap_or_default() {
        "thread.started" => v
            .get("thread_id")
            .and_then(Value::as_str)
            .map(|id| {
                vec![EntryKind::SessionStarted {
                    provider_session_id: ProviderSessionId(id.to_owned()),
                    model: str_of(&v, "model"),
                    cwd: str_of(&v, "cwd"),
                    permission_mode: None,
                    output_style: None,
                    fast_mode: None,
                    fast_mode_reason: None,
                    mcp_servers: Vec::new(),
                }]
            })
            .unwrap_or_default(),

        "item.started" => v
            .get("item")
            .map(|i| parse_item(i, Phase::Started))
            .unwrap_or_default(),
        "item.completed" => v
            .get("item")
            .map(|i| parse_item(i, Phase::Completed))
            .unwrap_or_default(),

        "turn.completed" => {
            let usage = v.get("usage").map(parse_usage);
            let mut out = Vec::new();
            if let Some(u) = usage.clone() {
                out.push(EntryKind::TokenUsage(u));
            }
            out.push(EntryKind::Finished(Outcome::Success {
                text: None,
                usage,
                denied: Vec::new(),
            }));
            out
        }

        "turn.failed" => vec![EntryKind::Finished(Outcome::Failed {
            message: v
                .get("error")
                .and_then(|e| e.get("message").and_then(Value::as_str))
                .or_else(|| v.get("message").and_then(Value::as_str))
                .unwrap_or("turn 失败")
                .to_owned(),
        })],

        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Started,
    Completed,
}

fn parse_item(item: &Value, phase: Phase) -> Vec<EntryKind> {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let id = || ToolId(str_of(item, "id").unwrap_or_else(|| kind.to_owned()));
    let done = phase == Phase::Completed;

    let text_only_when_done = |k: EntryKind| if done { vec![k] } else { Vec::new() };

    match kind {
        "agent_message" => text_of(item, &["text", "message"])
            .map(|text| text_only_when_done(EntryKind::AssistantMessage { text }))
            .unwrap_or_default(),

        "reasoning" => text_of(item, &["text", "summary", "content"])
            .map(|text| text_only_when_done(EntryKind::Thinking { text }))
            .unwrap_or_default(),

        "error" => text_of(item, &["message", "text"])
            .map(|message| text_only_when_done(EntryKind::Error { message }))
            .unwrap_or_default(),

        "command_execution" => {
            if !done {
                let cmd = text_of(item, &["command", "cmd"]).unwrap_or_default();
                return vec![EntryKind::ToolUse {
                    id: id(),
                    name: "Shell".to_owned(),
                    input: serde_json::json!({ "command": cmd }),
                }];
            }
            let code = item.get("exit_code").and_then(Value::as_i64);
            vec![EntryKind::ToolResult {
                id: id(),
                ok: code.is_none_or(|c| c == 0),
                content: text_of(item, &["aggregated_output", "output", "stdout"])
                    .unwrap_or_default(),
                structured: Some(item.clone()),
            }]
        }

        "file_change" | "patch_apply" if !done => vec![EntryKind::ToolUse {
            id: id(),
            name: "Edit".to_owned(),
            input: item.get("changes").cloned().unwrap_or_else(|| item.clone()),
        }],

        "mcp_tool_call" if !done => vec![EntryKind::ToolUse {
            id: id(),
            name: text_of(item, &["tool", "name"]).unwrap_or_else(|| "MCP".to_owned()),
            input: item.get("arguments").cloned().unwrap_or(Value::Null),
        }],

        "mcp_tool_call" => {
            let result = item.get("result");
            let text = result
                .and_then(|r| r.get("content"))
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .or_else(|| text_of(item, &["error"]))
                .unwrap_or_default();
            let failed = item.get("status").and_then(Value::as_str) == Some("failed")
                || item.get("error").is_some_and(|e| !e.is_null())
                || result
                    .and_then(|r| r.get("is_error").or_else(|| r.get("isError")))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            vec![EntryKind::ToolResult {
                id: id(),
                ok: !failed,
                content: text,
                structured: None,
            }]
        }

        "web_search" if !done => vec![EntryKind::ToolUse {
            id: id(),
            name: "WebSearch".to_owned(),
            input: serde_json::json!({ "query": text_of(item, &["query"]).unwrap_or_default() }),
        }],

        "todo_list" => text_of(item, &["text"])
            .map(|text| text_only_when_done(EntryKind::Thinking { text }))
            .unwrap_or_default(),

        _ => Vec::new(),
    }
}

fn parse_usage(u: &Value) -> TokenUsage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input: n("input_tokens"),

        output: n("output_tokens") + n("reasoning_output_tokens"),
        cache_read: n("cached_input_tokens"),
        cache_creation: n("cache_write_input_tokens"),
        cost_usd: None,
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn text_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        v.get(*k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &[&str] = &[
        r#"{"type":"thread.started","thread_id":"0192a7b0-5c6d-7e8f-9a0b-1c2d3e4f5a6b"}"#,
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Skill descriptions were shortened to fit the skills context budget."}}"#,
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Hi, lovely to meet you."}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":18697,"cached_input_tokens":11008,"cache_write_input_tokens":0,"output_tokens":11,"reasoning_output_tokens":0}}"#,
    ];

    #[test]
    fn parses_the_real_capture_end_to_end() {
        let all: Vec<_> = REAL.iter().flat_map(|l| parse_line(l)).collect();
        assert!(matches!(all[0], EntryKind::SessionStarted { .. }));
        assert!(matches!(all[1], EntryKind::Error { .. }));
        assert!(matches!(all[2], EntryKind::AssistantMessage { .. }));
        assert!(matches!(all[3], EntryKind::TokenUsage(_)));
        assert!(matches!(
            all[4],
            EntryKind::Finished(Outcome::Success { .. })
        ));
    }

    #[test]
    fn thread_id_is_captured_for_resume() {
        let out = parse_line(REAL[0]);
        let [
            EntryKind::SessionStarted {
                provider_session_id,
                ..
            },
        ] = out.as_slice()
        else {
            panic!("应产出 SessionStarted");
        };
        assert_eq!(
            provider_session_id.0,
            "0192a7b0-5c6d-7e8f-9a0b-1c2d3e4f5a6b"
        );
    }

    #[test]
    fn reasoning_tokens_count_as_output() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5,"reasoning_output_tokens":100}}"#;
        let out = parse_line(line);
        let EntryKind::TokenUsage(u) = &out[0] else {
            panic!("应先产出 TokenUsage");
        };
        assert_eq!(u.output, 105);
    }

    #[test]
    fn started_and_completed_do_not_duplicate_the_tool_call() {
        let item = r#"{"id":"c1","type":"command_execution","command":"ls","aggregated_output":"a","exit_code":0}"#;
        let started = parse_line(&format!(r#"{{"type":"item.started","item":{item}}}"#));
        let completed = parse_line(&format!(r#"{{"type":"item.completed","item":{item}}}"#));
        assert_eq!(started.len(), 1);
        assert!(matches!(started[0], EntryKind::ToolUse { .. }));
        assert_eq!(completed.len(), 1);
        assert!(matches!(completed[0], EntryKind::ToolResult { .. }));
    }

    #[test]
    fn message_is_emitted_once_on_completion_only() {
        let item = r#"{"id":"m1","type":"agent_message","text":"hi"}"#;
        assert!(parse_line(&format!(r#"{{"type":"item.started","item":{item}}}"#)).is_empty());
        assert_eq!(
            parse_line(&format!(r#"{{"type":"item.completed","item":{item}}}"#)).len(),
            1
        );
    }

    #[test]
    fn command_execution_yields_call_and_result() {
        let started = r#"{"type":"item.started","item":{"id":"c1","type":"command_execution",
            "command":"ls -la"}}"#;
        let completed = r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution",
            "command":"ls -la","aggregated_output":"a.txt","exit_code":0}}"#;
        let a = parse_line(started);
        let b = parse_line(completed);
        assert!(matches!(&a[0], EntryKind::ToolUse { name, .. } if name == "Shell"));
        assert!(matches!(&b[0], EntryKind::ToolResult { ok: true, .. }));
    }

    #[test]
    fn nonzero_exit_marks_result_failed() {
        let line = r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution",
            "command":"false","aggregated_output":"boom","exit_code":1}}"#;
        let out = parse_line(line);
        assert!(matches!(&out[0], EntryKind::ToolResult { ok: false, .. }));
    }

    #[test]
    fn turn_failed_becomes_failure_outcome() {
        let line = r#"{"type":"turn.failed","error":{"message":"额度耗尽"}}"#;
        let out = parse_line(line);
        assert!(matches!(
            &out[0],
            EntryKind::Finished(Outcome::Failed { message }) if message == "额度耗尽"
        ));
    }

    #[test]
    fn unknown_events_and_items_are_ignored_not_fatal() {
        assert!(parse_line(r#"{"type":"some.future.event"}"#).is_empty());
        assert!(parse_line(r#"{"type":"item.completed","item":{"type":"未来类型"}}"#).is_empty());
        assert!(parse_line(r#"{"type":"item.started","item":{"type":"未来类型"}}"#).is_empty());
        assert!(parse_line("not json").is_empty());
    }

    #[test]
    fn mcp_tool_call_yields_use_then_result() {
        let started = r#"{"type":"item.started","item":{"id":"item_2","type":"mcp_tool_call","server":"blazar","tool":"Read","arguments":{"file_path":"hello.py"},"result":null,"error":null,"status":"in_progress"}}"#;
        let done = r#"{"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"blazar","tool":"Read","arguments":{"file_path":"hello.py"},"result":{"content":[{"type":"text","text":"     1\timport socket\n"}],"structured_content":null},"error":null,"status":"completed"}}"#;
        let a = parse_line(started);
        assert!(matches!(&a[0], EntryKind::ToolUse { name, .. } if name == "Read"));
        let b = parse_line(done);
        match &b[0] {
            EntryKind::ToolResult {
                id, ok, content, ..
            } => {
                assert_eq!(id.0, "item_2", "结果要能挂回同一个调用");
                assert!(*ok);
                assert!(content.contains("import socket"));
            }
            other => panic!("期望 ToolResult，实得 {other:?}"),
        }
    }
}
