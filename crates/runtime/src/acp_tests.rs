use blazar_core_types::{ApprovalDecision, ApprovalId, EntryKind, Outcome};
use serde_json::{Value, json};

use super::*;
use crate::{McpServerSpec, SessionSpec};

fn feed(state: &mut State, v: &Value) -> Vec<EntryKind> {
    parse_line(state, &v.to_string())
}

fn update(session: &str, u: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": u } })
}

fn new_session(state: &mut State) {
    let out = feed(
        state,
        &json!({ "jsonrpc": "2.0", "id": NEW_ID, "result": {
            "sessionId": "perfect-surgeon",
            "modes": { "currentModeId": "accept-edits", "availableModes": [{ "id": "accept-edits", "name": "Code" }] },
            "configOptions": [
                { "id": "mode", "type": "select", "currentValue": "accept-edits", "options": [] },
                { "id": "model", "type": "select", "currentValue": "swe-2-flash-medium", "options": [] }
            ]
        } }),
    );
    assert_eq!(out.len(), 1, "{out:?}");
}

fn lines(input: &str) -> Vec<(String, Value)> {
    input
        .lines()
        .map(|l| {
            let (id, body) = l.split_once('\t').unwrap();
            (id.to_owned(), serde_json::from_str(body).unwrap())
        })
        .collect()
}

#[test]
fn a_new_session_starts_with_the_handshake_and_blazars_tools() {
    let mut s = SessionSpec::new("/repo", "hi");
    s.env.insert("FLEET_TOKEN".into(), "t0k".into());
    s.mcp_servers = vec![
        McpServerSpec {
            name: "blazar-fleet".into(),
            http: true,
            url: "http://127.0.0.1:7777/mcp".into(),
            headers: [
                ("Authorization".to_owned(), "${FLEET_TOKEN}".to_owned()),
                ("X-Missing".to_owned(), "${NOT_SET}".to_owned()),
            ]
            .into(),
            ..Default::default()
        },
        McpServerSpec {
            name: "notes".into(),
            command: "notes-mcp".into(),
            args: vec!["--stdio".into()],
            env: [
                ("MODE".to_owned(), "fast".to_owned()),
                ("KEY".to_owned(), "${NOTES_KEY}".to_owned()),
            ]
            .into(),
            ..Default::default()
        },
    ];
    let sent = lines(&first_input(&s, None));
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].1["method"], "initialize");
    assert_eq!(sent[0].1["params"]["protocolVersion"], 1);
    assert_eq!(
        sent[0].1["params"]["clientCapabilities"]["terminal"], false,
        "Devin 用自己的工具跑命令"
    );
    let new = &sent[1].1;
    assert_eq!(new["id"], NEW_ID);
    assert_eq!(new["method"], "session/new");
    assert_eq!(new["params"]["cwd"], "/repo");
    let servers = new["params"]["mcpServers"].as_array().unwrap();
    assert_eq!(
        servers[0],
        json!({ "type": "http", "name": "blazar-fleet", "url": "http://127.0.0.1:7777/mcp",
                "headers": [{ "name": "Authorization", "value": "t0k" }] }),
        "引用的环境变量要换成值，找不到的头不发"
    );
    assert_eq!(servers[1]["name"], "notes");
    assert_eq!(servers[1]["command"], "bash");
    assert_eq!(
        servers[1]["env"],
        json!([{ "name": "MODE", "value": "fast" }])
    );
    let args: Vec<&str> = servers[1]["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        args[3].contains("${NOTES_KEY?"),
        "密钥由 shell 在启动时从环境里取，不写进消息：{args:?}"
    );
    assert_eq!(&args[5..], ["notes-mcp", "--stdio"]);

    let load = lines(&first_input(&s, Some("perfect-surgeon")));
    assert_eq!(load[1].1["id"], "blazar-load:perfect-surgeon");
    assert_eq!(load[1].1["method"], "session/load");
    assert_eq!(load[1].1["params"]["sessionId"], "perfect-surgeon");
}

#[test]
fn the_briefing_rides_on_the_first_message_of_a_new_session_only() {
    let mut s = SessionSpec::new("/repo", "fix the bug");
    s.instructions = Some("Report back when done".into());
    assert_eq!(
        first_text(&s, false),
        "Report back when done\n\nfix the bug"
    );
    assert_eq!(first_text(&s, true), "fix the bug");
    s.instructions = None;
    assert_eq!(first_text(&s, false), "fix the bug");
}

#[test]
fn each_turn_sets_mode_and_model_before_the_prompt() {
    let sent = opening(
        "perfect-surgeon",
        Some("auto"),
        Some("swe-1-6-slow"),
        "look at this",
        &[crate::ImageInput {
            media_type: "image/png".into(),
            data: "iVBOR".into(),
        }],
        "blazar-prompt-u-1",
    );
    let ids: Vec<&str> = sent.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["mode", "model", "blazar-prompt-u-1"]);
    let v: Vec<Value> = sent
        .iter()
        .map(|(_, l)| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(v[0]["method"], "session/set_mode");
    assert_eq!(
        v[0]["params"]["modeId"], "smart",
        "Blazar 的 Auto 对应 Smart"
    );
    assert_eq!(v[1]["method"], "session/set_config_option");
    assert_eq!(v[1]["params"]["configId"], "model");
    assert_eq!(v[1]["params"]["value"], "swe-1-6-slow");
    assert_eq!(
        v[2]["params"]["prompt"],
        json!([
            { "type": "image", "mimeType": "image/png", "data": "iVBOR" },
            { "type": "text", "text": "look at this" }
        ])
    );
    let bare = opening(
        "s",
        Some("dontAsk"),
        Some(" "),
        "hi",
        &[],
        "blazar-prompt-u-2",
    );
    assert_eq!(bare.len(), 1, "不认的档位和空型号都不发");
    for (blazar, devin) in [
        ("ask", "ask"),
        ("acceptEdits", "accept-edits"),
        ("plan", "plan"),
        ("auto", "smart"),
        ("bypassPermissions", "bypass"),
    ] {
        assert_eq!(mode_for(blazar), Some(devin));
        assert_eq!(blazar_mode(devin), blazar);
    }
    assert_eq!(mode_for("default"), Some("accept-edits"));
}

#[test]
fn a_loaded_session_hides_the_replayed_history() {
    let mut state = State::default();
    for u in [
        json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "Reply with exactly: OK" } }),
        json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "The user wants exactly \"OK\"." } }),
        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "OK" } }),
        json!({ "sessionUpdate": "usage_update", "used": 14536, "size": 262144,
                "_meta": { "cognition.ai/inputTokens": 14522, "cognition.ai/outputTokens": 14 } }),
    ] {
        assert!(feed(&mut state, &update("perfect-surgeon", u)).is_empty());
    }
    let out = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "id": "blazar-load:perfect-surgeon", "result": {
            "modes": { "currentModeId": "smart", "availableModes": [] },
            "configOptions": [{ "id": "model", "currentValue": "swe-1-6-slow", "options": [] }]
        } }),
    );
    let [
        EntryKind::SessionStarted {
            provider_session_id,
            model,
            permission_mode,
            ..
        },
    ] = out.as_slice()
    else {
        panic!("{out:?}");
    };
    assert_eq!(provider_session_id.0, "perfect-surgeon");
    assert_eq!(model.as_deref(), Some("swe-1-6-slow"));
    assert_eq!(permission_mode.as_deref(), Some("auto"));
    let live = feed(
        &mut state,
        &update(
            "perfect-surgeon",
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "now" } }),
        ),
    );
    assert!(live.is_empty(), "正文攒到边界再出");
}

#[test]
fn chunks_become_whole_messages_and_the_turn_ends_on_the_response() {
    let mut state = State::default();
    new_session(&mut state);
    let mut out = Vec::new();
    for u in [
        json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "User rejected." } }),
        json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": " I should note it briefly." } }),
        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "The `rm note.txt` command was rejected" } }),
        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": ", so note.txt still exists." } }),
        json!({ "sessionUpdate": "usage_update", "used": 14806, "size": 262144,
                "_meta": { "cognition.ai/inputTokens": 14775, "cognition.ai/outputTokens": 31, "cognition.ai/cachedReadTokens": 14720 } }),
        json!({ "sessionUpdate": "usage_update", "used": 14806, "size": 262144,
                "_meta": { "cognition.ai/inputTokens": 14775, "cognition.ai/outputTokens": 31, "cognition.ai/cachedReadTokens": 14720,
                           "cognition.ai/subagent_context": { "parentAgentId": "root" } } }),
    ] {
        out.extend(feed(&mut state, &update("perfect-surgeon", u)));
    }
    let [
        EntryKind::Thinking { text: thought },
        EntryKind::TokenUsage(u),
    ] = out.as_slice()
    else {
        panic!("{out:?}");
    };
    assert_eq!(thought, "User rejected. I should note it briefly.");
    assert_eq!(
        (u.input, u.output, u.cache_read, u.context, u.window),
        (55, 31, 14720, Some(14806), Some(262_144)),
        "输入不含缓存命中，同一次调用的子代理副本不重复记"
    );
    let end = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "id": "blazar-prompt-u-1", "result": {
            "stopReason": "end_turn",
            "usage": { "totalTokens": 14806, "inputTokens": 14775, "outputTokens": 31, "cachedReadTokens": 14720 }
        } }),
    );
    let [
        EntryKind::AssistantMessage { text },
        EntryKind::InputConsumed { .. },
        EntryKind::Finished(Outcome::Success {
            text: Some(last),
            usage: Some(turn),
            ..
        }),
    ] = end.as_slice()
    else {
        panic!("{end:?}");
    };
    assert_eq!(
        text,
        "The `rm note.txt` command was rejected, so note.txt still exists."
    );
    assert_eq!(last, text);
    assert_eq!((turn.input, turn.output), (55, 31));
    assert!(
        parse_line(&mut state, "not json").is_empty(),
        "坏行不出条目"
    );
}

#[test]
fn tool_calls_pair_with_their_results_and_ask_for_approval() {
    let mut state = State::default();
    new_session(&mut state);
    let wrote = feed(
        &mut state,
        &update(
            "perfect-surgeon",
            json!({ "sessionUpdate": "tool_call", "toolCallId": "call_bcab67b69c5ffbd7", "title": "Wrote ./note.txt", "kind": "edit",
                    "content": [{ "type": "diff", "path": "/private/tmp/p/note.txt", "newText": "banana" }],
                    "rawInput": { "content": "banana", "file_path": "/private/tmp/p/note.txt" },
                    "_meta": { "cognition.ai/inferenceToolName": "write" } }),
        ),
    );
    assert!(
        matches!(wrote.as_slice(), [EntryKind::ToolUse { id, name, input }]
            if id.0 == "call_bcab67b69c5ffbd7" && name == "Write" && input["file_path"] == "/private/tmp/p/note.txt" && input["content"] == "banana"),
        "{wrote:?}"
    );
    let done = feed(
        &mut state,
        &update(
            "perfect-surgeon",
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call_bcab67b69c5ffbd7", "status": "completed",
                    "_meta": { "cognition.ai/inferenceToolName": "write" } }),
        ),
    );
    assert!(matches!(
        done.as_slice(),
        [EntryKind::ToolResult { ok: true, .. }]
    ));

    let ran = feed(
        &mut state,
        &update(
            "perfect-surgeon",
            json!({ "sessionUpdate": "tool_call", "toolCallId": "call_8536b137710bf8cc", "title": "Ran rm", "kind": "execute",
                    "content": [{ "type": "content", "content": { "type": "resource",
                                  "resource": { "mimeType": "text/x-shellscript", "text": "rm note.txt", "uri": "tool://preview" } } }],
                    "rawInput": { "command": "rm note.txt", "workdir": "/private/tmp/p" },
                    "_meta": { "cognition.ai/inferenceToolName": "exec" } }),
        ),
    );
    assert!(
        matches!(ran.as_slice(), [EntryKind::ToolUse { name, input, .. }] if name == "Bash" && input == &json!({ "command": "rm note.txt" }))
    );
    let raw = json!({ "jsonrpc": "2.0", "id": "339c8c09-e307-4553-b554-9b83459eb9fb", "method": "session/request_permission",
        "params": { "sessionId": "perfect-surgeon",
                    "toolCall": { "toolCallId": "call_8536b137710bf8cc", "_meta": { "cognition.ai/editableCommand": "rm note.txt" } },
                    "options": [
                        { "optionId": "allow_once", "name": "Allow", "kind": "allow_once" },
                        { "optionId": "allow_session", "name": "Yes, allow `rm` commands (this session)", "kind": "allow_always" },
                        { "optionId": "reject_once", "name": "Reject", "kind": "reject_once" }
                    ] } });
    let asked = feed(&mut state, &raw);
    let [EntryKind::Approval { id, request }] = asked.as_slice() else {
        panic!("{asked:?}");
    };
    assert_eq!(
        *id,
        ApprovalId::from_provider("devin:339c8c09-e307-4553-b554-9b83459eb9fb")
    );
    assert_eq!(
        request["provider_request_id"],
        "339c8c09-e307-4553-b554-9b83459eb9fb"
    );
    assert_eq!(request["tool_name"], "Bash");
    assert_eq!(request["input"]["command"], "rm note.txt");
    assert_eq!(request["tool_use_id"], "call_8536b137710bf8cc");

    let allow: Value =
        serde_json::from_str(&permission_reply(&raw.to_string(), true).unwrap()).unwrap();
    assert_eq!(allow["id"], "339c8c09-e307-4553-b554-9b83459eb9fb");
    assert_eq!(
        allow["result"]["outcome"],
        json!({ "outcome": "selected", "optionId": "allow_once" }),
        "只放行这一次，不替用户记规则"
    );
    let deny: Value =
        serde_json::from_str(&permission_reply(&raw.to_string(), false).unwrap()).unwrap();
    assert_eq!(deny["result"]["outcome"]["optionId"], "reject_once");
    let mut lone = raw.clone();
    lone["params"]["options"] = json!([{ "optionId": "ok", "kind": "allow_once" }]);
    let cancelled: Value =
        serde_json::from_str(&permission_reply(&lone.to_string(), false).unwrap()).unwrap();
    assert_eq!(
        cancelled["result"]["outcome"],
        json!({ "outcome": "cancelled" })
    );

    let rejected = feed(
        &mut state,
        &update(
            "perfect-surgeon",
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call_8536b137710bf8cc", "status": "failed",
                    "content": [{ "type": "content", "content": { "type": "text", "text": "Tool execution was rejected: User rejected this tool call" } }],
                    "_meta": { "cognition.ai/rejected": true } }),
        ),
    );
    assert!(
        matches!(rejected.as_slice(), [EntryKind::ToolResult { ok: false, content, .. }]
            if content == "Tool execution was rejected: User rejected this tool call")
    );
}

#[test]
fn shell_output_and_exit_codes_reach_the_result() {
    let mut state = State::default();
    new_session(&mut state);
    let call = |id: &str| {
        update(
            "s",
            json!({ "sessionUpdate": "tool_call", "toolCallId": id, "title": "Listed ./", "kind": "execute",
                    "rawInput": { "command": "ls", "workdir": "/p" }, "_meta": { "cognition.ai/inferenceToolName": "exec" } }),
        )
    };
    let output = |id: &str, code: i64| {
        update(
            "s",
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "in_progress",
                    "content": [{ "type": "content", "content": { "type": "text", "text": "note.txt" } }],
                    "_meta": { "terminal_exit": { "terminal_id": "263005", "exit_code": code, "signal": null } } }),
        )
    };
    let finish = |id: &str| {
        update(
            "s",
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed" }),
        )
    };
    feed(&mut state, &call("a"));
    assert!(feed(&mut state, &output("a", 0)).is_empty());
    let ok = feed(&mut state, &finish("a"));
    assert!(
        matches!(ok.as_slice(), [EntryKind::ToolResult { ok: true, content, .. }] if content == "note.txt")
    );
    feed(&mut state, &call("b"));
    feed(&mut state, &output("b", 2));
    let failed = feed(&mut state, &finish("b"));
    assert!(matches!(
        failed.as_slice(),
        [EntryKind::ToolResult { ok: false, .. }]
    ));
    assert!(
        feed(&mut state, &finish("b")).is_empty(),
        "同一个调用只出一次结果"
    );
}

#[test]
fn devin_tools_read_like_the_ones_blazar_already_shows() {
    assert_eq!(
        tool(
            "edit",
            "edit",
            &json!({ "file_path": "/p/note.txt", "old_string": "banana", "new_string": "apple" }),
            &Value::Null
        ),
        (
            "Edit".to_owned(),
            json!({ "file_path": "/p/note.txt", "old_string": "banana", "new_string": "apple" })
        )
    );
    assert_eq!(
        tool(
            "edit",
            "edit",
            &json!({}),
            &json!([{ "type": "diff", "path": "/p/a.rs", "oldText": "x", "newText": "y" }])
        ),
        (
            "Edit".to_owned(),
            json!({ "file_path": "/p/a.rs", "old_string": "x", "new_string": "y" })
        ),
        "参数里没有就从 diff 里取"
    );
    assert_eq!(
        tool(
            "read",
            "read",
            &json!({ "file_path": "/p/note.txt" }),
            &Value::Null
        ),
        ("Read".to_owned(), json!({ "file_path": "/p/note.txt" }))
    );
    assert_eq!(
        tool(
            "grep",
            "search",
            &json!({ "query": "apple", "path": "/p" }),
            &Value::Null
        ),
        (
            "Grep".to_owned(),
            json!({ "pattern": "apple", "path": "/p" })
        )
    );
    assert_eq!(
        tool(
            "find_file_by_name",
            "search",
            &json!({ "query": "*.txt", "path": "/p" }),
            &Value::Null
        ),
        (
            "Glob".to_owned(),
            json!({ "pattern": "*.txt", "path": "/p" })
        )
    );
    assert_eq!(
        tool("", "execute", &json!({ "command": "make" }), &Value::Null),
        ("Bash".to_owned(), json!({ "command": "make" }))
    );
    let odd = json!({ "agent": "x" });
    assert_eq!(
        tool("spawn_subagent", "other", &odd, &Value::Null),
        ("spawn_subagent".to_owned(), odd.clone())
    );
    assert_eq!(
        tool("read", "read", &odd, &Value::Null),
        ("read".to_owned(), odd),
        "缺了关键参数就原样保留"
    );
    assert_eq!(
        content_text(&json!([
            { "type": "content", "content": { "type": "resource_link", "name": "note.txt", "uri": "file:///p/note.txt" } },
            { "type": "content", "content": { "type": "text", "text": "1 lines" } },
            { "type": "content", "content": { "type": "resource", "resource": { "text": "ls", "uri": "tool://preview" } } }
        ])),
        "/p/note.txt\n1 lines",
        "命令预览不算结果"
    );
}

#[test]
fn the_plan_shows_as_a_todo_list() {
    let mut state = State::default();
    new_session(&mut state);
    let out = feed(
        &mut state,
        &update(
            "s",
            json!({ "sessionUpdate": "plan", "entries": [
                { "content": "Read note.txt", "priority": "medium", "status": "completed" },
                { "content": "Edit note.txt: banana -> apple", "priority": "medium", "status": "in_progress" }
            ] }),
        ),
    );
    let [
        EntryKind::ToolUse { id, name, input },
        EntryKind::ToolResult {
            id: done, ok: true, ..
        },
    ] = out.as_slice()
    else {
        panic!("{out:?}");
    };
    assert_eq!(name, "TodoWrite");
    assert_eq!(id, done);
    assert_eq!(input["todos"][0]["content"], "Read note.txt");
    assert_eq!(input["todos"][0]["status"], "completed");
    assert_eq!(input["todos"][1]["status"], "in_progress");
    let again = feed(
        &mut state,
        &update("s", json!({ "sessionUpdate": "plan", "entries": [] })),
    );
    assert!(
        matches!(again.as_slice(), [EntryKind::ToolUse { id: other, .. }, _] if other != id),
        "每次更新各用一个编号"
    );
}

#[test]
fn cancelling_settles_the_open_approval_and_interrupts_the_turn() {
    let mut state = State::default();
    new_session(&mut state);
    feed(
        &mut state,
        &update(
            "s",
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "1\n2\n3\n" } }),
        ),
    );
    let gone = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "method": "$/cancel_request", "params": { "requestId": "b9bf493f-0add-4524-a9a1-27a81b488857" } }),
    );
    assert!(
        matches!(gone.as_slice(), [EntryKind::ApprovalResolved { id, decision: ApprovalDecision::Cancelled }]
            if *id == ApprovalId::from_provider("devin:b9bf493f-0add-4524-a9a1-27a81b488857")),
        "{gone:?}"
    );
    let end = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "id": "blazar-prompt-u-1", "result": { "stopReason": "cancelled",
                 "usage": { "totalTokens": 0, "inputTokens": 0, "outputTokens": 0 } } }),
    );
    assert!(
        matches!(end.as_slice(), [EntryKind::AssistantMessage { text }, EntryKind::InputConsumed { .. }, EntryKind::Finished(Outcome::Interrupted)] if text == "1\n2\n3\n"),
        "打断前写出的部分要留下：{end:?}"
    );
}

#[test]
fn failures_keep_the_reason_devin_gave() {
    let mut state = State::default();
    let no_session = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "id": NEW_ID, "error": { "code": -32000, "message": "Authentication required" } }),
    );
    assert!(
        matches!(no_session.as_slice(), [EntryKind::Finished(Outcome::Failed { message })] if message == "Authentication required")
    );
    new_session(&mut state);
    let model = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "id": MODEL_ID, "error": { "code": -32602, "message": "Invalid params",
                 "data": "Invalid value 'claude-opus-5-5-medium' for config option 'model'" } }),
    );
    assert!(
        matches!(model.as_slice(), [EntryKind::Notice { message }] if message.contains("Invalid value 'claude-opus-5-5-medium'")),
        "{model:?}"
    );
    let turn = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "id": "blazar-prompt-u-1", "error": { "code": -32603, "message": "Quota exceeded" } }),
    );
    assert!(
        matches!(turn.as_slice(), [EntryKind::Error { .. }, EntryKind::InputConsumed { .. }, EntryKind::Finished(Outcome::Failed { message })] if message == "Quota exceeded"),
        "{turn:?}"
    );
    let refused = feed(
        &mut state,
        &json!({ "jsonrpc": "2.0", "id": "blazar-prompt-u-2", "result": { "stopReason": "refusal" } }),
    );
    assert!(matches!(
        refused.last(),
        Some(EntryKind::Finished(Outcome::Failed { .. }))
    ));
}

#[test]
fn switching_model_or_mode_updates_what_the_chat_shows() {
    let mut state = State::default();
    new_session(&mut state);
    let switched = feed(
        &mut state,
        &update(
            "perfect-surgeon",
            json!({ "sessionUpdate": "config_option_update", "configOptions": [
                { "id": "mode", "currentValue": "ask", "options": [] },
                { "id": "model", "currentValue": "swe-1-6-slow", "options": [] }
            ] }),
        ),
    );
    assert!(
        matches!(switched.as_slice(), [EntryKind::SessionStarted { model, permission_mode, .. }]
            if model.as_deref() == Some("swe-1-6-slow") && permission_mode.as_deref() == Some("ask")),
        "{switched:?}"
    );
    assert!(
        feed(
            &mut state,
            &update(
                "perfect-surgeon",
                json!({ "sessionUpdate": "current_mode_update", "currentModeId": "ask" })
            )
        )
        .is_empty(),
        "没变就不重复报"
    );
}

#[test]
fn the_hub_learns_the_session_and_the_end_of_each_prompt() {
    let new = ids(
        &json!({ "jsonrpc": "2.0", "id": NEW_ID, "result": { "sessionId": "maddening-mallet" } })
            .to_string(),
    );
    assert_eq!(new.session.as_deref(), Some("maddening-mallet"));
    let load = ids(
        &json!({ "jsonrpc": "2.0", "id": "blazar-load:perfect-surgeon", "result": {} }).to_string(),
    );
    assert_eq!(load.session.as_deref(), Some("perfect-surgeon"));
    let failed = ids(
        &json!({ "jsonrpc": "2.0", "id": "blazar-load:x", "error": { "message": "gone" } })
            .to_string(),
    );
    assert!(failed.session.is_none());
    assert!(
        ids(&json!({ "jsonrpc": "2.0", "id": "blazar-prompt-u-3", "result": { "stopReason": "end_turn" } }).to_string())
            .prompt_done
    );
    assert!(
        ids(
            &json!({ "jsonrpc": "2.0", "id": "u-9", "method": "fs/read_text_file", "params": {} })
                .to_string()
        )
        .request
    );
    assert_eq!(ids("noise"), Ids::default());
    let other = json!({ "jsonrpc": "2.0", "id": 7, "method": "fs/read_text_file", "params": {} })
        .to_string();
    let reply: Value = serde_json::from_str(&unsupported_reply(&other).unwrap()).unwrap();
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["error"]["code"], -32601);
    assert!(
        unsupported_reply(
            &json!({ "jsonrpc": "2.0", "id": "p", "method": PERMISSION }).to_string()
        )
        .is_none(),
        "审批等用户来答"
    );
}
