use super::*;

fn channel(id: &str, runtime: &str, label: &str, account: Option<&str>) -> Channel {
    Channel {
        id: id.into(),
        label: label.into(),
        runtime: runtime.into(),
        account: account.map(str::to_owned),
        metered: account.is_some(),
        images: true,
        installed: None,
    }
}

#[test]
fn claude_offers_each_model_family_with_a_plain_description() {
    let got = shapes(None, &channel("claude", "claude", "Claude Code", None));
    let labels: Vec<&str> = got.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Claude Code · Opus",
            "Claude Code · Fable",
            "Claude Code · Sonnet",
            "Claude Code · Haiku"
        ]
    );
    assert_eq!(got[2].1.as_deref(), Some("sonnet"));
    assert!(got[1].3.contains("most capable"));
    assert!(got[3].3.contains("fastest"));
}

#[test]
fn codex_offers_three_efforts_on_its_configured_model() {
    let got = shapes(None, &channel("codex", "codex", "Codex", None));
    let efforts: Vec<(String, Option<String>, Option<String>)> = got
        .iter()
        .take(3)
        .map(|s| (s.0.clone(), s.1.clone(), s.2.clone()))
        .collect();
    assert_eq!(
        efforts,
        [
            ("Codex · low effort".into(), None, Some("low".into())),
            ("Codex · medium effort".into(), None, Some("medium".into())),
            ("Codex · high effort".into(), None, Some("high".into())),
        ]
    );
    assert!(got[2].3.contains("deep reasoning"));
}

#[test]
fn an_api_account_offers_its_models_and_marks_the_small_one_cheap() {
    let e = crate::accounts::Endpoint {
        base_url: "https://api.example.com/anthropic".into(),
        models: vec!["big-pro".into(), "small-flash".into()],
        default_model: "big-pro".into(),
        small_model: Some("small-flash".into()),
        preset: None,
        images: false,
    };
    let got = shapes(Some(&e), &channel("acc:x", "claude", "Example", Some("x")));
    assert_eq!(got[0].0, "Example · big-pro");
    assert_eq!(got[1].1.as_deref(), Some("small-flash"));
    assert!(got[0].3.contains("capable general"));
    assert!(got[1].3.contains("cheap and fast"));
}

#[test]
fn agents_that_pick_their_own_model_are_one_option_each() {
    for rt in ["devin", "droid", "grok"] {
        let got = shapes(None, &channel(rt, rt, rt, None));
        assert_eq!(got.len(), 1, "{rt}");
        assert_eq!(got[0].1, None, "{rt}");
    }
}

#[test]
fn setups_rank_from_lightest_to_strongest() {
    assert_eq!(strength("claude", Some("haiku"), None, None), 1);
    assert_eq!(strength("claude", Some("sonnet"), None, None), 2);
    assert_eq!(strength("claude", Some("opus"), None, None), 3);
    assert_eq!(strength("claude", Some("claude-fable-5-2"), None, None), 4);
    assert_eq!(strength("codex", None, Some("low"), None), 1);
    assert_eq!(strength("codex", None, Some("medium"), None), 2);
    assert_eq!(strength("codex", None, Some("high"), None), 3);
    assert_eq!(
        strength("claude", Some("small-flash"), None, Some("small-flash")),
        1
    );
    assert_eq!(
        strength("claude", Some("big-pro"), None, Some("small-flash")),
        2
    );
    assert_eq!(strength("devin", None, None, None), 2);
}
