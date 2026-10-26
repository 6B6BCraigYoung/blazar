use super::*;
use serde_json::json;

use crate::chat_model::{
    Body, Step, always_rule, approval_choices, approval_title, first_open, step_sig,
};

fn row(sid: &str, seq: u64, ts: i64, kind: Value) -> Row {
    Row {
        session_id: sid.into(),
        seq,
        ts,
        parent: None,
        rewound: false,
        elided: false,
        kind: serde_json::from_value(kind).unwrap(),
    }
}

#[test]
fn folds_intermediate_steps_and_pairs_results() {
    let rows = vec![
        row("s", 1, 1000, json!({"type":"user_message","text":"修一下"})),
        row("s", 2, 2000, json!({"type":"thinking","text":"想想"})),
        row(
            "s",
            3,
            3000,
            json!({"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la","description":"列目录"}}),
        ),
        row(
            "s",
            4,
            3500,
            json!({"type":"tool_result","id":"t1","ok":true,"content":"a\nb\nc\nd\ne\nf"}),
        ),
        row(
            "s",
            5,
            4000,
            json!({"type":"assistant_message","text":"好了"}),
        ),
        row(
            "s",
            6,
            4100,
            json!({"type":"token_usage","input":10,"output":5,"cost_usd":0.12}),
        ),
        row(
            "s",
            7,
            4200,
            json!({"type":"finished","status":"success","text":"好了"}),
        ),
    ];
    let t = build(&rows, "/w", false);
    assert_eq!(t.items.len(), 3, "{:#?}", t.items);
    assert!(matches!(&t.items[0].body, Body::User { first: true, .. }));
    let Body::Fold(f) = &t.items[1].body else {
        panic!()
    };
    assert!(matches!(&f.steps[0], Step::Thinking { secs: 1, .. }));
    let Step::Tool(tool) = &f.steps[1] else {
        panic!()
    };
    assert_eq!(
        (tool.label.as_str(), tool.arg.as_str(), tool.state),
        ("Bash", "ls -la", St::Ok)
    );
    assert_eq!(tool.ms, Some(500));
    assert_eq!(
        tool.result.as_ref().map(|r| (r.shown.as_str(), r.more)),
        Some(("成功 · 0.5 秒", 6))
    );
    assert!(
        matches!(&t.items[2].body, Body::Assistant { .. }),
        "最终文本和上一条回答一样，不重复"
    );
}

#[test]
fn fleet_calls_name_the_child_and_summarise_the_result() {
    let rows = vec![
        row("s", 1, 0, json!({"type":"user_message","text":"派活"})),
        row(
            "s",
            2,
            1000,
            json!({"type":"tool_use","id":"t1","name":"mcp__blazar-fleet__send_prompt",
                "input":{"workspace_id":"w1","text":"\n  run the third pass\nthen report"}}),
        ),
        row(
            "s",
            3,
            1500,
            json!({"type":"tool_result","id":"t1","ok":true,"content":"{\"session_id\":\"x\"}"}),
        ),
        row(
            "s",
            4,
            2000,
            json!({"type":"tool_use","id":"t2","name":"mcp__blazar-fleet__wait_workspace",
                "input":{"workspace_id":"w1"}}),
        ),
        row(
            "s",
            5,
            2500,
            json!({"type":"tool_result","id":"t2","ok":true,
                "content":"{\"reached\":true,\"activity\":\"completed\",\"last_reply\":\"loss 0.82\"}"}),
        ),
        row(
            "s",
            6,
            3000,
            json!({"type":"assistant_message","text":"好了"}),
        ),
    ];
    let t = build(&rows, "/w", false);
    let Body::Fold(f) = &t.items[1].body else {
        panic!("{:#?}", t.items)
    };
    let tools: Vec<&Tool> = f
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Tool(t) => Some(t.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(
        (
            tools[0].label.as_str(),
            tools[0].tail.as_str(),
            tools[0].arg.as_str()
        ),
        ("派活给", "", "run the third pass")
    );
    assert_eq!(tools[0].extra, Extra::Fleet("w1".into()));
    assert_eq!(
        tools[0].result.as_ref().map(|r| r.shown.as_str()),
        Some("已派出")
    );
    let raw = tools[0].raw.as_deref().unwrap_or_default();
    assert!(
        raw.starts_with("输入\n{")
            && raw.contains("\"workspace_id\": \"w1\"")
            && raw.contains("\n输出\n{\n  \"session_id\": \"x\"\n}"),
        "{raw}"
    );
    assert_eq!(
        (tools[1].label.as_str(), tools[1].tail.as_str()),
        ("等", "停下")
    );
    assert_eq!(tools[1].extra, Extra::Fleet("w1".into()));
    assert_eq!(
        tools[1].result.as_ref().map(|r| r.shown.as_str()),
        Some("已完成\nloss 0.82")
    );
}

#[test]
fn live_fold_and_pending_approval() {
    let rows = vec![
        row("s", 1, 0, json!({"type":"user_message","text":"x"})),
        row(
            "s",
            2,
            0,
            json!({"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/w/src/a.rs","old_string":"a","new_string":"b"}}),
        ),
        row(
            "s",
            3,
            0,
            json!({"type":"approval","id":"01a0fa80-9ab8-7320-bd31-48d9174e2bcd","request":{"tool_name":"Edit","input":{"file_path":"/w/src/a.rs"}}}),
        ),
    ];
    let t = build(&rows, "/w", true);
    let Body::Fold(f) = &t.items[1].body else {
        panic!()
    };
    assert!(f.live);
    assert_eq!(t.pending.len(), 1);
    assert_eq!(
        approval_title(&t.pending[0].request, "/w", None),
        "允许编辑 src/a.rs？"
    );
    let Step::Tool(tool) = &f.steps[0] else {
        panic!()
    };
    assert_eq!(
        tool.extra,
        Extra::Diff {
            rows: vec![('-', "a".into()), ('+', "b".into())],
            more: 0,
            added: 1,
            removed: 1,
            start: None,
        }
    );

    let mut rows2 = rows.clone();
    rows2.push(row("s", 4, 0, json!({"type":"approval_resolved","id":"01a0fa80-9ab8-7320-bd31-48d9174e2bcd","decision":{"kind":"allow"}})));
    let t2 = build(&rows2, "/w", true);
    assert!(t2.pending.is_empty());
    let Body::Fold(f2) = &t2.items[1].body else {
        panic!()
    };
    assert!(matches!(&f2.steps[1], Step::Note(n) if n == "已允许 · Edit"));
    assert_ne!(t.items[1].sig, t2.items[1].sig, "内容变了签名要变");
    assert_eq!(t.items[0].sig, t2.items[0].sig, "没变的条目签名不变");
}

#[test]
fn live_turn_shows_its_token_count() {
    let rows = vec![
        row("s", 1, 0, json!({"type":"user_message","text":"x"})),
        row(
            "s",
            2,
            0,
            json!({"type":"progress","message_id":"m1","output_tokens":800}),
        ),
        row("s", 3, 0, json!({"type":"thinking","text":""})),
        row(
            "s",
            4,
            0,
            json!({"type":"progress","message_id":"m1","output_tokens":900}),
        ),
        row(
            "s",
            5,
            0,
            json!({"type":"progress","message_id":"m2","output_tokens":300}),
        ),
    ];
    let t = build(&rows, "", true);
    assert_eq!(t.turn_tokens, 1200, "每次调用取最大值再加总");
    assert!(matches!(&t.items[1].body, Body::Fold(f) if f.live));
    let mut more = rows.clone();
    more.push(row(
        "s",
        6,
        0,
        json!({"type":"thinking","text":"x".repeat(8000)}),
    ));
    assert_eq!(
        build(&more, "", true).turn_tokens,
        2000,
        "上报的偏小时用估算值"
    );
}

#[test]
fn subagent_steps_nest_under_task_and_rewound_turns_group() {
    let mut sub = row(
        "s",
        3,
        0,
        json!({"type":"tool_use","id":"c1","name":"Read","input":{"file_path":"/w/x.rs"}}),
    );
    sub.parent = Some("task1".into());
    let mut rw = row("s", 1, 0, json!({"type":"user_message","text":"旧的"}));
    rw.rewound = true;
    let rows = vec![
        rw,
        row(
            "s",
            2,
            0,
            json!({"type":"tool_use","id":"task1","name":"Task","input":{"description":"查一下"}}),
        ),
        sub,
    ];
    let t = build(&rows, "/w", false);
    assert!(matches!(&t.items[0].body, Body::Rewound { turns: 1, items } if items.len() == 1));
    let Body::Fold(f) = &t.items[1].body else {
        panic!("{:#?}", t.items)
    };
    let Step::Tool(task) = &f.steps[0] else {
        panic!()
    };
    assert_eq!((task.sub_calls, task.sub_last.as_str()), (1, "Read x.rs"));
    assert_eq!(task.sub.len(), 1);
}

#[test]
fn elided_subagent_rows_hide_output_until_full_rows_replace_them() {
    let sub = |seq, kind: Value, elided| {
        let mut r = row("s", seq, 0, kind);
        r.parent = Some("task1".into());
        r.elided = elided;
        r
    };
    let rows = |elided: bool| {
        vec![
            row(
                "s",
                1,
                0,
                json!({"type":"tool_use","id":"task1","name":"Task","input":{"description":"查一下"}}),
            ),
            sub(
                2,
                json!({"type":"tool_use","id":"c1","name":"Bash","input":{"command":"ls"}}),
                false,
            ),
            sub(
                3,
                json!({"type":"thinking","text": if elided { "" } else { "想一想" }}),
                elided,
            ),
            sub(
                4,
                json!({"type":"tool_result","id":"c1","ok":false,"content": if elided { "" } else { "a\nb" }}),
                elided,
            ),
        ]
    };
    let task = |rows: &[Row]| {
        let t = build(rows, "/w", false);
        let Body::Fold(f) = &t.items[0].body else {
            panic!("{:#?}", t.items)
        };
        let Step::Tool(task) = &f.steps[0] else {
            panic!()
        };
        (**task).clone()
    };

    let thin = task(&rows(true));
    assert!(thin.sub_elided);
    assert_eq!(thin.sub.len(), 1);
    let Step::Tool(call) = &thin.sub[0] else {
        panic!()
    };
    assert_eq!((call.state, call.result.is_none()), (St::Err, true));

    let full = task(&rows(false));
    assert!(!full.sub_elided);
    assert_eq!(full.sub.len(), 2);
    let Step::Tool(call) = &full.sub[0] else {
        panic!()
    };
    assert_eq!(call.state, St::Err);
    assert_eq!(
        call.result.as_ref().map(|r| r.full.as_str()),
        Some("失败\na\nb")
    );
    assert_ne!(
        step_sig(&Step::Tool(Box::new(thin))),
        step_sig(&Step::Tool(Box::new(full)))
    );
}

#[test]
fn failure_offers_account_switch() {
    let rows = vec![row(
        "s",
        1,
        0,
        json!({"type":"finished","status":"failed","message":"API Error: 429 rate_limit_error"}),
    )];
    let t = build(&rows, "", false);
    assert!(matches!(
        &t.items[0].body,
        Body::Error {
            by_account: true,
            ..
        }
    ));
    assert_eq!(
        always_rule(&json!({"tool_name":"Bash","input":{"command":"cargo test -p x"}})),
        ("Bash".into(), "cargo test".into())
    );
}

#[test]
fn api_errors_read_as_plain_words() {
    assert!(human_error("API Error: 529 {\"type\":\"overloaded_error\"}").starts_with("服务繁忙"));
    assert!(human_error("API Error: 429 rate limit").starts_with("请求太频繁"));
    assert!(human_error("401 not logged in").starts_with("登录已失效"));
    assert_eq!(human_error("disk full"), "disk full");
}

#[test]
fn a_pending_bash_approval_always_gets_a_card() {
    let rows = vec![
        row(
            "s",
            1,
            0,
            json!({"type":"user_message","text":"跑一下冒烟训练"}),
        ),
        row(
            "s",
            2,
            0,
            json!({"type":"tool_use","id":"t1","name":"Bash","input":{"command":"python train.py --steps 200"}}),
        ),
        row(
            "s",
            3,
            0,
            json!({"type":"approval","id":"01a0fa80-9ab8-7320-bd31-48d9174e2bce","request":{"tool_name":"Bash","input":{"command":"python train.py --steps 200"}}}),
        ),
    ];
    for live in [true, false] {
        let t = build(&rows, "/w", live);
        let card = first_open(&t.pending, |_| false).expect("有待审批时必须有审批卡");
        assert!(!card.ask);
        assert_eq!(
            approval_title(&card.request, "/w", None),
            "允许运行这条 Bash 命令？"
        );
        assert!(
            first_open(&t.pending, |id| id
                == "01a0fa80-9ab8-7320-bd31-48d9174e2bce")
            .is_none()
        );
    }
    let (choices, hint) = approval_choices(false);
    assert_eq!(choices, ["允许", "允许，并且这个工作区不再询问", "拒绝"]);
    assert_eq!(hint, "告诉 Claude 换个做法");
    assert_eq!(approval_choices(true).0.len(), 3);
}

#[test]
fn api_retries_show_once_and_clear_when_the_turn_moves_on() {
    let retry = |seq, attempt| {
        row(
            "s",
            seq,
            0,
            json!({"type":"api_retry","attempt":attempt,"max_retries":10,"delay_ms":4000,"status":529,"error":"server_error"}),
        )
    };
    let mut rows = vec![
        row("s", 1, 0, json!({"type":"user_message","text":"x"})),
        retry(2, 1),
        retry(3, 2),
    ];
    let t = build(&rows, "", true);
    assert_eq!(t.items.len(), 2);
    assert!(matches!(
        &t.items[1].body,
        Body::Retry { why, attempt: 2, max: 10, until: 4000 } if why == "服务繁忙（529）"
    ));
    rows.push(row(
        "s",
        4,
        0,
        json!({"type":"assistant_message","text":"好了"}),
    ));
    let t = build(&rows, "", true);
    assert_eq!(t.items.len(), 2);
    assert!(matches!(&t.items[1].body, Body::Assistant { .. }));
    let err = build(
        &[row(
            "s",
            1,
            0,
            json!({"type":"error","message":"API Error: 529 Overloaded"}),
        )],
        "",
        false,
    );
    assert!(matches!(
        &err.items[0].body,
        Body::Error { text, .. } if text == "服务繁忙，稍后再试\nAPI Error: 529 Overloaded"
    ));
}

#[test]
fn lost_resume_points_read_as_a_note_and_other_failures_carry_their_cause() {
    use blazar_core_types::failure::FailureCause;
    let lost = build(
        &[row(
            "s",
            1,
            0,
            json!({"type":"finished","status":"failed","message":"agent 异常退出（退出码 1）。stderr: No message found with message.uuid of: 9b915e37-5c1e-4a7e-9d3b-2f1c0a8e7b6d"}),
        )],
        "",
        false,
    );
    assert!(
        matches!(&lost.items[0].body, Body::Meta { bad: false, text } if text.contains("自动重新接上")),
        "{:?}",
        lost.items[0].body
    );
    let asleep = build(
        &[row(
            "s",
            1,
            0,
            json!({"type":"finished","status":"failed","message":"API Error: Connection lost while your computer was asleep"}),
        )],
        "",
        false,
    );
    assert!(matches!(
        &asleep.items[0].body,
        Body::Error {
            cause: FailureCause::Network,
            ..
        }
    ));
    let signed_out = build(
        &[row(
            "s",
            1,
            0,
            json!({"type":"error","message":"authentication_failed：Not logged in · Please run /login"}),
        )],
        "",
        false,
    );
    assert!(matches!(
        &signed_out.items[0].body,
        Body::Error {
            cause: FailureCause::Login,
            by_account: true,
            ..
        }
    ));
}
