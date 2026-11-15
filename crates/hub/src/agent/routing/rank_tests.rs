use super::*;

fn verdict(difficulty: f64, kind: &str) -> Verdict {
    Verdict {
        difficulty,
        difficulty_confidence: 0.9,
        kind: kind.into(),
        kind_confidence: 0.9,
        model: None,
        ms: 0,
        cost_usd: None,
    }
}

fn cand(channel: &str, label: &str, peak: Option<f64>, first: bool) -> Candidate {
    let metered = channel.starts_with("acc:");
    Candidate {
        channel: channel.into(),
        runtime: if metered {
            "claude".into()
        } else {
            channel.into()
        },
        account: metered.then(|| channel.trim_start_matches("acc:").to_owned()),
        label: label.into(),
        peak,
        strong: !metered && channel != "droid",
        metered,
        first,
    }
}

#[test]
fn difficulty_sets_the_tier_and_the_kind_nudges_it() {
    assert_eq!(tier_of(None), Tier::Medium);
    assert_eq!(tier_of(Some(&verdict(0.4, "fix"))), Tier::Light);
    assert_eq!(tier_of(Some(&verdict(2.0, "feature"))), Tier::Medium);
    assert_eq!(tier_of(Some(&verdict(3.6, "feature"))), Tier::Heavy);
    assert_eq!(tier_of(Some(&verdict(3.6, "research"))), Tier::Medium);
    assert_eq!(tier_of(Some(&verdict(0.2, "refactor"))), Tier::Medium);
    let mut unsure = verdict(3.9, "feature");
    unsure.difficulty_confidence = 0.2;
    assert_eq!(tier_of(Some(&unsure)), Tier::Medium);
    let mut vague = verdict(3.6, "research");
    vague.kind_confidence = 0.1;
    assert_eq!(tier_of(Some(&vague)), Tier::Heavy);
}

#[test]
fn the_preferred_channel_wins_until_its_usage_climbs() {
    let line = 0.85;
    let calm = [
        cand("claude", "Claude Code", Some(0.3), true),
        cand("codex", "Codex", Some(0.05), false),
        cand("devin", "Devin", Some(0.02), false),
    ];
    assert_eq!(best(&calm, Tier::Medium, line).unwrap().channel, "claude");
    let busy = [
        cand("claude", "Claude Code", Some(0.75), true),
        cand("codex", "Codex", Some(0.05), false),
        cand("devin", "Devin", Some(0.02), false),
    ];
    assert_eq!(best(&busy, Tier::Medium, line).unwrap().channel, "devin");
    let unknown = [
        cand("codex", "Codex", None, false),
        cand("devin", "Devin", None, false),
    ];
    assert_eq!(best(&unknown, Tier::Medium, line).unwrap().channel, "codex");
    assert!(best(&[], Tier::Medium, line).is_none());
}

#[test]
fn metered_and_weaker_channels_only_win_light_work() {
    let line = 0.85;
    let pair = [
        cand("claude", "Claude Code", Some(0.75), true),
        cand("acc:deepseek", "DeepSeek", None, false),
    ];
    assert_eq!(best(&pair, Tier::Heavy, line).unwrap().channel, "claude");
    assert_eq!(best(&pair, Tier::Medium, line).unwrap().channel, "claude");
    assert_eq!(
        best(&pair, Tier::Light, line).unwrap().channel,
        "acc:deepseek"
    );
}

#[test]
fn light_and_heavy_work_tunes_claude_and_codex() {
    assert_eq!(tuning("claude", Tier::Light), (Some("sonnet"), None));
    assert_eq!(tuning("claude", Tier::Heavy), (None, None));
    assert_eq!(tuning("codex", Tier::Light), (None, Some("low")));
    assert_eq!(tuning("codex", Tier::Medium), (None, None));
    assert_eq!(tuning("codex", Tier::Heavy), (None, Some("high")));
    assert_eq!(tuning("devin", Tier::Heavy), (None, None));
    assert_eq!(tuning("acc:deepseek", Tier::Light), (None, None));
}

#[test]
fn the_note_says_who_and_why_in_one_line() {
    let claude = cand("claude", "Claude Code", Some(0.78), true);
    let codex = cand("codex", "Codex", Some(0.1), false);
    let hard = verdict(3.2, "refactor");
    assert_eq!(
        note(&Story {
            chosen: &codex,
            model: None,
            effort: Some("high"),
            judged: Some(Ok(&hard)),
            first: Some(&claude),
            first_skip: None,
            fallback: false,
        }),
        "Auto → Codex · high effort · hard refactor · Claude Code at 78% of its limit"
    );

    let over = Skip {
        channel: "claude".into(),
        label: "Claude Code".into(),
        why: Why::OverLine,
    };
    assert_eq!(
        note(&Story {
            chosen: &codex,
            model: None,
            effort: None,
            judged: Some(Err(&Failure::Credits)),
            first: None,
            first_skip: Some(&over),
            fallback: false,
        }),
        "Auto → Codex · Jev out of credits · Claude Code is over its usage line"
    );

    let calm = cand("claude", "Claude Code", Some(0.2), true);
    let tiny = verdict(0.2, "question");
    assert_eq!(
        note(&Story {
            chosen: &calm,
            model: Some("sonnet"),
            effort: None,
            judged: Some(Ok(&tiny)),
            first: Some(&calm),
            first_skip: None,
            fallback: false,
        }),
        "Auto → Claude Code · Sonnet · trivial question"
    );

    assert_eq!(
        note(&Story {
            chosen: &calm,
            model: None,
            effort: None,
            judged: None,
            first: None,
            first_skip: None,
            fallback: true,
        }),
        "Auto → Claude Code · fallback: no channel is available"
    );
}
