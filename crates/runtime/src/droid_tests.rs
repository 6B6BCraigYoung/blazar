use blazar_core_types::{EntryKind, Outcome};
use serde_json::json;

use super::{Turn, parse_line, tool};

fn feed(turn: &mut Turn, v: &serde_json::Value) -> Vec<EntryKind> {
    parse_line(turn, &v.to_string())
}

#[test]
fn the_session_starts_from_the_init_event() {
    let mut turn = Turn::default();
    let out = feed(
        &mut turn,
        &json!({
            "type": "system", "subtype": "init", "cwd": "/private/tmp/probe",
            "session_id": "de735edc-af74-46c2-9c58-2d2356a13459",
            "tools": ["Read", "LS", "Execute"],
            "model": "custom:chatgpt/gpt-6-astra", "reasoning_effort": "high"
        }),
    );
    let [
        EntryKind::SessionStarted {
            provider_session_id,
            model,
            cwd,
            ..
        },
    ] = out.as_slice()
    else {
        panic!("{out:?}");
    };
    assert_eq!(
        provider_session_id.0,
        "de735edc-af74-46c2-9c58-2d2356a13459"
    );
    assert_eq!(model.as_deref(), Some("custom:chatgpt/gpt-6-astra"));
    assert_eq!(cwd.as_deref(), Some("/private/tmp/probe"));
}

#[test]
fn the_user_echo_is_not_shown_twice() {
    let mut turn = Turn::default();
    let out = feed(
        &mut turn,
        &json!({ "type": "message", "role": "user", "id": "u", "text": "Reply with exactly: OK",
                 "timestamp": 1, "session_id": "s" }),
    );
    assert!(out.is_empty());
    let out = feed(
        &mut turn,
        &json!({ "type": "message", "role": "assistant", "id": "a", "text": "OK",
                 "timestamp": 2, "session_id": "s" }),
    );
    assert!(matches!(out.as_slice(), [EntryKind::AssistantMessage { text }] if text == "OK"));
}

#[test]
fn a_failed_turn_keeps_the_reason_droid_gave() {
    let mut turn = Turn::default();
    let reason = "The ChatGPT user has reached their Subscription Sharing usage limit. Ask the user to try again after their usage limit resets or use an API key instead.";
    let first = feed(
        &mut turn,
        &json!({ "type": "error", "source": "agent_loop", "message": reason, "timestamp": 1, "session_id": "s" }),
    );
    assert!(matches!(first.as_slice(), [EntryKind::Error { message }] if message == reason));
    let last = feed(
        &mut turn,
        &json!({ "type": "error", "source": "cli", "message": "OpenAI stream error event (invalid_request_error)",
                 "timestamp": 2, "session_id": "s" }),
    );
    assert!(
        matches!(last.as_slice(), [EntryKind::Finished(Outcome::Failed { message })] if message == reason),
        "笼统的 cli 报错换成 agent_loop 说的原因：{last:?}"
    );
    let alone = feed(
        &mut turn,
        &json!({ "type": "error", "source": "cli", "message": "402 No active subscription found.", "timestamp": 3 }),
    );
    assert!(
        matches!(alone.as_slice(), [EntryKind::Finished(Outcome::Failed { message })] if message == "402 No active subscription found.")
    );
}

#[test]
fn tool_results_pair_with_their_calls() {
    let mut turn = Turn::default();
    let call = |id: &str, file: &str| {
        json!({ "type": "tool_call", "id": id, "messageId": "93696540", "toolId": "Read",
                "toolName": "Read", "parameters": { "file_path": file },
                "timestamp": 1_794_644_304_196_u64, "session_id": "38f5b017" })
    };
    let result = |id: Option<&str>, value: &str| {
        let mut v = json!({ "type": "tool_result", "messageId": "cd1b6579", "toolId": "Read",
                            "isError": false, "value": value, "timestamp": 1_794_644_304_208_u64,
                            "session_id": "38f5b017" });
        if let Some(id) = id {
            v["id"] = json!(id);
        }
        v
    };
    let a = feed(
        &mut turn,
        &call("call_52dd03fad1285159_0", "/repo/note.txt"),
    );
    let b = feed(
        &mut turn,
        &call("call_52dd03fad1285159_1", "/repo/other.txt"),
    );
    assert!(
        matches!(a.as_slice(), [EntryKind::ToolUse { id, name, input }] if id.0 == "call_52dd03fad1285159_0" && name == "Read" && input["file_path"] == "/repo/note.txt"),
        "toolId 是工具类型，id 才是这次调用：{a:?}"
    );
    assert!(
        matches!(b.as_slice(), [EntryKind::ToolUse { id, .. }] if id.0 == "call_52dd03fad1285159_1")
    );
    let second = feed(
        &mut turn,
        &result(Some("call_52dd03fad1285159_1"), "other\n"),
    );
    assert!(
        matches!(second.as_slice(), [EntryKind::ToolResult { id, ok: true, content, .. }] if id.0 == "call_52dd03fad1285159_1" && content == "other\n")
    );
    let first = feed(&mut turn, &result(None, "banana\n"));
    assert!(
        matches!(first.as_slice(), [EntryKind::ToolResult { id, content, .. }] if id.0 == "call_52dd03fad1285159_0" && content == "banana\n"),
        "结果不带 id 时按先后顺序配给最早那个：{first:?}"
    );
    let failed = feed(
        &mut turn,
        &json!({ "type": "tool_result", "id": "call_x", "toolId": "Execute", "isError": true,
                 "error": { "type": "permission", "message": "Not allowed in read-only mode" },
                 "timestamp": 3, "session_id": "s" }),
    );
    assert!(
        matches!(failed.as_slice(), [EntryKind::ToolResult { ok: false, content, .. }] if content == "Not allowed in read-only mode")
    );
}

#[test]
fn reasoning_shows_once_even_when_droid_repeats_it() {
    let mut turn = Turn::default();
    let thought = json!({ "type": "reasoning", "id": "fb9400d1", "text": "The user asks to reply exactly OK.",
                          "timestamp": 1_794_644_284_337_u64, "session_id": "a7a7e4bd" });
    let first = feed(&mut turn, &thought);
    assert!(
        matches!(first.as_slice(), [EntryKind::Thinking { text }] if text == "The user asks to reply exactly OK.")
    );
    assert!(feed(&mut turn, &thought).is_empty());
}

#[test]
fn droid_tools_read_like_the_ones_blazar_already_shows() {
    assert_eq!(
        tool(
            "Create",
            &json!({ "file_path": "a.rs", "content": "fn main() {}" })
        ),
        (
            "Write".to_owned(),
            json!({ "file_path": "a.rs", "content": "fn main() {}" })
        )
    );
    assert_eq!(
        tool(
            "Edit",
            &json!({ "path": "a.rs", "old_str": "x", "new_str": "y" })
        ),
        (
            "Edit".to_owned(),
            json!({ "file_path": "a.rs", "old_string": "x", "new_string": "y" })
        )
    );
    assert_eq!(
        tool(
            "Glob",
            &json!({ "folder": "/repo", "patterns": ["**/*.rs", "*.toml"] })
        ),
        (
            "Glob".to_owned(),
            json!({ "pattern": "**/*.rs *.toml", "path": "/repo" })
        )
    );
    assert_eq!(
        tool("FetchUrl", &json!({ "url": "https://example.com" })).0,
        "WebFetch"
    );
    assert_eq!(
        tool("LS", &json!({ "directory_path": "/repo" })),
        ("LS".to_owned(), json!({ "path": "/repo" }))
    );
    let odd = json!({ "agent": "x" });
    assert_eq!(
        tool("SpawnAgent", &odd),
        ("SpawnAgent".to_owned(), odd.clone())
    );
    assert_eq!(
        tool("Read", &odd),
        ("Read".to_owned(), odd),
        "缺了关键参数就原样保留"
    );
}

#[test]
fn completion_finishes_the_turn_with_its_usage() {
    let mut turn = Turn::default();
    let out = feed(
        &mut turn,
        &json!({ "type": "completion", "finalText": "OK", "numTurns": 1, "durationMs": 1373,
                 "session_id": "a7a7e4bd", "timestamp": 1_794_644_284_354_u64,
                 "usage": { "input_tokens": 8423, "output_tokens": 39, "cache_read_input_tokens": 16512,
                            "cache_creation_input_tokens": 0, "factory_credits": 0,
                            "thinking_tokens": 0, "ttft_ms": 672 } }),
    );
    let [
        EntryKind::TokenUsage(u),
        EntryKind::Finished(Outcome::Success {
            text: Some(t),
            usage: Some(again),
            ..
        }),
    ] = out.as_slice()
    else {
        panic!("{out:?}");
    };
    assert_eq!((u.input, u.output, u.cache_read), (8423, 39, 16512));
    assert_eq!(u.cost_usd, None, "Factory 不给估价，也不按 Claude 价格算");
    assert_eq!(again.output, 39);
    assert_eq!(t, "OK");
    assert!(parse_line(&mut turn, "not json").is_empty());
}
