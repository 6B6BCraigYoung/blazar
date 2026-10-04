use std::collections::{BTreeMap, HashMap};

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

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigDraft {
    pub path: String,
    pub model: String,
    pub concurrency: String,
    pub permission: String,
    pub args: String,
    pub env: String,
}

#[derive(Clone, Copy)]
pub enum DraftField {
    Path,
    Model,
    Concurrency,
    Permission,
    Args,
    Env,
}

impl ConfigDraft {
    fn for_node(configs: &[AgentConfig], node: &str) -> Self {
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

    pub fn set(&mut self, field: DraftField, value: String) {
        *match field {
            DraftField::Path => &mut self.path,
            DraftField::Model => &mut self.model,
            DraftField::Concurrency => &mut self.concurrency,
            DraftField::Permission => &mut self.permission,
            DraftField::Args => &mut self.args,
            DraftField::Env => &mut self.env,
        } = value;
    }
}

#[derive(Default)]
pub struct ConfigDrafts {
    drafts: HashMap<(String, String), ConfigDraft>,
}

impl ConfigDrafts {
    pub fn get(&self, configs: &[AgentConfig], node: &str, section: &str) -> ConfigDraft {
        self.drafts
            .get(&(node.to_owned(), section.to_owned()))
            .cloned()
            .unwrap_or_else(|| ConfigDraft::for_node(configs, node))
    }

    pub fn remember(&mut self, node: String, section: String, draft: ConfigDraft) {
        self.drafts.insert((node, section), draft);
    }

    pub fn saved(&mut self, node: &str, section: &str) {
        self.drafts.remove(&(node.to_owned(), section.to_owned()));
    }
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
        let drafts = ConfigDrafts::default();
        let remote = drafts.get(&configs(), "gpu-1", "general");
        assert_eq!(remote.path, "/opt/bin/codex");
        assert_eq!(remote.model, "model-remote");
        assert_eq!(remote.permission, "full-auto");
        assert_eq!(remote.concurrency, "4");
        assert_eq!(remote.args, "--remote-flag");
        assert_eq!(remote.env, "DEMO_MODE=remote");
        let global = drafts.get(&configs(), "", "args");
        assert_eq!(global.model, "model-default");
        assert_eq!(global.args, "--global-flag");
        assert_eq!(global.env, "DEMO_MODE=global");
    }

    #[test]
    fn runtime_drafts_survive_machine_and_section_switches() {
        let mut drafts = ConfigDrafts::default();
        let mut remote = drafts.get(&configs(), "gpu-1", "general");
        remote.set(DraftField::Model, "edited-model".into());
        drafts.remember("gpu-1".into(), "general".into(), remote);
        assert_eq!(drafts.get(&configs(), "", "general").model, "model-default");
        assert_eq!(drafts.get(&configs(), "gpu-1", "env").model, "model-remote");
        assert_eq!(
            drafts.get(&configs(), "gpu-1", "general").model,
            "edited-model"
        );
        drafts.saved("gpu-1", "env");
        assert_eq!(
            drafts.get(&configs(), "gpu-1", "general").model,
            "edited-model"
        );
        drafts.saved("gpu-1", "general");
        assert_eq!(
            drafts.get(&configs(), "gpu-1", "general").model,
            "model-remote"
        );
    }

    #[test]
    fn a_new_machine_does_not_inherit_the_first_config() {
        let mut draft = ConfigDrafts::default().get(&configs(), "hub-host", "env");
        assert!(draft.path.is_empty());
        assert!(draft.env.is_empty());
        assert!(draft.args.is_empty());
        for field in [
            DraftField::Path,
            DraftField::Concurrency,
            DraftField::Permission,
            DraftField::Args,
            DraftField::Env,
        ] {
            draft.set(field, "value".into());
        }
        assert_eq!(draft.path, "value");
        assert_eq!(draft.concurrency, "value");
        assert_eq!(draft.permission, "value");
        assert_eq!(draft.args, "value");
        assert_eq!(draft.env, "value");
    }
}
