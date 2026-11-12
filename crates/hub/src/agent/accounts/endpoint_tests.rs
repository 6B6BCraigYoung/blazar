use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::*;

fn deepseek() -> Endpoint {
    valid_endpoint(&EndpointInput {
        base_url: "https://api.deepseek.com/anthropic/".into(),
        models: vec!["deepseek-v4-pro".into(), "deepseek-flash".into()],
        default_model: None,
        preset: Some("deepseek".into()),
    })
    .unwrap()
}

#[test]
fn base_urls_are_normalized_and_kept_off_plain_http() {
    assert_eq!(
        valid_base_url(" https://api.deepseek.com/anthropic/ ").unwrap(),
        "https://api.deepseek.com/anthropic"
    );
    assert_eq!(
        valid_base_url("https://gw.example.com/anthropic/v1").unwrap(),
        "https://gw.example.com/anthropic",
        "Claude Code 会自己补 /v1/messages"
    );
    assert_eq!(
        valid_base_url("https://gw.example.com/v1/messages").unwrap(),
        "https://gw.example.com"
    );
    assert_eq!(
        valid_base_url("http://127.0.0.1:4000").unwrap(),
        "http://127.0.0.1:4000"
    );
    assert!(
        valid_base_url("http://10.0.0.8:30000").is_err(),
        "密钥不能明文走网络"
    );
    assert!(valid_base_url("https://user:pw@gw.example.com").is_err());
    assert!(valid_base_url("https://gw.example.com/x?key=1").is_err());
    assert!(valid_base_url("api.deepseek.com").is_err());
}

#[test]
fn keys_are_trimmed_and_subscription_tokens_are_refused() {
    assert_eq!(valid_key("  sk-abc12345\n").unwrap(), "sk-abc12345");
    assert_eq!(valid_key("Bearer sk-abc12345").unwrap(), "sk-abc12345");
    assert!(valid_key("sk-abc 12345").is_err());
    assert!(valid_key("short").is_err());
    assert!(valid_key("sk-ant-oat01-xyz-long-enough").is_err());
}

#[test]
fn the_deepseek_preset_brings_its_small_model() {
    let e = deepseek();
    assert_eq!(e.base_url, "https://api.deepseek.com/anthropic");
    assert_eq!(e.default_model, "deepseek-v4-pro");
    assert_eq!(e.small_model.as_deref(), Some("deepseek-flash"));
    assert_eq!(e.preset.as_deref(), Some("deepseek"));
    assert_eq!(e.host(), "api.deepseek.com");
}

#[test]
fn a_preset_only_sticks_to_its_own_address() {
    let e = valid_endpoint(&EndpointInput {
        base_url: "https://gw.example.com/anthropic".into(),
        models: vec![" glm-5 ".into(), "glm-5".into(), String::new()],
        default_model: Some("glm-5".into()),
        preset: Some("deepseek".into()),
    })
    .unwrap();
    assert_eq!(e.models, vec!["glm-5".to_owned()], "去空格、去重");
    assert_eq!(e.preset, None);
    assert_eq!(e.small_model, None);
}

#[test]
fn endpoint_input_is_checked() {
    let base = || EndpointInput {
        base_url: "https://gw.example.com".into(),
        models: vec!["m1".into()],
        default_model: None,
        preset: None,
    };
    assert_eq!(valid_endpoint(&base()).unwrap().default_model, "m1");
    let mut none = base();
    none.models.clear();
    assert!(valid_endpoint(&none).is_err());
    let mut stray = base();
    stray.default_model = Some("m2".into());
    assert!(valid_endpoint(&stray).is_err());
    let mut bad = base();
    bad.models = vec!["m 1".into()];
    assert!(valid_endpoint(&bad).is_err());
    let mut unknown = base();
    unknown.preset = Some("nope".into());
    assert!(valid_endpoint(&unknown).is_err());
}

#[test]
fn claude_models_fall_back_to_the_account_default() {
    let e = deepseek();
    assert_eq!(model_for(&e, Some("deepseek-flash")), "deepseek-flash");
    assert_eq!(model_for(&e, Some("opus")), "deepseek-v4-pro");
    assert_eq!(model_for(&e, Some("claude-fable-5-2")), "deepseek-v4-pro");
    assert_eq!(model_for(&e, None), "deepseek-v4-pro");
}

#[test]
fn a_run_points_every_alias_at_the_endpoint_and_drops_other_credentials() {
    let e = deepseek();
    let mut env = BTreeMap::from([
        ("ANTHROPIC_AUTH_TOKEN".to_owned(), "from-profile".to_owned()),
        (
            "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
            "sk-ant-oat01-x".to_owned(),
        ),
        (
            "ANTHROPIC_BASE_URL".to_owned(),
            "https://elsewhere".to_owned(),
        ),
        ("KEEP".to_owned(), "1".to_owned()),
    ]);
    let mut unset = BTreeSet::new();
    apply(&mut env, &mut unset, &e, "deepseek-flash");
    assert_eq!(
        env["ANTHROPIC_BASE_URL"],
        "https://api.deepseek.com/anthropic"
    );
    for alias in ["FABLE", "OPUS", "SONNET"] {
        assert_eq!(
            env[&format!("ANTHROPIC_DEFAULT_{alias}_MODEL")],
            "deepseek-flash"
        );
    }
    assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "deepseek-flash");
    assert!(
        !env.contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        "这个开关会关掉 Auto 模式的安全检查，命令和联网全被拒"
    );
    assert_eq!(env["KEEP"], "1");
    assert!(
        !env.contains_key("ANTHROPIC_AUTH_TOKEN"),
        "密钥只从账号目录的文件里读"
    );
    assert!(!env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(unset.contains("CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(unset.contains("ANTHROPIC_API_KEY"));
}

#[test]
fn the_menu_lists_only_the_endpoint_models() {
    let rows = rows(&deepseek());
    let ids: Vec<&str> = rows.iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(ids, vec!["", "deepseek-v4-pro", "deepseek-flash"]);
    assert_eq!(rows[0]["desc"], "Uses deepseek-v4-pro");
    assert!(rows.iter().all(|r| r["efforts"] == json!([])));
}

#[test]
fn endpoint_answers_are_judged_by_status() {
    let listed = judge(
        200,
        &json!({ "object": "list", "data": [{ "id": "deepseek-flash" }, { "id": "deepseek-v4-pro" }] }),
    );
    assert_eq!(listed.status, "ok");
    assert_eq!(listed.models, vec!["deepseek-flash", "deepseek-v4-pro"]);
    assert_eq!(judge(401, &json!({})).status, "logged_out");
    assert_eq!(judge(403, &json!({})).status, "logged_out");
    assert_eq!(
        judge(404, &json!({})).status,
        "ok",
        "没有型号列表也算连上了"
    );
    let odd = judge(502, &json!({}));
    assert_eq!(odd.status, "unknown");
    assert!(odd.error.unwrap().contains("502"));
}
