use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub id: String,
    pub node: Option<String>,
    pub agent_id: String,
    pub program_path: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub custom_args: Vec<String>,
    pub custom_env: BTreeMap<String, String>,
    pub max_concurrent: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigDraft {
    pub path: String,
    pub model: String,
    pub concurrency: String,
    pub permission: String,
    pub args: String,
    pub env: String,
}

impl ConfigDraft {
    pub fn for_node(configs: &[AgentConfig], node: &str) -> Self {
        let config = configs
            .iter()
            .find(|c| c.node.as_deref().unwrap_or_default() == node);
        Self {
            path: config
                .and_then(|c| c.program_path.clone())
                .unwrap_or_default(),
            model: config.and_then(|c| c.model.clone()).unwrap_or_default(),
            concurrency: config.map_or(2, |c| c.max_concurrent).to_string(),
            permission: config
                .and_then(|c| c.permission_mode.clone())
                .unwrap_or_default(),
            args: config.map(|c| c.custom_args.join("\n")).unwrap_or_default(),
            env: config
                .map(|c| {
                    c.custom_env
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        }
    }
}

pub fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            l.split_once('=')
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

pub fn parse_args(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configs() -> Vec<AgentConfig> {
        vec![
            AgentConfig {
                node: Some("gpu-1".into()),
                program_path: Some("/opt/bin/codex".into()),
                model: Some("model-remote".into()),
                permission_mode: Some("full-auto".into()),
                max_concurrent: 4,
                custom_args: vec!["--remote-flag".into()],
                custom_env: BTreeMap::from([("DEMO_MODE".into(), "remote".into())]),
                ..AgentConfig::default()
            },
            AgentConfig {
                model: Some("model-default".into()),
                max_concurrent: 3,
                custom_args: vec!["--global-flag".into()],
                custom_env: BTreeMap::from([("DEMO_MODE".into(), "global".into())]),
                ..AgentConfig::default()
            },
        ]
    }

    #[test]
    fn runtime_forms_hydrate_the_selected_machine_without_clearing_other_fields() {
        let remote = ConfigDraft::for_node(&configs(), "gpu-1");
        assert_eq!(remote.path, "/opt/bin/codex");
        assert_eq!(remote.model, "model-remote");
        assert_eq!(remote.permission, "full-auto");
        assert_eq!(remote.concurrency, "4");
        assert_eq!(remote.args, "--remote-flag");
        assert_eq!(remote.env, "DEMO_MODE=remote");
        let global = ConfigDraft::for_node(&configs(), "");
        assert_eq!(global.model, "model-default");
        assert_eq!(global.args, "--global-flag");
        assert_eq!(global.env, "DEMO_MODE=global");
    }

    #[test]
    fn a_new_machine_does_not_inherit_the_first_config() {
        let draft = ConfigDraft::for_node(&configs(), "hub-host");
        assert!(draft.path.is_empty());
        assert!(draft.env.is_empty());
        assert!(draft.args.is_empty());
        assert_eq!(draft.concurrency, "2");
    }

    #[test]
    fn env_and_args_come_one_per_line() {
        let env = parse_env(" HTTPS_PROXY = http://127.0.0.1:9527 \n# note\n\nBAD\n=x\n");
        assert_eq!(
            env,
            BTreeMap::from([("HTTPS_PROXY".to_owned(), "http://127.0.0.1:9527".to_owned())])
        );
        assert_eq!(
            parse_args(" --verbose \n\n--model x\n"),
            vec!["--verbose", "--model x"]
        );
    }
}
