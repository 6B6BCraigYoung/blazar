use blazar_core_types::EntryKind;

use super::synthetic::{Prng, chaos, realistic, row};
use super::*;
use serde_json::json;

fn same(a: &Transcript, b: &Transcript) -> Result<(), String> {
    if a.items.len() != b.items.len() {
        return Err(format!("item count {} vs {}", a.items.len(), b.items.len()));
    }
    for (i, (x, y)) in a.items.iter().zip(&b.items).enumerate() {
        if **x != **y {
            return Err(format!("item {i} differs:\n{x:#?}\nvs\n{y:#?}"));
        }
        if a.item(&x.key) != b.item(&x.key) {
            return Err(format!("lookup of {} differs", x.key));
        }
    }
    if a.pending != b.pending {
        return Err(format!("pending {:?} vs {:?}", a.pending, b.pending));
    }
    if a.todos != b.todos {
        return Err("todos differ".into());
    }
    if a.mcp != b.mcp {
        return Err("session info differs".into());
    }
    if format!("{:?}", a.rate) != format!("{:?}", b.rate) {
        return Err("rate differs".into());
    }
    if a.last_session != b.last_session {
        return Err("last session differs".into());
    }
    if a.turn_tokens != b.turn_tokens {
        return Err(format!(
            "turn tokens {} vs {}",
            a.turn_tokens, b.turn_tokens
        ));
    }
    Ok(())
}

fn mutate(rng: &mut Prng, r: &mut Row) {
    match rng.below(4) {
        0 => r.rewound = !r.rewound,
        1 => {
            r.elided = !r.elided;
            if let EntryKind::ToolResult { content, .. } | EntryKind::Thinking { text: content } =
                &mut r.kind
            {
                if r.elided {
                    content.clear();
                } else {
                    content.push_str("loaded");
                }
            }
        }
        2 => {
            if let EntryKind::UserMessage { text }
            | EntryKind::AssistantMessage { text }
            | EntryKind::ToolResult { content: text, .. } = &mut r.kind
            {
                text.push('!');
            }
        }
        _ => r.parent = None,
    }
}

fn check(seed: u64, rows: &mut [Row], m: &mut Model) {
    let mut rng = Prng::new(seed ^ 0x5bd1_e995);
    let mut fed = 0;
    let mut epoch = 0u64;
    let mut steps = 0;
    while fed < rows.len() || steps == 0 {
        steps += 1;
        if !rng.chance(0.1) {
            fed = (fed + rng.range(1, 24)).min(rows.len());
        }
        if fed > 0 && rng.chance(0.04) {
            let i = rng.below(fed);
            mutate(&mut rng, &mut rows[i]);
            epoch += 1;
        }
        let running = rng.chance(0.7);
        let inc = m.update(&rows[..fed], epoch, running);
        let full = build(&rows[..fed], "/w", running);
        if let Err(e) = same(&inc, &full) {
            panic!("seed {seed} fed {fed} running {running}: {e}");
        }
    }
}

#[test]
fn incremental_build_matches_full_rebuild_on_chaotic_rows() {
    for seed in 0..80 {
        let mut rows = chaos(seed, 400);
        check(seed, &mut rows, &mut Model::new("/w"));
    }
}

#[test]
fn incremental_build_matches_full_rebuild_on_realistic_rows() {
    for seed in 0..12 {
        let mut rows = realistic(seed, 700);
        let mut m = Model::new("/w");
        check(seed, &mut rows, &mut m);
        assert!(
            m.frozen_rows() > rows.len() / 2,
            "seed {seed}: only {} of {} rows were frozen",
            m.frozen_rows(),
            rows.len()
        );
    }
}

#[test]
fn stale_prefixes_are_detected_without_an_epoch_bump() {
    let mut m = Model::new("/w");
    let a = realistic(3, 400);
    let b = realistic(4, 400);
    let t = m.update(&a, 0, false);
    assert!(same(&t, &build(&a, "/w", false)).is_ok());
    assert!(m.frozen_rows() > 0);
    for n in [(a.len() - 5).min(b.len()), b.len(), 10] {
        let t = m.update(&b[..n], 0, true);
        same(&t, &build(&b[..n], "/w", true)).unwrap();
    }
    let t = m.update(&[], 0, true);
    assert!(t.items.is_empty() && t.last_session.is_none());
}

#[test]
fn rows_that_touch_frozen_items_fall_back_to_a_full_rebuild() {
    let base = |running: bool| {
        vec![
            row("s", 1, 1000, None, false, false, json!({"type":"user_message","text":"go"})),
            row(
                "s",
                2,
                2000,
                None,
                false,
                false,
                json!({"type":"tool_use","id":"t1","name":"Task","input":{"description":"look"}}),
            ),
            row(
                "s",
                3,
                3000,
                None,
                false,
                false,
                json!({"type":"tool_result","id":"t1","ok":true,"content":"done"}),
            ),
            row("s", 4, 4000, None, false, false, json!({"type":"assistant_message","text":"ok"})),
            row(
                "s",
                5,
                5000,
                None,
                false,
                false,
                json!({"type":"approval","id":"01a0fa80-9ab8-7320-bd31-000000000001","request":{"tool_name":"Bash","input":{"command":"ls"}}}),
            ),
            row(
                "s",
                6,
                6000,
                None,
                false,
                false,
                json!({"type":"approval_resolved","id":"01a0fa80-9ab8-7320-bd31-000000000001","decision":{"kind":"allow"}}),
            ),
            row("s", 7, 7000, None, false, false, json!({"type":"assistant_message","text":"fine"})),
            row("s", 8, 8000, None, false, false, json!({"type":"finished","status":"success","text":"fine"})),
            row("s2", 1, 9000, None, false, false, json!({"type":"user_message","text":"next"})),
        ]
        .into_iter()
        .chain(std::iter::once(row(
            "s2",
            2,
            9500,
            None,
            false,
            false,
            json!({"type":"progress","message_id":"m","output_tokens": u64::from(running)}),
        )))
        .collect::<Vec<Row>>()
    };
    let late = [
        row(
            "s2",
            3,
            9600,
            Some("t1"),
            false,
            false,
            json!({"type":"thinking","text":"late child"}),
        ),
        row(
            "s2",
            3,
            9600,
            None,
            false,
            false,
            json!({"type":"tool_result","id":"t1","ok":false,"content":"again"}),
        ),
        row(
            "s2",
            3,
            9600,
            None,
            false,
            false,
            json!({"type":"approval_resolved","id":"01a0fa80-9ab8-7320-bd31-000000000001","decision":{"kind":"deny","message":"no"}}),
        ),
        row(
            "s2",
            3,
            9600,
            Some("t1"),
            false,
            false,
            json!({"type":"tool_use","id":"t2","name":"Read","input":{"file_path":"/w/x"}}),
        ),
    ];
    for extra in late {
        let mut m = Model::new("/w");
        let mut rows = base(true);
        m.update(&rows, 0, true);
        assert!(m.frozen_rows() >= 9, "frozen {}", m.frozen_rows());
        rows.push(extra);
        let inc = m.update(&rows, 0, true);
        same(&inc, &build(&rows, "/w", true)).unwrap();
        let mut rows = base(false);
        let inc = m.update(&rows, 1, false);
        same(&inc, &build(&rows, "/w", false)).unwrap();
        rows.push(row(
            "s2",
            3,
            9700,
            None,
            false,
            false,
            json!({"type":"finished","status":"interrupted"}),
        ));
        let inc = m.update(&rows, 1, false);
        same(&inc, &build(&rows, "/w", false)).unwrap();
    }
}

#[test]
fn a_late_denial_decision_reaches_a_frozen_card() {
    let refused = "Permission for this action was denied by the Claude Code auto mode classifier. Reason: [Remote Shell Writes].";
    let mut rows = vec![
        row(
            "s",
            1,
            1000,
            None,
            false,
            false,
            json!({"type":"session_started","provider_session_id":"p","model":"m","cwd":"/w","permission_mode":"auto"}),
        ),
        row(
            "s",
            2,
            2000,
            None,
            false,
            false,
            json!({"type":"user_message","text":"go"}),
        ),
        row(
            "s",
            3,
            3000,
            None,
            false,
            false,
            json!({"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ssh edge.invalid 'systemctl restart x'"}}),
        ),
        row(
            "s",
            4,
            4000,
            None,
            false,
            false,
            json!({"type":"tool_result","id":"t1","ok":false,"content":refused}),
        ),
        row(
            "s",
            5,
            5000,
            None,
            false,
            false,
            json!({"type":"finished","status":"success","denied":["Bash: ssh edge.invalid 'systemctl restart x'"],
                "denials":[{"tool_name":"Bash","tool_use_id":"t1","input":{"command":"ssh edge.invalid 'systemctl restart x'"}}]}),
        ),
    ];
    let mut m = Model::new("/w");
    let first = m.update(&rows, 0, false);
    same(&first, &build(&rows, "/w", false)).unwrap();
    assert!(m.frozen_rows() >= 5, "frozen {}", m.frozen_rows());
    rows.push(row(
        "s",
        6,
        6000,
        None,
        false,
        false,
        json!({"type":"denial_decided","tool_use_id":"t1","allow":true,"by":"me","what":"ssh edge.invalid 'systemctl restart x'"}),
    ));
    let inc = m.update(&rows, 0, false);
    same(&inc, &build(&rows, "/w", false)).unwrap();
    let decided = inc.items.iter().any(|i| {
        matches!(&i.body, Body::Warn { calls, .. } if calls[0].decided == Some((true, "me".into())))
    });
    assert!(decided, "{:#?}", inc.items);
    for (seq, kind) in [
        (
            1,
            json!({"type":"user_message","text":"I approve the command that was just blocked."}),
        ),
        (
            2,
            json!({"type":"tool_use","id":"t2","name":"Bash","input":{"command":"ssh edge.invalid 'systemctl restart x'"}}),
        ),
        (
            3,
            json!({"type":"tool_result","id":"t2","ok":false,"content":refused}),
        ),
        (
            4,
            json!({"type":"finished","status":"success","denied":["Bash: ssh edge.invalid 'systemctl restart x'"],
            "denials":[{"tool_name":"Bash","tool_use_id":"t2","input":{"command":"ssh edge.invalid 'systemctl restart x'"}}]}),
        ),
    ] {
        rows.push(row("s2", seq, 7000 + seq as i64, None, false, false, kind));
        let inc = m.update(&rows, 0, seq < 4);
        same(&inc, &build(&rows, "/w", seq < 4)).unwrap();
    }
    let hard = m.update(&rows, 0, false).items.iter().any(
        |i| matches!(&i.body, Body::Warn { calls, .. } if calls[0].sid == "s2" && calls[0].hard),
    );
    assert!(hard);
}
