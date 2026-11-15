use std::collections::HashMap;

use blazar_core_types::{
    ApprovalDecision, ApprovalId, EntryKind, Outcome, ProviderSessionId, TokenUsage, ToolId,
};
use serde_json::{Value, json};

use crate::{ImageInput, SessionSpec};

pub const INIT_ID: &str = "blazar-init";
pub const NEW_ID: &str = "blazar-new";
pub const LOAD_PREFIX: &str = "blazar-load:";
pub const PROMPT_PREFIX: &str = "blazar-prompt-";
pub const MODE_ID: &str = "blazar-mode";
pub const MODEL_ID: &str = "blazar-model";
pub const PERMISSION: &str = "session/request_permission";

#[must_use]
pub fn mode_for(mode: &str) -> Option<&'static str> {
    match mode {
        "ask" => Some("ask"),
        "default" | "acceptEdits" => Some("accept-edits"),
        "plan" => Some("plan"),
        "auto" => Some("smart"),
        "bypassPermissions" => Some("bypass"),
        _ => None,
    }
}

#[must_use]
pub fn blazar_mode(mode: &str) -> String {
    match mode {
        "accept-edits" => "acceptEdits",
        "smart" => "auto",
        "bypass" => "bypassPermissions",
        other => other,
    }
    .to_owned()
}

fn request(id: &str, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn line(id: &str, v: &Value) -> String {
    format!("{id}\t{v}")
}

fn pairs(
    map: &std::collections::BTreeMap<String, String>,
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<Value> {
    map.iter()
        .filter_map(|(name, value)| {
            let value = match crate::cli_runtime::env_reference(value) {
                Some(source) => env.get(source)?.clone(),
                None => value.clone(),
            };
            Some(json!({ "name": name, "value": value }))
        })
        .collect()
}

#[must_use]
pub fn mcp_servers(session: &SessionSpec) -> Vec<Value> {
    session
        .mcp_servers
        .iter()
        .map(|m| {
            if m.http {
                json!({
                    "type": "http",
                    "name": m.name,
                    "url": m.url,
                    "headers": pairs(&m.headers, &session.env),
                })
            } else {
                let stdio = crate::cli_runtime::stdio_launch(m);
                json!({
                    "name": m.name,
                    "command": stdio.command,
                    "args": stdio.args,
                    "env": stdio
                        .env
                        .iter()
                        .map(|(name, value)| json!({ "name": name, "value": value }))
                        .collect::<Vec<_>>(),
                })
            }
        })
        .collect()
}

#[must_use]
pub fn first_input(session: &SessionSpec, resume: Option<&str>) -> String {
    let init = request(
        INIT_ID,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "terminal": false,
            },
            "clientInfo": { "name": "blazar", "title": "Blazar", "version": env!("CARGO_PKG_VERSION") },
        }),
    );
    let cwd = session.cwd.display().to_string();
    let servers = mcp_servers(session);
    let open = match resume {
        Some(id) => request(
            &format!("{LOAD_PREFIX}{id}"),
            "session/load",
            json!({ "sessionId": id, "cwd": cwd, "mcpServers": servers }),
        ),
        None => request(
            NEW_ID,
            "session/new",
            json!({ "cwd": cwd, "mcpServers": servers }),
        ),
    };
    [line("init", &init), line("session", &open)].join("\n")
}

#[must_use]
pub fn first_text(session: &SessionSpec, resume: bool) -> String {
    let briefing = crate::cli_runtime::briefing_of(session);
    if resume || briefing.is_empty() {
        session.prompt.clone()
    } else {
        format!("{briefing}\n\n{}", session.prompt)
    }
}

#[must_use]
pub fn prompt(session: &str, text: &str, images: &[ImageInput], id: &str) -> String {
    let mut blocks: Vec<Value> = images
        .iter()
        .map(|i| json!({ "type": "image", "mimeType": i.media_type, "data": i.data }))
        .collect();
    blocks.push(json!({ "type": "text", "text": text }));
    request(
        id,
        "session/prompt",
        json!({ "sessionId": session, "prompt": blocks }),
    )
    .to_string()
}

#[must_use]
pub fn set_mode(session: &str, mode: &str, id: &str) -> Option<String> {
    let mode = mode_for(mode)?;
    Some(
        request(
            id,
            "session/set_mode",
            json!({ "sessionId": session, "modeId": mode }),
        )
        .to_string(),
    )
}

#[must_use]
pub fn set_model(session: &str, model: &str) -> Option<String> {
    let model = model.trim();
    (!model.is_empty()).then(|| {
        request(
            MODEL_ID,
            "session/set_config_option",
            json!({ "sessionId": session, "configId": "model", "value": model }),
        )
        .to_string()
    })
}

#[must_use]
pub fn opening(
    session: &str,
    mode: Option<&str>,
    model: Option<&str>,
    text: &str,
    images: &[ImageInput],
    prompt_id: &str,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(m) = mode.and_then(|m| set_mode(session, m, MODE_ID)) {
        out.push(("mode".to_owned(), m));
    }
    if let Some(m) = model.and_then(|m| set_model(session, m)) {
        out.push(("model".to_owned(), m));
    }
    out.push((
        prompt_id.to_owned(),
        prompt(session, text, images, prompt_id),
    ));
    out
}

#[must_use]
pub fn cancel(session: &str) -> String {
    json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } })
        .to_string()
}

#[must_use]
pub fn permission_reply(raw_request: &str, allow: bool) -> Option<String> {
    let v: Value = serde_json::from_str(raw_request).ok()?;
    if v.get("method").and_then(Value::as_str) != Some(PERMISSION) {
        return None;
    }
    let id = v.get("id")?.clone();
    let options = v["params"]["options"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let (exact, family) = if allow {
        ("allow_once", "allow")
    } else {
        ("reject_once", "reject")
    };
    let pick = options
        .iter()
        .find(|o| o["kind"] == exact)
        .or_else(|| {
            options
                .iter()
                .find(|o| o["kind"].as_str().is_some_and(|k| k.starts_with(family)))
        })
        .and_then(|o| o["optionId"].as_str());
    let outcome = match pick {
        Some(option) => json!({ "outcome": "selected", "optionId": option }),
        None => json!({ "outcome": "cancelled" }),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": { "outcome": outcome } }).to_string())
}

#[must_use]
pub fn unsupported_reply(raw_request: &str) -> Option<String> {
    let v: Value = serde_json::from_str(raw_request).ok()?;
    let method = v.get("method").and_then(Value::as_str)?;
    if method == PERMISSION {
        return None;
    }
    let id = v.get("id")?.clone();
    Some(
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("Blazar doesn't handle {method}") },
        })
        .to_string(),
    )
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Ids {
    pub session: Option<String>,
    pub prompt_done: bool,
    pub request: bool,
}

#[must_use]
pub fn ids(line: &str) -> Ids {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Ids::default();
    };
    let mut out = Ids::default();
    let id = crate::codex_app::request_id_text(&v);
    if v.get("method").is_some() {
        out.request = id.is_some();
        return out;
    }
    let Some(id) = id else {
        return out;
    };
    if v.get("error").is_none() {
        if id == NEW_ID {
            out.session = v["result"]["sessionId"].as_str().map(str::to_owned);
        } else if let Some(s) = id.strip_prefix(LOAD_PREFIX) {
            out.session = Some(s.to_owned());
        }
    }
    out.prompt_done = id.starts_with(PROMPT_PREFIX);
    out
}

#[derive(Debug, Clone)]
struct Call {
    name: String,
    input: Value,
    content: String,
    exit: Option<i64>,
    done: bool,
}

#[derive(Debug, Default)]
pub struct State {
    open: bool,
    session: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    text: String,
    thought: String,
    last_text: Option<String>,
    ended: bool,
    tools: HashMap<String, Call>,
    plans: u64,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.trim().is_empty())
        .map(str::to_owned)
}

fn diff_of(content: &Value) -> Option<&Value> {
    content.as_array()?.iter().find(|c| c["type"] == "diff")
}

#[must_use]
pub fn tool(name: &str, kind: &str, raw: &Value, content: &Value) -> (String, Value) {
    let path = || s(raw, "file_path").or_else(|| s(raw, "path"));
    let query = || s(raw, "pattern").or_else(|| s(raw, "query"));
    let mapped = match (name, kind) {
        ("exec" | "bash" | "shell", _) | (_, "execute") => {
            s(raw, "command").map(|c| ("Bash", json!({ "command": c })))
        }
        ("write", _) => path().map(|f| {
            (
                "Write",
                json!({ "file_path": f, "content": s(raw, "content").unwrap_or_default() }),
            )
        }),
        ("edit", _) => {
            let diff = diff_of(content);
            path().or_else(|| diff.and_then(|d| s(d, "path"))).map(|f| {
                let pick = |raw_key: &str, diff_key: &str| {
                    raw.get(raw_key)
                        .and_then(Value::as_str)
                        .or_else(|| diff.and_then(|d| d.get(diff_key)).and_then(Value::as_str))
                        .unwrap_or_default()
                        .to_owned()
                };
                (
                    "Edit",
                    json!({
                        "file_path": f,
                        "old_string": pick("old_string", "oldText"),
                        "new_string": pick("new_string", "newText"),
                    }),
                )
            })
        }
        ("read", _) => path().map(|f| ("Read", json!({ "file_path": f }))),
        ("grep", _) => query().map(|q| ("Grep", json!({ "pattern": q, "path": path() }))),
        ("find_file_by_name" | "glob" | "find", _) => {
            query().map(|q| ("Glob", json!({ "pattern": q, "path": path() })))
        }
        ("list_dir" | "ls", _) => path().map(|p| ("LS", json!({ "path": p }))),
        ("web_search", _) => s(raw, "query").map(|q| ("WebSearch", json!({ "query": q }))),
        ("fetch" | "read_url" | "web_get_contents", _) => {
            s(raw, "url").map(|u| ("WebFetch", json!({ "url": u })))
        }
        _ => None,
    };
    mapped.map_or_else(
        || {
            let shown = if name.is_empty() { kind } else { name };
            (
                if shown.is_empty() { "Tool" } else { shown }.to_owned(),
                raw.clone(),
            )
        },
        |(n, input)| (n.to_owned(), input),
    )
}

fn content_text(content: &Value) -> String {
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|c| match c["type"].as_str() {
                    Some("diff") => s(c, "path"),
                    Some("content") => {
                        let inner = &c["content"];
                        match inner["type"].as_str() {
                            Some("text") => inner["text"].as_str().map(str::to_owned),
                            Some("resource_link") => s(inner, "uri")
                                .map(|u| u.strip_prefix("file://").map_or(u.clone(), str::to_owned))
                                .or_else(|| s(inner, "name")),
                            Some("resource") if inner["resource"]["uri"] != "tool://preview" => {
                                s(&inner["resource"], "text")
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn error_text(e: &Value) -> String {
    s(e, "data")
        .or_else(|| s(e, "message"))
        .unwrap_or_else(|| "Devin returned an error".to_owned())
}

fn flush(state: &mut State) -> Vec<EntryKind> {
    let mut out = Vec::new();
    let thought = std::mem::take(&mut state.thought);
    if !thought.trim().is_empty() {
        out.push(EntryKind::Thinking { text: thought });
    }
    let text = std::mem::take(&mut state.text);
    if !text.trim().is_empty() {
        state.last_text = Some(text.clone());
        out.push(EntryKind::AssistantMessage { text });
    }
    out
}

fn usage(u: &Value) -> Option<TokenUsage> {
    let meta = u.get("_meta").filter(|m| m.is_object())?;
    if meta.get("cognition.ai/subagent_context").is_some() {
        return None;
    }
    let n = |k: &str| meta.get(k).and_then(Value::as_u64);
    let input = n("cognition.ai/inputTokens")?;
    let cached = n("cognition.ai/cachedReadTokens").unwrap_or(0);
    Some(TokenUsage {
        input: input.saturating_sub(cached),
        output: n("cognition.ai/outputTokens").unwrap_or(0),
        cache_read: cached,
        cache_creation: n("cognition.ai/cachedWriteTokens").unwrap_or(0),
        cost_usd: None,
        context: u.get("used").and_then(Value::as_u64),
        window: u.get("size").and_then(Value::as_u64),
    })
}

fn turn_usage(r: &Value) -> Option<TokenUsage> {
    let u = r.get("usage").filter(|u| u.is_object())?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cached = n("cachedReadTokens");
    Some(TokenUsage {
        input: n("inputTokens").saturating_sub(cached),
        output: n("outputTokens"),
        cache_read: cached,
        cache_creation: n("cachedWriteTokens"),
        cost_usd: None,
        context: Some(n("totalTokens")).filter(|t| *t > 0),
        window: None,
    })
}

fn config_value(options: &Value, id: &str) -> Option<String> {
    options
        .as_array()?
        .iter()
        .find(|o| o["id"] == id)
        .and_then(|o| s(o, "currentValue"))
}

fn started(state: &State) -> Vec<EntryKind> {
    let Some(session) = state.session.clone() else {
        return Vec::new();
    };
    vec![EntryKind::SessionStarted {
        provider_session_id: ProviderSessionId(session),
        model: state.model.clone(),
        cwd: None,
        permission_mode: state.mode.as_deref().map(blazar_mode),
        output_style: None,
        fast_mode: None,
        fast_mode_reason: None,
        mcp_servers: Vec::new(),
    }]
}

fn open(state: &mut State, r: &Value, session: String) -> Vec<EntryKind> {
    state.open = true;
    state.session = Some(session);
    state.model = config_value(&r["configOptions"], "model");
    state.mode =
        s(&r["modes"], "currentModeId").or_else(|| config_value(&r["configOptions"], "mode"));
    started(state)
}

fn settings(state: &mut State, model: Option<String>, mode: Option<String>) -> Vec<EntryKind> {
    let model = model.or_else(|| state.model.clone());
    let mode = mode.or_else(|| state.mode.clone());
    if model == state.model && mode == state.mode {
        return Vec::new();
    }
    state.model = model;
    state.mode = mode;
    started(state)
}

fn response(state: &mut State, v: &Value, id: &str) -> Vec<EntryKind> {
    if let Some(e) = v.get("error") {
        let message = error_text(e);
        if id.starts_with(PROMPT_PREFIX) {
            let mut out = flush(state);
            state.ended = true;
            out.push(EntryKind::Error {
                message: message.clone(),
            });
            out.push(EntryKind::InputConsumed {
                text: String::new(),
            });
            out.push(EntryKind::Finished(Outcome::Failed { message }));
            return out;
        }
        return match id {
            INIT_ID | NEW_ID => vec![EntryKind::Finished(Outcome::Failed { message })],
            MODEL_ID => vec![EntryKind::Notice {
                message: format!("Devin kept its current model: {message}"),
            }],
            _ if id.starts_with(LOAD_PREFIX) => {
                vec![EntryKind::Finished(Outcome::Failed { message })]
            }
            _ => vec![EntryKind::Notice { message }],
        };
    }
    let r = &v["result"];
    if id == NEW_ID {
        return s(r, "sessionId")
            .map(|sid| open(state, r, sid))
            .unwrap_or_default();
    }
    if let Some(sid) = id.strip_prefix(LOAD_PREFIX) {
        return open(state, r, sid.to_owned());
    }
    if !id.starts_with(PROMPT_PREFIX) {
        return Vec::new();
    }
    let mut out = flush(state);
    state.ended = true;
    let used = turn_usage(r);
    out.push(EntryKind::InputConsumed {
        text: String::new(),
    });
    let stop = r["stopReason"].as_str().unwrap_or("end_turn");
    out.push(EntryKind::Finished(match stop {
        "cancelled" => Outcome::Interrupted,
        "refusal" => Outcome::Failed {
            message: "Devin declined this request".to_owned(),
        },
        _ => Outcome::Success {
            text: state.last_text.clone(),
            usage: used,
            denied: Vec::new(),
            denials: Vec::new(),
        },
    }));
    if matches!(stop, "max_tokens" | "max_turn_requests") {
        out.insert(
            out.len() - 1,
            EntryKind::Notice {
                message: if stop == "max_tokens" {
                    "Devin stopped at its output limit".to_owned()
                } else {
                    "Devin stopped at its step limit for one turn".to_owned()
                },
            },
        );
    }
    state.tools.clear();
    out
}

fn permission(state: &mut State, v: &Value, rid: &str) -> Vec<EntryKind> {
    let mut out = flush(state);
    let call = &v["params"]["toolCall"];
    let call_id = s(call, "toolCallId");
    let known = call_id.as_ref().and_then(|id| state.tools.get(id));
    let (name, input) = match known {
        Some(c) => (c.name.clone(), c.input.clone()),
        None => match s(&call["_meta"], "cognition.ai/editableCommand") {
            Some(command) => ("Bash".to_owned(), json!({ "command": command })),
            None => tool(
                "",
                call["kind"].as_str().unwrap_or_default(),
                call.get("rawInput").unwrap_or(&Value::Null),
                call.get("content").unwrap_or(&Value::Null),
            ),
        },
    };
    out.push(EntryKind::Approval {
        id: ApprovalId::from_provider(&format!("devin:{rid}")),
        request: json!({
            "provider_request_id": rid,
            "tool_name": name,
            "display_name": name,
            "input": input,
            "description": Value::Null,
            "tool_use_id": call_id,
        }),
    });
    out
}

fn tool_use(state: &mut State, u: &Value) -> Vec<EntryKind> {
    let Some(id) = s(u, "toolCallId") else {
        return Vec::new();
    };
    let meta_name = u["_meta"]["cognition.ai/inferenceToolName"]
        .as_str()
        .unwrap_or_default();
    let content = u.get("content").unwrap_or(&Value::Null);
    let raw = u
        .get("rawInput")
        .filter(|r| r.is_object())
        .cloned()
        .unwrap_or_else(|| json!({ "title": s(u, "title") }));
    let (name, input) = tool(
        meta_name,
        u["kind"].as_str().unwrap_or_default(),
        &raw,
        content,
    );
    let mut out = vec![EntryKind::ToolUse {
        id: ToolId(id.clone()),
        name: name.clone(),
        input: input.clone(),
    }];
    state.tools.insert(
        id.clone(),
        Call {
            name,
            input,
            content: String::new(),
            exit: None,
            done: false,
        },
    );
    out.extend(tool_update(state, u));
    out
}

fn tool_update(state: &mut State, u: &Value) -> Vec<EntryKind> {
    let Some(id) = s(u, "toolCallId") else {
        return Vec::new();
    };
    let Some(call) = state.tools.get_mut(&id) else {
        return Vec::new();
    };
    if let Some(c) = u.get("content").filter(|c| c.is_array()) {
        let text = content_text(c);
        if !text.is_empty() || call.content.is_empty() {
            call.content = text;
        }
    }
    if let Some(code) = u["_meta"]["terminal_exit"]["exit_code"].as_i64() {
        call.exit = Some(code);
    }
    let status = u["status"].as_str().unwrap_or_default();
    if call.done || !matches!(status, "completed" | "failed") {
        return Vec::new();
    }
    call.done = true;
    vec![EntryKind::ToolResult {
        id: ToolId(id),
        ok: status == "completed" && call.exit.is_none_or(|c| c == 0),
        content: call.content.clone(),
        structured: None,
        images: Vec::new(),
    }]
}

fn plan(state: &mut State, u: &Value) -> Vec<EntryKind> {
    let todos: Vec<Value> = u["entries"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    let content = s(e, "content")?;
                    Some(json!({
                        "content": content,
                        "activeForm": content,
                        "status": s(e, "status").unwrap_or_else(|| "pending".to_owned()),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    state.plans += 1;
    let stamp = chrono::Utc::now().timestamp_micros();
    let id = ToolId(format!("devin-plan-{stamp}-{}", state.plans));
    vec![
        EntryKind::ToolUse {
            id: id.clone(),
            name: "TodoWrite".to_owned(),
            input: json!({ "todos": todos }),
        },
        EntryKind::ToolResult {
            id,
            ok: true,
            content: String::new(),
            structured: None,
            images: Vec::new(),
        },
    ]
}

fn update(state: &mut State, u: &Value) -> Vec<EntryKind> {
    let chunk = || {
        let c = &u["content"];
        (c["type"] == "text")
            .then(|| c["text"].as_str())
            .flatten()
            .unwrap_or_default()
    };
    let kind = u["sessionUpdate"].as_str().unwrap_or_default();
    if state.ended
        && matches!(
            kind,
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "plan"
        )
    {
        state.ended = false;
        state.last_text = None;
    }
    match kind {
        "agent_message_chunk" => {
            let mut out = Vec::new();
            if !state.thought.is_empty() {
                let thought = std::mem::take(&mut state.thought);
                if !thought.trim().is_empty() {
                    out.push(EntryKind::Thinking { text: thought });
                }
            }
            state.text.push_str(chunk());
            out
        }
        "agent_thought_chunk" => {
            let mut out = Vec::new();
            if !state.text.is_empty() {
                out = flush(state);
            }
            state.thought.push_str(chunk());
            out
        }
        "tool_call" => {
            let mut out = flush(state);
            out.extend(tool_use(state, u));
            out
        }
        "tool_call_update" => {
            let mut out = flush(state);
            out.extend(tool_update(state, u));
            out
        }
        "plan" => {
            let mut out = flush(state);
            out.extend(plan(state, u));
            out
        }
        "usage_update" => usage(u).map(EntryKind::TokenUsage).into_iter().collect(),
        "config_option_update" => settings(
            state,
            config_value(&u["configOptions"], "model"),
            config_value(&u["configOptions"], "mode"),
        ),
        "current_mode_update" => settings(state, None, s(u, "currentModeId")),
        _ => Vec::new(),
    }
}

#[must_use]
pub fn parse_line(state: &mut State, line: &str) -> Vec<EntryKind> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let id = crate::codex_app::request_id_text(&v);
    match (v.get("method").and_then(Value::as_str), id) {
        (None, Some(id)) => response(state, &v, &id),
        (Some(PERMISSION), Some(rid)) => permission(state, &v, &rid),
        (Some("session/update"), None) if state.open => update(state, &v["params"]["update"]),
        (Some("$/cancel_request"), None) => {
            let rid = match &v["params"]["requestId"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return Vec::new(),
            };
            vec![EntryKind::ApprovalResolved {
                id: ApprovalId::from_provider(&format!("devin:{rid}")),
                decision: ApprovalDecision::Cancelled,
            }]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
