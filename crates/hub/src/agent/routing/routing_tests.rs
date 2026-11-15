use super::*;

fn ch(id: &str, runtime: &str, label: &str, account: Option<&str>) -> Channel {
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

fn choice(
    channel: &str,
    runtime: &str,
    label: &str,
    model: Option<&str>,
    effort: Option<&str>,
    seats: &[(&str, &str)],
) -> Choice {
    Choice {
        key: String::new(),
        channel: channel.into(),
        runtime: runtime.into(),
        label: label.into(),
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
        about: String::new(),
        seats: seats
            .iter()
            .map(|(id, label)| Seat {
                account: Some((*id).into()),
                label: (*label).into(),
                peak: None,
                pass: true,
                blocked: Vec::new(),
            })
            .collect(),
        metered: channel.starts_with("acc:"),
        strong: true,
        first: false,
        rank: options::strength(runtime, model, effort, None),
    }
}

#[test]
fn a_typed_model_resolves_to_one_setup() {
    let all = vec![
        ch("claude", "claude", "Claude Code", None),
        ch("codex", "codex", "Codex", None),
        ch("acc:ds", "claude", "DeepSeek", Some("ds")),
    ];
    let seats = [("c1", "work"), ("c2", "claude-max")];
    let choices = vec![
        choice(
            "claude",
            "claude",
            "Claude Code · Opus",
            Some("opus"),
            None,
            &seats,
        ),
        choice(
            "claude",
            "claude",
            "Claude Code · Sonnet",
            Some("sonnet"),
            None,
            &seats,
        ),
        choice(
            "codex",
            "codex",
            "Codex · low effort",
            None,
            Some("low"),
            &[("x1", "default")],
        ),
        choice(
            "codex",
            "codex",
            "Codex · high effort",
            None,
            Some("high"),
            &[("x1", "default")],
        ),
        choice(
            "acc:ds",
            "claude",
            "DeepSeek · deepseek-v4-pro",
            Some("deepseek-v4-pro"),
            None,
            &[("ds", "DeepSeek")],
        ),
        choice(
            "acc:ds",
            "claude",
            "DeepSeek · deepseek-flash",
            Some("deepseek-flash"),
            None,
            &[("ds", "DeepSeek")],
        ),
    ];

    let r = matching(&choices, &all, "opus").unwrap();
    assert_eq!(r.agent, "r:claude");
    assert_eq!(r.model.as_deref(), Some("opus"));
    assert_eq!(r.account, None);

    let r = matching(&choices, &all, "codex high").unwrap();
    assert_eq!(r.runtime, "codex");
    assert_eq!(r.effort.as_deref(), Some("high"));
    assert_eq!(r.label, "Codex · high effort");

    let r = matching(&choices, &all, "codex").unwrap();
    assert_eq!((r.model, r.effort), (None, None));
    assert_eq!(r.label, "Codex");

    let r = matching(&choices, &all, "deepseek flash").unwrap();
    assert_eq!(r.model.as_deref(), Some("deepseek-flash"));
    assert_eq!(r.account.as_deref(), Some("ds"));

    let r = matching(&choices, &all, "sonnet max").unwrap();
    assert_eq!(r.model.as_deref(), Some("sonnet"));
    assert_eq!(r.account.as_deref(), Some("c2"));

    assert_eq!(
        matching(&choices, &all, "codex 高")
            .unwrap()
            .effort
            .as_deref(),
        Some("high")
    );
    assert_eq!(matching(&choices, &all, "Auto").unwrap().agent, "r:auto");
    assert!(matching(&choices, &all, "gemini").is_none());
    assert!(matching(&choices, &all, "   ").is_none());
}

#[test]
fn a_followup_recognises_the_setup_it_is_already_on() {
    let opus = choice(
        "claude",
        "claude",
        "Claude Code · Opus",
        Some("opus"),
        None,
        &[],
    );
    let low = choice(
        "codex",
        "codex",
        "Codex · low effort",
        None,
        Some("low"),
        &[],
    );
    let routed = Current {
        model: Some("opus".into()),
        ..Current::default()
    };
    assert!(same(&opus, &routed, false));
    let plain = Current {
        actual: Some("claude-opus-5-5".into()),
        ..Current::default()
    };
    assert!(same(&opus, &plain, false));
    assert!(!same(&low, &plain, false));
    assert!(same(&low, &Current::default(), true));
}

#[test]
fn settings_default_to_on_and_ignore_a_remote_jev_address_over_http() {
    let s = settings_of(&Value::Null);
    assert!(s.enabled);
    assert!(s.escalate);
    assert_eq!(s.channels, ["claude", "codex", "devin"]);
    assert_eq!(s.first, "claude");
    let off = settings_of(&json!({ "enabled": false, "jev_url": "http://example.com/v1" }));
    assert!(!off.enabled);
    assert_eq!(off.jev_url, None);
    let local = settings_of(&json!({ "jev_url": "http://127.0.0.1:9/v1/systemone" }));
    assert_eq!(
        local.jev_url.as_deref(),
        Some("http://127.0.0.1:9/v1/systemone")
    );
}

#[test]
fn an_unhappy_turn_steps_up_one_level_and_names_the_next_command() {
    let mut list = vec![
        choice(
            "claude",
            "claude",
            "Claude Code · Opus",
            Some("opus"),
            None,
            &[],
        ),
        choice(
            "claude",
            "claude",
            "Claude Code · Fable",
            Some("fable"),
            None,
            &[],
        ),
        choice(
            "claude",
            "claude",
            "Claude Code · Sonnet",
            Some("sonnet"),
            None,
            &[],
        ),
        choice(
            "claude",
            "claude",
            "Claude Code · Haiku",
            Some("haiku"),
            None,
            &[],
        ),
    ];
    number(&mut list);
    assert_eq!(step_up(&list, 2, None).unwrap().label, "Claude Code · Opus");
    assert_eq!(
        step_up(&list, 3, None).unwrap().label,
        "Claude Code · Fable"
    );
    assert!(step_up(&list, 4, None).is_none());

    let high = choice(
        "codex",
        "codex",
        "Codex · high effort",
        None,
        Some("high"),
        &[],
    );
    assert_eq!(command_for(&high), "/model codex high");
    assert_eq!(command_for(&list[1]), "/model fable");
    let devin = choice("devin", "devin", "Devin", None, None, &[]);
    assert_eq!(command_for(&devin), "/model devin");
}
