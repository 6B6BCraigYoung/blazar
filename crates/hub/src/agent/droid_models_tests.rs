use super::*;

const HELP: &str = "Usage: droid exec [options] [prompt]

Available Models:
  auto                         Auto Model
  claude-opus-5-5              Opus 5.5
  gpt-6-astra                  GPT-6 Astra
  gpt-6.1-sol                  GPT-6.1 Sol (default)
  deepseek-v4-pro              DeepSeek V4 Pro [Deprecated]
  glm-5.3                      GLM-5.3

Model details:
  - Opus 5.5: supports reasoning: Yes; supported: [off, low, medium, high, max]; default: high
  - GPT-6 Astra: supports reasoning: Yes; supported: [low, medium, high, xhigh, max]; default: medium
  - GPT-6.1 Sol: supports reasoning: Yes; supported: [low, medium, high, xhigh, max]; default: medium
  - GLM-5.3: supports reasoning: Yes; supported: [low, high, max]; default: max

Authentication:
  Create an API key: https://app.factory.ai/settings/api-keys
";

#[test]
fn the_model_list_comes_from_droids_own_help() {
    let models = parse_help(HELP);
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "auto",
            "claude-opus-5-5",
            "gpt-6-astra",
            "gpt-6.1-sol",
            "glm-5.3"
        ],
        "过时的型号不列"
    );
    let sol = models.iter().find(|m| m.id == "gpt-6.1-sol").unwrap();
    assert!(sol.default);
    assert_eq!(sol.label, "GPT-6.1 Sol");
    assert_eq!(sol.efforts, vec!["low", "medium", "high", "xhigh", "max"]);
    let opus = models.iter().find(|m| m.id == "claude-opus-5-5").unwrap();
    assert_eq!(
        opus.efforts,
        vec!["low", "medium", "high", "max"],
        "Blazar 不认的档位（off）不给"
    );
    assert!(
        models
            .iter()
            .find(|m| m.id == "auto")
            .unwrap()
            .efforts
            .is_empty()
    );
}

#[test]
fn models_used_recently_on_this_mac_come_first() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".factory/sessions/-Users-me");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, model: &str, secs: u64| {
        let path = dir.join(format!("{name}.settings.json"));
        std::fs::write(
            &path,
            json!({ "model": model, "reasoningEffort": "medium" }).to_string(),
        )
        .unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    };
    write("a", "gpt-6.1-sol", 1);
    write("b", "custom:chatgpt/gpt-6-astra", 3);
    write("c", "gpt-6.1-sol", 2);
    std::fs::write(dir.join("d.jsonl"), "{}").unwrap();
    assert_eq!(
        recent(home.path()),
        vec!["custom:chatgpt/gpt-6-astra", "gpt-6.1-sol"]
    );

    let list = rows(&parse_help(HELP), &recent(home.path()));
    let ids: Vec<&str> = list.iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "",
            "custom:chatgpt/gpt-6-astra",
            "gpt-6.1-sol",
            "auto",
            "claude-opus-5-5",
            "gpt-6-astra",
            "glm-5.3"
        ]
    );
    assert_eq!(list[0]["desc"], "Uses GPT-6.1 Sol");
    assert_eq!(list[1]["label"], "GPT-6 Astra · ChatGPT");
    assert_eq!(list[1]["desc"], "Used recently on this Mac");
    assert_eq!(
        list[1]["efforts"],
        json!(["low", "medium", "high", "xhigh", "max"])
    );
    assert!(recent(&home.path().join("nowhere")).is_empty());
    let own = rows(&parse_help(HELP), &["custom:DeepSeek-Flash-0".to_owned()]);
    assert_eq!(
        own[1]["label"], "DeepSeek Flash",
        "自带 key 的模型按显示名写"
    );
}
