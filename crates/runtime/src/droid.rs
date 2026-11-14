use std::collections::VecDeque;

use blazar_core_types::{EntryKind, Outcome, ProviderSessionId, TokenUsage, ToolId};
use serde_json::{Value, json};

#[derive(Debug, Default)]
pub struct Turn {
    calls: VecDeque<String>,
    reason: Option<String>,
    thought: Option<(String, String)>,
}

fn text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
        .filter(|s| !s.trim().is_empty())
}

#[must_use]
pub fn tool(name: &str, p: &Value) -> (String, Value) {
    let path = || text(p, &["file_path", "path"]);
    let mapped = match name {
        "Read" => path().map(|f| ("Read", json!({ "file_path": f }))),
        "LS" => text(p, &["directory_path", "path"]).map(|d| ("LS", json!({ "path": d }))),
        "Glob" => p
            .get("patterns")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|s| !s.is_empty())
            .or_else(|| text(p, &["pattern"]))
            .map(|pattern| {
                (
                    "Glob",
                    json!({ "pattern": pattern, "path": text(p, &["folder", "path"]) }),
                )
            }),
        "Grep" => text(p, &["pattern"])
            .map(|pattern| ("Grep", json!({ "pattern": pattern, "path": text(p, &["path"]) }))),
        "Execute" => text(p, &["command"]).map(|c| ("Bash", json!({ "command": c }))),
        "Edit" => path().map(|f| {
            (
                "Edit",
                json!({
                    "file_path": f,
                    "old_string": text(p, &["old_string", "old_str"]).unwrap_or_default(),
                    "new_string": text(p, &["new_string", "new_str"]).unwrap_or_default(),
                }),
            )
        }),
        "MultiEdit" => path().map(|f| {
            (
                "MultiEdit",
                json!({ "file_path": f, "edits": p.get("edits").or_else(|| p.get("changes")).cloned() }),
            )
        }),
        "Create" => path().map(|f| {
            (
                "Write",
                json!({ "file_path": f, "content": text(p, &["content"]).unwrap_or_default() }),
            )
        }),
        "FetchUrl" => text(p, &["url"]).map(|u| ("WebFetch", json!({ "url": u }))),
        _ => None,
    };
    mapped.map_or_else(
        || (name.to_owned(), p.clone()),
        |(n, input)| (n.to_owned(), input),
    )
}

fn call_id(v: &Value) -> Option<String> {
    text(v, &["id", "toolId"])
}

fn usage(v: &Value) -> Option<TokenUsage> {
    let u = v.get("usage").filter(|u| u.is_object())?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(TokenUsage {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_read: n("cache_read_input_tokens"),
        cache_creation: n("cache_creation_input_tokens"),
        cost_usd: None,
        context: None,
        window: None,
    })
}

fn result_content(v: &Value) -> String {
    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
        return text(e, &["message"]).unwrap_or_else(|| e.to_string());
    }
    match v.get("value") {
        Some(Value::String(s)) => s.clone(),
        None | Some(Value::Null) => String::new(),
        Some(other) => other.to_string(),
    }
}

#[must_use]
pub fn parse_line(turn: &mut Turn, line: &str) -> Vec<EntryKind> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    match v.get("type").and_then(Value::as_str).unwrap_or_default() {
        "system" => text(&v, &["session_id"])
            .map(|id| {
                vec![EntryKind::SessionStarted {
                    provider_session_id: ProviderSessionId(id),
                    model: text(&v, &["model"]),
                    cwd: text(&v, &["cwd"]),
                    permission_mode: None,
                    output_style: None,
                    fast_mode: None,
                    fast_mode_reason: None,
                    mcp_servers: Vec::new(),
                }]
            })
            .unwrap_or_default(),
        "message" if v["role"] == "assistant" => text(&v, &["text"])
            .map(|t| vec![EntryKind::AssistantMessage { text: t }])
            .unwrap_or_default(),
        "reasoning" => {
            let Some(t) = text(&v, &["text"]) else {
                return Vec::new();
            };
            let key = (text(&v, &["id"]).unwrap_or_default(), t.clone());
            if turn.thought.as_ref() == Some(&key) {
                return Vec::new();
            }
            turn.thought = Some(key);
            vec![EntryKind::Thinking { text: t }]
        }
        "tool_call" => {
            let Some(id) = call_id(&v) else {
                return Vec::new();
            };
            let (name, input) = tool(
                v.get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                v.get("parameters").unwrap_or(&Value::Null),
            );
            turn.calls.push_back(id.clone());
            vec![EntryKind::ToolUse {
                id: ToolId(id),
                name,
                input,
            }]
        }
        "tool_result" => {
            let named = call_id(&v);
            let id = match named
                .as_ref()
                .and_then(|n| turn.calls.iter().position(|c| c == n))
            {
                Some(at) => turn.calls.remove(at),
                None => turn.calls.pop_front().or(named),
            };
            let Some(id) = id else {
                return Vec::new();
            };
            let failed = v["isError"].as_bool().unwrap_or(false)
                || v.get("error").is_some_and(|e| !e.is_null());
            vec![EntryKind::ToolResult {
                id: ToolId(id),
                ok: !failed,
                content: result_content(&v),
                structured: v
                    .get("value")
                    .filter(|x| x.is_object() || x.is_array())
                    .cloned(),
                images: Vec::new(),
            }]
        }
        "error" => {
            let message =
                text(&v, &["message"]).unwrap_or_else(|| "Droid stopped with an error".to_owned());
            if v["source"] == "cli" {
                vec![EntryKind::Finished(Outcome::Failed {
                    message: turn.reason.take().unwrap_or(message),
                })]
            } else {
                turn.reason = Some(message.clone());
                vec![EntryKind::Error { message }]
            }
        }
        "completion" => {
            turn.calls.clear();
            turn.reason = None;
            turn.thought = None;
            let used = usage(&v);
            let mut out: Vec<EntryKind> = used
                .clone()
                .map(EntryKind::TokenUsage)
                .into_iter()
                .collect();
            out.push(EntryKind::Finished(Outcome::Success {
                text: text(&v, &["finalText"]),
                usage: used,
                denied: Vec::new(),
                denials: Vec::new(),
            }));
            out
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "droid_tests.rs"]
mod tests;
