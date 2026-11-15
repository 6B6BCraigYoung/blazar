use std::collections::BTreeMap;

use super::*;

fn verdict(difficulty: f64, kind: &str) -> Verdict {
    Verdict {
        difficulty,
        difficulty_confidence: 0.9,
        kind: kind.into(),
        kind_confidence: 0.9,
        pick: None,
        pick_confidence: 0.0,
        probs: BTreeMap::new(),
        unhappy: None,
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
fn jev_leads_and_a_busy_account_holds_it_back() {
    let line = 0.85;
    let fresh = worth(0.55, Some(0.1), false, false, line);
    let busy = worth(0.6, Some(0.8), false, false, line);
    assert!(fresh > busy);
    assert!(worth(0.5, None, true, false, line) > worth(0.5, None, false, false, line));
    assert!(worth(0.5, None, false, true, line) < worth(0.5, None, false, false, line));
    assert!(load(Some(0.2), line) < load(Some(0.7), line));
    assert!(load(None, line) < load(Some(0.6), line));
}

#[test]
fn switching_midway_needs_a_clear_and_confident_lead() {
    assert!(should_switch(0.7, 0.2, 0.7));
    assert!(!should_switch(0.5, 0.3, 0.9));
    assert!(!should_switch(0.8, 0.1, 0.5));
}

#[test]
fn the_first_note_says_who_and_why_in_one_line() {
    let claude = cand("claude", "Claude Code", Some(0.78), true);
    let hard = verdict(3.2, "refactor");
    assert_eq!(
        note(&Story {
            label: "Codex · high effort",
            channel: "codex",
            seat: None,
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
            label: "Codex",
            channel: "codex",
            seat: None,
            judged: Some(Err(&Failure::Credits)),
            first: None,
            first_skip: Some(&over),
            fallback: false,
        }),
        "Auto → Codex · Jev out of credits · Claude Code is over its usage line"
    );

    let tiny = verdict(0.2, "question");
    assert_eq!(
        note(&Story {
            label: "Claude Code · Sonnet",
            channel: "claude",
            seat: Some("work"),
            judged: Some(Ok(&tiny)),
            first: Some(&cand("claude", "Claude Code", Some(0.2), true)),
            first_skip: None,
            fallback: false,
        }),
        "Auto → Claude Code · Sonnet · work · trivial question"
    );

    assert_eq!(
        note(&Story {
            label: "Claude Code",
            channel: "claude",
            seat: None,
            judged: None,
            first: None,
            first_skip: None,
            fallback: true,
        }),
        "Auto → Claude Code · fallback: no channel is available"
    );
}

#[test]
fn a_midway_switch_names_the_new_setup_or_the_new_account() {
    let tiny = verdict(0.3, "question");
    assert_eq!(
        shift_note(&Shift {
            label: "Claude Code · Haiku",
            seat: None,
            judged: Some(Ok(&tiny)),
            left: None,
            model_changed: true,
            unhappy: false,
        }),
        "Auto → Claude Code · Haiku · trivial question"
    );
    assert_eq!(
        shift_note(&Shift {
            label: "Claude Code · Opus",
            seat: Some("backup"),
            judged: None,
            left: Some(("work", 0.9)),
            model_changed: false,
            unhappy: false,
        }),
        "Auto → backup · work at 90% of its limit"
    );
}

#[test]
fn an_unhappy_user_gets_told_why_the_model_went_up() {
    let task = verdict(2.0, "fix");
    assert_eq!(
        shift_note(&Shift {
            label: "Claude Code · Fable",
            seat: None,
            judged: Some(Ok(&task)),
            left: None,
            model_changed: true,
            unhappy: true,
        }),
        "Auto → Claude Code · Fable · you seemed unhappy with the last result"
    );
    assert_eq!(
        top_note("Claude Code", Some("/model codex high")),
        "Already on the strongest Claude Code setup · try /model codex high"
    );
    assert_eq!(
        top_note("Devin", None),
        "Already on the strongest Devin setup"
    );
}
