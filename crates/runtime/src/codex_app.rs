use blazar_core_types::{
    ApprovalDecision, ApprovalId, EntryKind, Outcome, Provider, ProviderSessionId, RateLimit,
    RateLimitWindow, TokenUsage, ToolId,
};
use serde_json::{Value, json};

use crate::{ImageInput, SessionSpec};

pub const INIT_ID: &str = "blazar-init";
pub const THREAD_REQ: &str = "blazar-thread";
pub const TURN_REQ: &str = "blazar-turn";

pub const SANDBOXES: &[&str] = &["read-only", "workspace-write", "danger-full-access"];

pub fn launch_args(extra: &[String]) -> (Vec<String>, Option<String>) {
    let mut out = vec!["app-server".to_owned()];
    let mut sandbox = None;
    let mut it = extra.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-c" | "--config" => {
                if let Some(v) = it.next() {
                    out.push("-c".into());
                    out.push(v.clone());
                }
            }
            "--disable" => {
                if let Some(f) = it.next() {
                    out.push("-c".into());
                    out.push(format!("features.{f}=false"));
                }
            }
            "--enable" => {
                if let Some(f) = it.next() {
                    out.push("-c".into());
                    out.push(format!("features.{f}=true"));
                }
            }
            "-s" | "--sandbox" => sandbox = it.next().cloned(),
            _ => {}
        }
    }
    (out, sandbox)
}

fn approval_policy(sandbox: &str) -> &'static str {
    if sandbox == "danger-full-access" {
        "never"
    } else {
        "on-request"
    }
}

fn line(id: &str, v: &Value) -> String {
    format!("{id}\t{v}")
}

fn request(id: &str, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub fn user_input(text: &str, images: &[ImageInput]) -> Vec<Value> {
    let mut out: Vec<Value> = images
        .iter()
        .map(|i| {
            json!({
                "type": "image",
                "url": format!("data:{};base64,{}", i.media_type, i.data),
            })
        })
        .collect();
    out.push(json!({ "type": "text", "text": text, "text_elements": [] }));
    out
}

pub fn first_input(session: &SessionSpec, resume: Option<&str>, sandbox: Option<&str>) -> String {
    let sandbox = sandbox
        .filter(|s| SANDBOXES.contains(s))
        .unwrap_or("workspace-write");
    let cwd = session.cwd.display().to_string();
    let init = request(
        INIT_ID,
        "initialize",
        json!({
            "clientInfo": { "name": "blazar", "title": "Blazar", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": null,
        }),
    );
    let initialized = json!({ "jsonrpc": "2.0", "method": "initialized" });
    let mut params = json!({
        "cwd": cwd,
        "approvalPolicy": approval_policy(sandbox),
        "sandbox": sandbox,
    });
    if let Some(m) = session.model.as_deref().filter(|m| !m.trim().is_empty()) {
        params["model"] = json!(m);
    }
    let thread = match resume {
        Some(id) => {
            params["threadId"] = json!(id);
            params["excludeTurns"] = json!(true);
            request(THREAD_REQ, "thread/resume", params)
        }
        None => request(THREAD_REQ, "thread/start", params),
    };
    [
        line("init", &init),
        line("initialized", &initialized),
        line("thread", &thread),
    ]
    .join("\n")
}

pub fn turn_start(thread: &str, text: &str, images: &[ImageInput], client_id: &str) -> String {
    request(
        TURN_REQ,
        "turn/start",
        json!({ "threadId": thread, "input": user_input(text, images), "clientUserMessageId": client_id }),
    )
    .to_string()
}

pub fn steer(
    thread: &str,
    turn: &str,
    text: &str,
    images: &[ImageInput],
    client_id: &str,
) -> String {
    request(
        &format!("blazar-steer-{client_id}"),
        "turn/steer",
        json!({
            "threadId": thread,
            "expectedTurnId": turn,
            "input": user_input(text, images),
            "clientUserMessageId": client_id,
        }),
    )
    .to_string()
}

pub fn interrupt(thread: &str, turn: &str, req_id: &str) -> String {
    request(
        req_id,
        "turn/interrupt",
        json!({ "threadId": thread, "turnId": turn }),
    )
    .to_string()
}

pub fn compact(thread: &str, req_id: &str) -> String {
    request(
        req_id,
        "thread/compact/start",
        json!({ "threadId": thread }),
    )
    .to_string()
}

pub fn review(thread: &str, instructions: &str) -> String {
    let target = if instructions.trim().is_empty() {
        json!({ "type": "uncommittedChanges" })
    } else {
        json!({ "type": "custom", "instructions": instructions.trim() })
    };
    request(
        TURN_REQ,
        "review/start",
        json!({ "threadId": thread, "target": target, "delivery": "inline" }),
    )
    .to_string()
}

pub fn first_turn(thread: &str, text: &str, images: &[ImageInput], client_id: &str) -> String {
    let t = text.trim();
    if t == "/compact" {
        return compact(thread, "blazar-compact");
    }
    if let Some(rest) = t.strip_prefix("/review")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        return review(thread, rest);
    }
    turn_start(thread, text, images, client_id)
}

const APPROVAL_METHODS: &[&str] = &[
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "execCommandApproval",
    "applyPatchApproval",
];

pub fn approval_reply(raw_request: &str, allow: bool) -> Option<String> {
    let v: Value = serde_json::from_str(raw_request).ok()?;
    let method = v.get("method").and_then(Value::as_str)?;
    if !APPROVAL_METHODS.contains(&method) {
        return None;
    }
    let id = v.get("id")?.clone();
    let decision = match (method, allow) {
        ("execCommandApproval" | "applyPatchApproval", true) => "approved",
        ("execCommandApproval" | "applyPatchApproval", false) => "denied",
        (_, true) => "accept",
        (_, false) => "decline",
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "decision": decision } }).to_string())
}

pub fn request_id_text(v: &Value) -> Option<String> {
    match v.get("id")? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ids {
    pub thread: Option<String>,
    pub turn: Option<String>,
    pub turn_ended: bool,
}

pub fn ids(line: &str) -> Ids {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Ids::default();
    };
    let mut out = Ids::default();
    if v.get("id").and_then(Value::as_str) == Some(THREAD_REQ) {
        out.thread = v["result"]["thread"]["id"].as_str().map(str::to_owned);
    }
    if v.get("id").and_then(Value::as_str) == Some(TURN_REQ) {
        out.turn = v["result"]["turn"]["id"].as_str().map(str::to_owned);
    }
    match v.get("method").and_then(Value::as_str) {
        Some("turn/started") => out.turn = v["params"]["turn"]["id"].as_str().map(str::to_owned),
        Some("turn/completed") => out.turn_ended = true,
        _ => {}
    }
    out
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
}

fn window_name(mins: Option<i64>) -> String {
    match mins {
        Some(300) => "five_hour".into(),
        Some(10080) => "seven_day".into(),
        Some(m) if m % 1440 == 0 => format!("{}_day", m / 1440),
        Some(m) if m % 60 == 0 => format!("{}_hour", m / 60),
        Some(m) => format!("{m}_min"),
        None => "codex".into(),
    }
}

fn rate_limits(v: &Value) -> Option<EntryKind> {
    let rl = v.get("rateLimits")?;
    let windows: Vec<RateLimitWindow> = ["primary", "secondary"]
        .iter()
        .filter_map(|k| rl.get(*k).filter(|w| !w.is_null()))
        .map(|w| RateLimitWindow {
            name: window_name(w.get("windowDurationMins").and_then(Value::as_i64)),
            utilization: w.get("usedPercent").and_then(Value::as_f64).unwrap_or(0.0) / 100.0,
            resets_at: w
                .get("resetsAt")
                .and_then(Value::as_i64)
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
        })
        .collect();
    Some(EntryKind::RateLimit(RateLimit {
        provider: Provider::OpenAi,
        allowed: rl.get("rateLimitReachedType").is_none_or(Value::is_null),
        windows,
    }))
}

fn usage(v: &Value) -> Option<TokenUsage> {
    let u = v.get("tokenUsage")?.get("last")?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    Some(TokenUsage {
        input: n("inputTokens"),
        output: n("outputTokens") + n("reasoningOutputTokens"),
        cache_read: n("cachedInputTokens"),
        cache_creation: n("cacheWriteInputTokens"),
        cost_usd: None,
    })
}

fn tool_id(item: &Value) -> ToolId {
    ToolId(s(item, "id").unwrap_or_else(|| "item".to_owned()))
}

fn file_changes(item: &Value) -> Value {
    item.get("changes").cloned().unwrap_or(Value::Null)
}

fn item_started(item: &Value) -> Vec<EntryKind> {
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "commandExecution" => vec![EntryKind::ToolUse {
            id: tool_id(item),
            name: "Shell".to_owned(),
            input: json!({ "command": s(item, "command").unwrap_or_default() }),
        }],
        "fileChange" => vec![EntryKind::ToolUse {
            id: tool_id(item),
            name: "Edit".to_owned(),
            input: file_changes(item),
        }],
        "mcpToolCall" | "dynamicToolCall" => vec![EntryKind::ToolUse {
            id: tool_id(item),
            name: s(item, "tool").unwrap_or_else(|| "MCP".to_owned()),
            input: item.get("arguments").cloned().unwrap_or(Value::Null),
        }],
        "webSearch" => vec![EntryKind::ToolUse {
            id: tool_id(item),
            name: "WebSearch".to_owned(),
            input: json!({ "query": s(item, "query").unwrap_or_default() }),
        }],
        _ => Vec::new(),
    }
}

fn mcp_text(item: &Value) -> String {
    item.get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .or_else(|| {
            item.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn item_completed(item: &Value) -> Vec<EntryKind> {
    let status = s(item, "status").unwrap_or_default();
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "agentMessage" => s(item, "text")
            .map(|text| vec![EntryKind::AssistantMessage { text }])
            .unwrap_or_default(),
        "reasoning" => {
            let pick = |k: &str| {
                item.get(k)
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|t| !t.trim().is_empty())
            };
            pick("summary")
                .or_else(|| pick("content"))
                .map(|text| vec![EntryKind::Thinking { text }])
                .unwrap_or_default()
        }
        "plan" => s(item, "text")
            .map(|text| vec![EntryKind::Thinking { text }])
            .unwrap_or_default(),
        "userMessage" => {
            let text = item
                .get("content")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            vec![EntryKind::InputConsumed { text }]
        }
        "commandExecution" => vec![EntryKind::ToolResult {
            id: tool_id(item),
            ok: status == "completed"
                && item
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .is_none_or(|c| c == 0),
            content: s(item, "aggregatedOutput").unwrap_or_else(|| {
                if status == "declined" {
                    "Declined".to_owned()
                } else {
                    String::new()
                }
            }),
            structured: None,
        }],
        "fileChange" => vec![EntryKind::ToolResult {
            id: tool_id(item),
            ok: status == "completed",
            content: item
                .get("changes")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter_map(|x| x.get("path").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            structured: None,
        }],
        "mcpToolCall" | "dynamicToolCall" => vec![EntryKind::ToolResult {
            id: tool_id(item),
            ok: status == "completed",
            content: mcp_text(item),
            structured: None,
        }],
        "webSearch" => vec![EntryKind::ToolResult {
            id: tool_id(item),
            ok: true,
            content: String::new(),
            structured: None,
        }],
        "contextCompaction" => vec![EntryKind::Notice {
            message: "Context compacted".to_owned(),
        }],
        _ => Vec::new(),
    }
}

fn approval(v: &Value, method: &str) -> Vec<EntryKind> {
    let Some(rid) = request_id_text(v) else {
        return Vec::new();
    };
    let p = &v["params"];
    let (tool, input) = if method.contains("fileChange") || method == "applyPatchApproval" {
        (
            "Edit",
            json!({ "changes": p.get("changes").or_else(|| p.get("fileChanges")).cloned().unwrap_or(Value::Null), "grant_root": p.get("grantRoot").cloned().unwrap_or(Value::Null) }),
        )
    } else {
        let command = match p.get("command") {
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            Some(Value::String(c)) => c.clone(),
            _ => String::new(),
        };
        (
            "Bash",
            json!({ "command": command, "cwd": p.get("cwd").cloned().unwrap_or(Value::Null) }),
        )
    };
    vec![EntryKind::Approval {
        id: ApprovalId::from_provider(&format!("codex:{rid}")),
        request: json!({
            "provider_request_id": rid,
            "tool_name": tool,
            "display_name": tool,
            "input": input,
            "description": p.get("reason").cloned().unwrap_or(Value::Null),
            "tool_use_id": p.get("itemId").cloned().unwrap_or(Value::Null),
        }),
    }]
}

fn turn_error(turn: &Value) -> String {
    turn.get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("Turn failed")
        .to_owned()
}

pub fn parse_line(line: &str) -> Vec<EntryKind> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    if let Some(id) = v.get("id").and_then(Value::as_str)
        && v.get("method").is_none()
    {
        if let Some(err) = v.get("error") {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex returned an error")
                .to_owned();
            return if id == THREAD_REQ || id == TURN_REQ || id == INIT_ID {
                vec![EntryKind::Finished(Outcome::Failed { message })]
            } else {
                vec![EntryKind::Notice { message }]
            };
        }
        if id == THREAD_REQ {
            let r = &v["result"];
            return r["thread"]["id"]
                .as_str()
                .map(|tid| {
                    vec![EntryKind::SessionStarted {
                        provider_session_id: ProviderSessionId(tid.to_owned()),
                        model: s(r, "model"),
                        cwd: s(r, "cwd"),
                        permission_mode: r
                            .get("sandbox")
                            .and_then(|x| x.get("type"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        output_style: None,
                        fast_mode: None,
                        fast_mode_reason: None,
                        mcp_servers: Vec::new(),
                    }]
                })
                .unwrap_or_default();
        }
        return Vec::new();
    }
    let method = v.get("method").and_then(Value::as_str).unwrap_or_default();
    let p = &v["params"];
    if v.get("id").is_some() && APPROVAL_METHODS.contains(&method) {
        return approval(&v, method);
    }
    match method {
        "item/started" => item_started(&p["item"]),
        "item/completed" => item_completed(&p["item"]),
        "thread/tokenUsage/updated" => usage(p).map(EntryKind::TokenUsage).into_iter().collect(),
        "account/rateLimits/updated" => rate_limits(p).into_iter().collect(),
        "serverRequest/resolved" => {
            request_id_text(&json!({ "id": p.get("requestId").cloned().unwrap_or(Value::Null) }))
                .map(|rid| {
                    vec![EntryKind::ApprovalResolved {
                        id: ApprovalId::from_provider(&format!("codex:{rid}")),
                        decision: ApprovalDecision::Cancelled,
                    }]
                })
                .unwrap_or_default()
        }
        "error" => {
            let message = p["error"]["message"]
                .as_str()
                .unwrap_or("Codex error")
                .to_owned();
            if p.get("willRetry").and_then(Value::as_bool) == Some(true) {
                vec![EntryKind::Notice { message }]
            } else {
                vec![EntryKind::Error { message }]
            }
        }
        "warning" | "configWarning" | "deprecationNotice" | "guardianWarning" => s(p, "message")
            .or_else(|| s(p, "summary"))
            .map(|message| vec![EntryKind::Notice { message }])
            .unwrap_or_default(),
        "model/rerouted" => vec![EntryKind::Notice {
            message: format!(
                "Model rerouted from {} to {}",
                s(p, "fromModel").unwrap_or_default(),
                s(p, "toModel").unwrap_or_default()
            ),
        }],
        "turn/completed" => {
            let turn = &p["turn"];
            match turn
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "interrupted" => vec![EntryKind::Finished(Outcome::Interrupted)],
                "failed" => vec![EntryKind::Finished(Outcome::Failed {
                    message: turn_error(turn),
                })],
                _ => vec![EntryKind::Finished(Outcome::Success {
                    text: None,
                    usage: None,
                    denied: Vec::new(),
                    denials: Vec::new(),
                })],
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<EntryKind> {
        parse_line(line)
    }

    #[test]
    fn exec_flags_become_app_server_config() {
        let extra: Vec<String> = [
            "--disable",
            "plugins",
            "-s",
            "read-only",
            "-c",
            "model_reasoning_effort=\"high\"",
            "--add-dir",
            "/x",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let (args, sandbox) = launch_args(&extra);
        assert_eq!(
            args,
            vec![
                "app-server",
                "-c",
                "features.plugins=false",
                "-c",
                "model_reasoning_effort=\"high\""
            ]
        );
        assert_eq!(sandbox.as_deref(), Some("read-only"));
    }

    #[test]
    fn first_input_starts_or_resumes_a_thread() {
        let mut spec = SessionSpec::new("/w", "hello");
        spec.model = Some("gpt-x".into());
        let fresh = first_input(&spec, None, Some("workspace-write"));
        let lines: Vec<&str> = fresh.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("init\t"));
        let t: Value = serde_json::from_str(lines[2].split_once('\t').unwrap().1).unwrap();
        assert_eq!(t["method"], "thread/start");
        assert_eq!(t["params"]["approvalPolicy"], "on-request");
        assert_eq!(t["params"]["model"], "gpt-x");
        let resumed = first_input(&spec, Some("thr-1"), Some("danger-full-access"));
        let t: Value =
            serde_json::from_str(resumed.lines().nth(2).unwrap().split_once('\t').unwrap().1)
                .unwrap();
        assert_eq!(t["method"], "thread/resume");
        assert_eq!(t["params"]["threadId"], "thr-1");
        assert_eq!(t["params"]["excludeTurns"], true);
        assert_eq!(t["params"]["approvalPolicy"], "never");
    }

    #[test]
    fn real_event_sequence_maps_to_entries() {
        let started = r#"{"id":"blazar-thread","result":{"thread":{"id":"01a1"},"model":"gpt-6","cwd":"/w"}}"#;
        assert!(
            matches!(&kinds(started)[..], [EntryKind::SessionStarted { provider_session_id, .. }] if provider_session_id.0 == "01a1")
        );
        assert_eq!(ids(started).thread.as_deref(), Some("01a1"));
        let turn = r#"{"method":"turn/started","params":{"threadId":"01a1","turn":{"id":"t1"}}}"#;
        assert_eq!(ids(turn).turn.as_deref(), Some("t1"));
        let msg = r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m","text":"OK"}}}"#;
        assert!(matches!(&kinds(msg)[..], [EntryKind::AssistantMessage { text }] if text == "OK"));
        let user = r#"{"method":"item/completed","params":{"item":{"type":"userMessage","id":"u","content":[{"type":"text","text":"hi"}]}}}"#;
        assert!(matches!(&kinds(user)[..], [EntryKind::InputConsumed { text }] if text == "hi"));
        let cmd_s = r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"c1","command":"ls","status":"inProgress"}}}"#;
        assert!(matches!(&kinds(cmd_s)[..], [EntryKind::ToolUse { name, .. }] if name == "Shell"));
        let cmd_d = r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"c1","command":"ls","status":"completed","exitCode":0,"aggregatedOutput":"a"}}}"#;
        assert!(
            matches!(&kinds(cmd_d)[..], [EntryKind::ToolResult { ok: true, content, .. }] if content == "a")
        );
        let usage = r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"last":{"inputTokens":10,"cachedInputTokens":4,"outputTokens":2,"reasoningOutputTokens":1,"cacheWriteInputTokens":0}}}}"#;
        assert!(
            matches!(&kinds(usage)[..], [EntryKind::TokenUsage(u)] if u.input == 10 && u.output == 3 && u.cache_read == 4)
        );
        let rl = r#"{"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":60,"windowDurationMins":10080,"resetsAt":1791948763},"secondary":null,"rateLimitReachedType":null}}}"#;
        assert!(
            matches!(&kinds(rl)[..], [EntryKind::RateLimit(r)] if r.allowed && r.windows[0].name == "seven_day" && (r.windows[0].utilization - 0.6).abs() < 1e-9)
        );
        let done =
            r#"{"method":"turn/completed","params":{"turn":{"id":"t1","status":"completed"}}}"#;
        assert!(matches!(
            &kinds(done)[..],
            [EntryKind::Finished(Outcome::Success { .. })]
        ));
        assert!(ids(done).turn_ended);
        let failed = r#"{"method":"turn/completed","params":{"turn":{"id":"t1","status":"failed","error":{"message":"额度用完"}}}}"#;
        assert!(
            matches!(&kinds(failed)[..], [EntryKind::Finished(Outcome::Failed { message })] if message == "额度用完")
        );
        let bad = r#"{"id":"blazar-thread","error":{"code":-32600,"message":"no such thread"}}"#;
        assert!(matches!(
            &kinds(bad)[..],
            [EntryKind::Finished(Outcome::Failed { .. })]
        ));
    }

    #[test]
    fn approvals_round_trip_with_the_original_request_id() {
        let req = r#"{"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"a","turnId":"t","itemId":"c1","command":"rm -rf build","cwd":"/w","reason":"needs write"}}"#;
        let [EntryKind::Approval { id, request }] = &kinds(req)[..] else {
            panic!()
        };
        assert_eq!(request["provider_request_id"], "7");
        assert_eq!(request["tool_name"], "Bash");
        assert_eq!(request["input"]["command"], "rm -rf build");
        assert_eq!(*id, ApprovalId::from_provider("codex:7"));
        let yes: Value = serde_json::from_str(&approval_reply(req, true).unwrap()).unwrap();
        assert_eq!(yes["id"], 7);
        assert_eq!(yes["result"]["decision"], "accept");
        let no: Value = serde_json::from_str(&approval_reply(req, false).unwrap()).unwrap();
        assert_eq!(no["result"]["decision"], "decline");
        let file = r#"{"jsonrpc":"2.0","id":"x9","method":"item/fileChange/requestApproval","params":{"itemId":"f1"}}"#;
        assert!(
            matches!(&kinds(file)[..], [EntryKind::Approval { request, .. }] if request["tool_name"] == "Edit")
        );
        assert!(approval_reply(r#"{"id":1,"method":"turn/start"}"#, true).is_none());
    }

    #[test]
    fn slash_commands_map_to_app_server_methods() {
        let c: Value = serde_json::from_str(&first_turn("thr", " /compact ", &[], "u-1")).unwrap();
        assert_eq!(c["method"], "thread/compact/start");
        let r: Value = serde_json::from_str(&first_turn("thr", "/review", &[], "u-1")).unwrap();
        assert_eq!(r["method"], "review/start");
        assert_eq!(r["params"]["target"]["type"], "uncommittedChanges");
        let r: Value =
            serde_json::from_str(&first_turn("thr", "/review focus on auth", &[], "u-1")).unwrap();
        assert_eq!(r["params"]["target"]["instructions"], "focus on auth");
        let t: Value =
            serde_json::from_str(&first_turn("thr", "/reviewer is here", &[], "u-1")).unwrap();
        assert_eq!(t["method"], "turn/start");
        let done = r#"{"method":"item/completed","params":{"item":{"type":"exitedReviewMode","id":"r","review":"Looks good"}}}"#;
        assert!(
            parse_line(done).is_empty(),
            "审查结果已经作为普通消息发过一次"
        );
    }

    #[test]
    fn steering_needs_the_active_turn() {
        let v: Value = serde_json::from_str(&steer("thr", "t1", "also do x", &[], "u-2")).unwrap();
        assert_eq!(v["method"], "turn/steer");
        assert_eq!(v["params"]["expectedTurnId"], "t1");
        assert_eq!(v["params"]["input"][0]["text"], "also do x");
        let i: Value = serde_json::from_str(&interrupt("thr", "t1", "i-1")).unwrap();
        assert_eq!(i["method"], "turn/interrupt");
    }
}
