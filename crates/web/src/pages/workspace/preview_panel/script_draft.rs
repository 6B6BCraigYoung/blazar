use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Scripts {
    pub setup: String,
    pub cleanup: String,
    pub dev: String,
    pub copy_files: String,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptDraft {
    value: Option<Scripts>,
    pub error: Option<String>,
}

impl ScriptDraft {
    pub fn loaded(result: Result<Scripts, String>) -> Self {
        match result {
            Ok(value) => Self {
                value: Some(value),
                error: None,
            },
            Err(error) => Self {
                value: None,
                error: Some(error),
            },
        }
    }

    pub fn ready(&self) -> bool {
        self.value.is_some()
    }

    pub fn value(&self) -> Option<&Scripts> {
        self.value.as_ref()
    }

    pub fn edit(&mut self, edit: impl FnOnce(&mut Scripts)) {
        if let Some(value) = &mut self.value {
            edit(value);
        }
    }

    pub fn for_save(&self) -> Result<Scripts, String> {
        self.value.clone().ok_or_else(|| {
            self.error
                .as_ref()
                .map(|error| format!("脚本设置加载失败：{error}"))
                .unwrap_or_else(|| "脚本设置尚未加载".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_or_failed_script_settings_cannot_be_saved_or_tested() {
        let mut pending = ScriptDraft::default();
        assert!(!pending.ready());
        pending.edit(|scripts| scripts.dev = "unloaded change".into());
        assert!(pending.value().is_none());
        assert!(pending.for_save().is_err());
        let failed = ScriptDraft::loaded(Err("connection lost".into()));
        assert!(!failed.ready());
        assert!(failed.for_save().is_err());
    }

    #[test]
    fn loaded_script_edits_preserve_other_commands() {
        let mut draft = ScriptDraft::loaded(Ok(Scripts {
            setup: "npm ci".into(),
            cleanup: "npm run format".into(),
            dev: "npm run dev".into(),
            copy_files: "config/*.local.json".into(),
        }));
        assert!(draft.ready());
        draft.edit(|scripts| scripts.dev = "npm run preview".into());
        let saved = draft.for_save().unwrap();
        assert_eq!(saved.setup, "npm ci");
        assert_eq!(saved.cleanup, "npm run format");
        assert_eq!(saved.dev, "npm run preview");
        assert_eq!(saved.copy_files, "config/*.local.json");
        let empty = ScriptDraft::loaded(Ok(Scripts::default()));
        assert_eq!(empty.for_save().unwrap(), Scripts::default());
    }
}
