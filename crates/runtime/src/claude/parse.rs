use blazar_core_types::{
    EntryKind, Outcome, Provider, ProviderSessionId, RateLimit, RateLimitWindow, TokenUsage, ToolId,
};
use chrono::{DateTime, Utc};
use serde_json::Value;

pub struct ParsedLine {
    pub entries: Vec<EntryKind>,

    pub parent_tool_use_id: Option<ToolId>,
}

#[must_use]
pub fn parse_line(line: &str) -> Option<ParsedLine> {
    let value: Value = serde_json::from_str(line).ok()?;
    let parent_tool_use_id = value
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .map(|s| ToolId(s.to_owned()));

    let entries = match value.get("type").and_then(Value::as_str)? {
        "rate_limit_event" => parse_rate_limit(&value),
        "system" => match value.get("subtype").and_then(Value::as_str) {
            Some("init") => parse_init(&value),

            Some(st @ ("task_started" | "task_updated" | "task_notification")) => {
                parse_task(st, &value).into_iter().collect()
            }
            _ => Vec::new(),
        },
        "assistant" => parse_assistant(&value),
        "user" => parse_user(&value),

        "control_request" => parse_control_request(&value),
        "control_cancel_request" => parse_control_cancel(&value),

        "control_response" => Vec::new(),
        "result" => parse_result(&value),
        _ => Vec::new(),
    };

    Some(ParsedLine {
        entries,
        parent_tool_use_id,
    })
}

fn parse_init(v: &Value) -> Vec<EntryKind> {
    let Some(session_id) = v.get("session_id").and_then(Value::as_str) else {
        return Vec::new();
    };
    vec![EntryKind::SessionStarted {
        provider_session_id: ProviderSessionId(session_id.to_owned()),
        model: string_field(v, "model"),
        cwd: string_field(v, "cwd"),
        permission_mode: string_field(v, "permissionMode"),
        output_style: string_field(v, "output_style"),
        fast_mode: string_field(v, "fast_mode_state"),
        fast_mode_reason: string_field(v, "fast_mode_disabled_reason"),
        mcp_servers: blazar_core_types::McpServerStatus::list_from(v.get("mcp_servers")),
    }]
}

fn parse_rate_limit(v: &Value) -> Vec<EntryKind> {
    let Some(info) = v.get("rate_limit_info") else {
        return Vec::new();
    };

    let windows = info
        .get("unifiedWindows")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(name, w)| RateLimitWindow {
                    name: name.clone(),
                    utilization: w.get("utilization").and_then(Value::as_f64).unwrap_or(0.0),
                    resets_at: w.get("resetsAt").and_then(unix_seconds),
                })
                .collect()
        })
        .unwrap_or_default();

    vec![EntryKind::RateLimit(RateLimit {
        provider: Provider::Anthropic,

        allowed: match info.get("status").and_then(Value::as_str) {
            None => true,
            Some(st) => st.starts_with("allowed"),
        },
        windows,
    })]
}

fn parse_assistant(v: &Value) -> Vec<EntryKind> {
    let Some(message) = v.get("message") else {
        return Vec::new();
    };

    if let Some(kind) = v.get("error").and_then(Value::as_str) {
        let text: Vec<String> = content_blocks(message)
            .filter_map(|b| non_empty(b.get("text")))
            .collect();
        return vec![EntryKind::Error {
            message: if text.is_empty() {
                kind.to_owned()
            } else {
                format!("{kind}：{}", text.join(" "))
            },
        }];
    }
    let mut out = Vec::new();

    for block in content_blocks(message) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = non_empty(block.get("text")) {
                    out.push(EntryKind::AssistantMessage { text });
                }
            }
            Some("thinking") => {
                out.push(EntryKind::Thinking {
                    text: non_empty(block.get("thinking")).unwrap_or_default(),
                });
            }
            Some("tool_use") => {
                if let Some(id) = block.get("id").and_then(Value::as_str) {
                    out.push(EntryKind::ToolUse {
                        id: ToolId(id.to_owned()),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        input: block.get("input").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            _ => {}
        }
    }

    out
}

fn parse_user(v: &Value) -> Vec<EntryKind> {
    let Some(message) = v.get("message") else {
        return Vec::new();
    };

    if v.get("isReplay").and_then(Value::as_bool).unwrap_or(false) {
        let text = match message.get("content") {
            Some(Value::String(s)) => s.clone(),
            _ => content_blocks(message)
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        return vec![EntryKind::InputConsumed { text }];
    }

    let structured = v.get("tool_use_result").cloned();
    let mut out = Vec::new();

    for block in content_blocks(message) {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
            continue;
        };
        out.push(EntryKind::ToolResult {
            id: ToolId(id.to_owned()),
            ok: !block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            content: flatten_result_content(block.get("content")),
            structured: structured.clone(),
        });
    }
    out
}

fn parse_result(v: &Value) -> Vec<EntryKind> {
    let usage = v.get("usage").and_then(parse_usage).map(|mut u| {
        u.cost_usd = v.get("total_cost_usd").and_then(Value::as_f64);
        u
    });

    let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");

    let failed = v.get("is_error").and_then(Value::as_bool).unwrap_or(false)
        || subtype.starts_with("error_");
    let outcome = if failed {
        let errors: Vec<String> = v
            .get("errors")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let reason = v.get("terminal_reason").and_then(Value::as_str);
        let base = if errors.is_empty() {
            string_field(v, "result")
                .or_else(|| {
                    (!subtype.is_empty() && subtype != "success").then(|| subtype.to_owned())
                })
                .unwrap_or_else(|| "agent 以错误状态结束".to_owned())
        } else {
            errors.join("；")
        };
        Outcome::Failed {
            message: match reason {
                Some(r) if !base.contains(r) => format!("{base}（{r}）"),
                _ => base,
            },
        }
    } else {
        Outcome::Success {
            text: string_field(v, "result"),
            usage: usage.clone(),
            denied: permission_denials(v),
        }
    };

    let mut out = Vec::new();
    if let Some(usage) = usage {
        out.push(EntryKind::TokenUsage(usage));
    }
    out.push(EntryKind::Finished(outcome));
    out
}

fn parse_control_request(v: &Value) -> Vec<EntryKind> {
    let Some(req) = v.get("request") else {
        return Vec::new();
    };
    if req.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
        return Vec::new();
    }
    let Some(rid) = v.get("request_id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let pick = |k: &str| req.get(k).cloned().unwrap_or(Value::Null);
    vec![EntryKind::Approval {
        id: blazar_core_types::ApprovalId::from_provider(rid),
        request: serde_json::json!({
            "provider_request_id": rid,
            "tool_name": pick("tool_name"),
            "display_name": pick("display_name"),
            "input": pick("input"),
            "description": pick("description"),
            "blocked_path": pick("blocked_path"),
            "tool_use_id": pick("tool_use_id"),
            "permission_suggestions": pick("permission_suggestions"),
        }),
    }]
}

fn parse_control_cancel(v: &Value) -> Vec<EntryKind> {
    v.get("request_id")
        .and_then(Value::as_str)
        .map(|rid| {
            vec![EntryKind::ApprovalResolved {
                id: blazar_core_types::ApprovalId::from_provider(rid),
                decision: blazar_core_types::ApprovalDecision::Cancelled,
            }]
        })
        .unwrap_or_default()
}

fn permission_denials(v: &Value) -> Vec<String> {
    v.get("permission_denials")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|d| {
                    let tool = d.get("tool_name").and_then(Value::as_str).unwrap_or("?");
                    let input = d.get("tool_input");

                    let what = ["command", "file_path", "path", "url", "pattern"]
                        .iter()
                        .find_map(|k| input?.get(*k)?.as_str())
                        .map(str::to_owned)
                        .unwrap_or_default();
                    if what.is_empty() {
                        tool.to_owned()
                    } else {
                        format!("{tool}: {what}")
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_task(subtype: &str, v: &Value) -> Option<EntryKind> {
    let task_id = v.get("task_id")?.as_str()?.to_owned();
    let desc = || string_field(v, "description");
    let ty = || string_field(v, "task_type");
    let status = match subtype {
        "task_started" => {
            if !v
                .get("is_backgrounded")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            "started".to_owned()
        }
        "task_updated" => v.get("patch")?.get("status")?.as_str()?.to_owned(),
        "task_notification" => v.get("status")?.as_str()?.to_owned(),
        _ => return None,
    };
    Some(EntryKind::BackgroundTask {
        task_id,
        status,
        description: desc(),
        task_type: ty(),
    })
}

fn content_blocks(message: &Value) -> impl Iterator<Item = &Value> {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
}

fn parse_usage(v: &Value) -> Option<TokenUsage> {
    let u = |key: &str| v.get(key).and_then(Value::as_u64).unwrap_or(0);
    Some(TokenUsage {
        input: u("input_tokens"),
        output: u("output_tokens"),
        cache_read: u("cache_read_input_tokens"),
        cache_creation: u("cache_creation_input_tokens"),
        cost_usd: None,
    })
}

fn flatten_result_content(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn string_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn non_empty(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn unix_seconds(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(v.as_i64()?, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE_LIMIT: &str = r#"{"type":"rate_limit_event","session_id":"s","uuid":"u",
        "rate_limit_info":{"status":"allowed","resetsAt":1789664400,"rateLimitType":"five_hour",
        "overageStatus":"rejected","isUsingOverage":false,
        "unifiedWindows":{"five_hour":{"utilization":0.06,"resetsAt":1789664400},
                          "seven_day":{"utilization":0.6,"resetsAt":1789765200}}}}"#;

    #[test]
    fn rate_limit_yields_both_windows() {
        let parsed = parse_line(RATE_LIMIT).expect("应能解析");
        let [EntryKind::RateLimit(rl)] = parsed.entries.as_slice() else {
            panic!("期望单条 RateLimit，实得 {:?}", parsed.entries);
        };
        assert!(rl.allowed);
        assert_eq!(rl.windows.len(), 2);
        let seven = rl.windows.iter().find(|w| w.name == "seven_day").unwrap();
        assert!((seven.utilization - 0.6).abs() < f64::EPSILON);
        assert!(seven.resets_at.is_some());
    }

    #[test]
    fn init_carries_session_id_for_resume() {
        let line = r#"{"type":"system","subtype":"init","session_id":"938707cf","model":"claude-opus-5",
            "cwd":"/tmp/x","permissionMode":"acceptEdits"}"#;
        let parsed = parse_line(line).unwrap();
        let [
            EntryKind::SessionStarted {
                provider_session_id,
                model,
                ..
            },
        ] = parsed.entries.as_slice()
        else {
            panic!("期望 SessionStarted");
        };
        assert_eq!(provider_session_id.0, "938707cf");
        assert_eq!(model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn assistant_line_does_not_double_count_usage() {
        let line = r#"{"type":"assistant","session_id":"s","message":{"role":"assistant",
            "content":[{"type":"tool_use","id":"toolu_01","name":"Read",
                        "input":{"file_path":"/tmp/a.txt"}}],
            "usage":{"input_tokens":2,"output_tokens":1,
                     "cache_read_input_tokens":10126,"cache_creation_input_tokens":14095}}}"#;
        let parsed = parse_line(line).unwrap();

        assert_eq!(parsed.entries.len(), 1, "assistant 行只该产出 ToolUse");
        let EntryKind::ToolUse { id, name, .. } = &parsed.entries[0] else {
            panic!("应为 ToolUse");
        };
        assert_eq!(id.0, "toolu_01");
        assert_eq!(name, "Read");
        assert!(
            !parsed
                .entries
                .iter()
                .any(|e| matches!(e, EntryKind::TokenUsage(_))),
            "assistant 行不能再发 TokenUsage"
        );
    }

    #[test]
    fn tool_result_keeps_structured_payload() {
        let line = r#"{"type":"user","session_id":"s","message":{"role":"user",
            "content":[{"type":"tool_result","tool_use_id":"toolu_01","content":"1\thello"}]},
            "tool_use_result":{"type":"text","file":{"filePath":"/tmp/a.txt","numLines":2}}}"#;
        let parsed = parse_line(line).unwrap();
        let [
            EntryKind::ToolResult {
                id, ok, structured, ..
            },
        ] = parsed.entries.as_slice()
        else {
            panic!("期望 ToolResult");
        };
        assert_eq!(id.0, "toolu_01");
        assert!(*ok);
        assert!(structured.is_some(), "结构化结果应保留用于富渲染");
    }

    #[test]
    fn result_success_carries_cost() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,
            "stop_reason":"end_turn","result":"done","total_cost_usd":0.1839295,
            "usage":{"input_tokens":4,"output_tokens":140}}"#;
        let parsed = parse_line(line).unwrap();
        let EntryKind::TokenUsage(u) = &parsed.entries[0] else {
            panic!("应先产出 TokenUsage");
        };
        assert_eq!(u.cost_usd, Some(0.1839295));
        assert!(matches!(
            parsed.entries[1],
            EntryKind::Finished(Outcome::Success { .. })
        ));
    }

    #[test]
    fn subagent_lines_keep_parent_tool_id() {
        let line = r#"{"type":"assistant","parent_tool_use_id":"toolu_parent",
            "message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
        let parsed = parse_line(line).unwrap();
        assert_eq!(
            parsed.parent_tool_use_id.as_ref().map(|t| t.0.as_str()),
            Some("toolu_parent")
        );
    }

    #[test]
    fn unknown_event_types_are_ignored_not_fatal() {
        let parsed = parse_line(r#"{"type":"some_future_event","payload":1}"#).unwrap();
        assert!(parsed.entries.is_empty());
    }

    #[test]
    fn malformed_json_is_skipped() {
        assert!(parse_line("not json at all").is_none());
    }

    #[test]
    fn a_whole_turn_reports_usage_exactly_once() {
        let lines = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"a"}],
                "usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":10126}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"b"}],
                "usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":10126}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"c"}],
                "usage":{"input_tokens":2,"output_tokens":2,"cache_read_input_tokens":24221}}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done",
                "total_cost_usd":0.1839295,
                "usage":{"input_tokens":4,"output_tokens":140,"cache_read_input_tokens":34347}}"#,
        ];
        let usages: Vec<TokenUsage> = lines
            .iter()
            .filter_map(|l| parse_line(l))
            .flat_map(|p| p.entries)
            .filter_map(|e| match e {
                EntryKind::TokenUsage(u) => Some(u),
                _ => None,
            })
            .collect();
        assert_eq!(usages.len(), 1, "一轮只能报一次用量，报多次就是重复计费");
        assert_eq!(usages[0].output, 140, "要取 result 的权威值而不是快照累加");
        assert_eq!(usages[0].cache_read, 34347);
        assert_eq!(usages[0].cost_usd, Some(0.1839295));
    }

    #[test]
    fn not_logged_in_is_an_error_not_something_claude_said() {
        let a = r#"{"type":"assistant","error":"authentication_failed","message":{"content":[{"type":"text","text":"Not logged in · Please run /login"}]}}"#;
        let p = parse_line(a).unwrap();
        assert!(
            matches!(&p.entries[..], [EntryKind::Error { message }] if message.contains("authentication_failed") && message.contains("Not logged in")),
            "{:?}",
            p.entries
        );
        let r = r#"{"type":"result","subtype":"success","is_error":true,"terminal_reason":"api_error","result":"Not logged in · Please run /login"}"#;
        let p = parse_line(r).unwrap();
        assert!(
            p.entries.iter().any(|e| matches!(e, EntryKind::Finished(Outcome::Failed { message }) if message.contains("Not logged in") && message.contains("api_error"))),
            "{:?}", p.entries
        );
        assert_eq!(
            blazar_core_types::FailureClass::from_claude(Some("authentication_failed"), None),
            Some(blazar_core_types::FailureClass::AuthExpired),
            "没登录要归成凭据问题，而不是 agent 自己的错"
        );
    }

    #[test]
    fn denied_tool_calls_do_not_read_as_success() {
        let r = r#"{"type":"result","subtype":"success","is_error":false,"result":"好的",
            "permission_denials":[{"tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"touch made_it.txt"}}]}"#;
        let p = parse_line(r).unwrap();
        let Some(EntryKind::Finished(Outcome::Success { denied, .. })) = p.entries.last() else {
            panic!("{:?}", p.entries)
        };
        assert_eq!(denied, &vec!["Bash: touch made_it.txt".to_owned()]);
        assert_eq!(
            blazar_core_types::ActivityState::from_entry(p.entries.last().unwrap()),
            Some(blazar_core_types::ActivityState::Errored),
            "有操作被拦下的一轮不能显示成已完成"
        );
    }

    #[test]
    fn error_subtypes_fail_even_without_is_error() {
        let r = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":false}"#;
        let p = parse_line(r).unwrap();
        assert!(
            p.entries.iter().any(|e| matches!(e, EntryKind::Finished(Outcome::Failed { message }) if message.contains("error_max_budget_usd"))),
            "{:?}", p.entries
        );
    }

    #[test]
    fn background_task_killed_after_the_turn_is_surfaced() {
        let lines = [
            r#"{"type":"system","subtype":"task_started","task_id":"b3f","is_backgrounded":true,"task_type":"local_bash","description":"跑训练"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"已启动后台任务"}"#,
            r#"{"type":"system","subtype":"task_updated","task_id":"b3f","patch":{"status":"killed"}}"#,
            r#"{"type":"system","subtype":"task_notification","task_id":"b3f","status":"stopped"}"#,
        ];
        let kinds: Vec<EntryKind> = lines
            .iter()
            .filter_map(|l| parse_line(l))
            .flat_map(|p| p.entries)
            .collect();
        let last = kinds.last().unwrap();
        assert!(matches!(last, EntryKind::BackgroundTask { status, .. } if status == "stopped"));
        assert_eq!(
            blazar_core_types::ActivityState::from_entry(&kinds[2]),
            Some(blazar_core_types::ActivityState::Errored),
            "被杀的后台任务要把工作区状态拉回「需要你看」"
        );

        let fg =
            r#"{"type":"system","subtype":"task_started","task_id":"x","is_backgrounded":false}"#;
        assert!(parse_line(fg).unwrap().entries.is_empty());
    }

    #[test]
    fn rejected_rate_limit_is_not_reported_as_allowed() {
        let line = |st: &str| {
            format!(
                r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{st}","unifiedWindows":{{"five_hour":{{"utilization":1.0}}}}}}}}"#
            )
        };
        let allowed = |st: &str| match &parse_line(&line(st)).unwrap().entries[0] {
            EntryKind::RateLimit(r) => r.allowed,
            e => panic!("{e:?}"),
        };
        assert!(allowed("allowed"));
        assert!(allowed("allowed_warning"), "接近上限但仍可用");
        assert!(!allowed("rejected"), "真被限流时不能报告还能用");
    }

    #[test]
    fn permission_request_becomes_an_approval_with_a_stable_id() {
        let l = r#"{"type":"control_request","request_id":"707e1df4-c69c","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"touch marker.txt","description":"Create a marker file"},"description":"Create a marker file","permission_suggestions":[{"type":"setMode","mode":"acceptEdits"}],"blocked_path":"/w/marker.txt","tool_use_id":"toolu_01"}}"#;
        let p = parse_line(l).unwrap();
        let [EntryKind::Approval { id, request }] = &p.entries[..] else {
            panic!("{:?}", p.entries)
        };
        assert_eq!(
            *id,
            blazar_core_types::ApprovalId::from_provider("707e1df4-c69c")
        );
        assert_eq!(request["tool_name"], "Bash");
        assert_eq!(request["input"]["command"], "touch marker.txt");
        assert_eq!(request["provider_request_id"], "707e1df4-c69c");

        let c = parse_line(r#"{"type":"control_cancel_request","request_id":"707e1df4-c69c"}"#)
            .unwrap();
        assert!(
            matches!(&c.entries[..], [EntryKind::ApprovalResolved { id: cid, decision: blazar_core_types::ApprovalDecision::Cancelled }] if cid == id)
        );
    }

    #[test]
    fn replayed_user_input_is_a_receipt_not_a_second_message() {
        let arr = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"别动那个文件"}]},"isReplay":true}"#;
        let p = parse_line(arr).unwrap();
        assert!(
            matches!(&p.entries[..], [EntryKind::InputConsumed { text }] if text == "别动那个文件")
        );
        let s =
            r#"{"type":"user","message":{"role":"user","content":"纯字符串形式"},"isReplay":true}"#;
        assert!(
            matches!(&parse_line(s).unwrap().entries[..], [EntryKind::InputConsumed { text }] if text == "纯字符串形式")
        );

        assert!(
            parse_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"x"}}"#
            )
            .unwrap()
            .entries
            .is_empty()
        );

        assert!(
            parse_line(
                r#"{"type":"control_request","request_id":"y","request":{"subtype":"interrupt"}}"#
            )
            .unwrap()
            .entries
            .is_empty()
        );
    }
}
