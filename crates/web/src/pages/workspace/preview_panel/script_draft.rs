use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Scripts {
    pub setup: String,
    pub cleanup: String,
    pub dev: String,
    pub copy_files: String,
    pub preview_port: Option<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptDraft {
    value: Option<Scripts>,
    pub error: Option<String>,
    pub port: String,
}

pub fn parse_port(text: &str) -> Result<Option<u16>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .map(Some)
        .ok_or_else(|| format!("预览端口要在 1 到 65535 之间：{t}"))
}

impl ScriptDraft {
    pub fn loaded(result: Result<Scripts, String>) -> Self {
        match result {
            Ok(value) => Self {
                port: value
                    .preview_port
                    .map(|p| p.to_string())
                    .unwrap_or_default(),
                value: Some(value),
                error: None,
            },
            Err(error) => Self {
                value: None,
                error: Some(error),
                port: String::new(),
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
        let mut value = self.value.clone().ok_or_else(|| {
            self.error
                .as_ref()
                .map(|error| format!("脚本设置加载失败：{error}"))
                .unwrap_or_else(|| "脚本设置尚未加载".into())
        })?;
        value.preview_port = parse_port(&self.port)?;
        Ok(value)
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
            preview_port: Some(3000),
        }));
        assert!(draft.ready());
        assert_eq!(draft.port, "3000");
        draft.edit(|scripts| scripts.dev = "npm run preview".into());
        let saved = draft.for_save().unwrap();
        assert_eq!(saved.setup, "npm ci");
        assert_eq!(saved.cleanup, "npm run format");
        assert_eq!(saved.dev, "npm run preview");
        assert_eq!(saved.copy_files, "config/*.local.json");
        assert_eq!(saved.preview_port, Some(3000));
        let empty = ScriptDraft::loaded(Ok(Scripts::default()));
        assert_eq!(empty.port, "");
        assert_eq!(empty.for_save().unwrap(), Scripts::default());
    }

    #[test]
    fn the_preview_port_text_is_checked_before_saving() {
        assert_eq!(parse_port(""), Ok(None));
        assert_eq!(parse_port(" 8080 "), Ok(Some(8080)));
        for bad in ["0", "70000", "abc", "80a"] {
            assert!(parse_port(bad).is_err(), "{bad}");
        }
        let mut draft = ScriptDraft::loaded(Ok(Scripts::default()));
        draft.port = "4321".into();
        assert_eq!(draft.for_save().unwrap().preview_port, Some(4321));
        draft.port = "x".into();
        assert!(draft.for_save().is_err());
        draft.port = "  ".into();
        assert_eq!(draft.for_save().unwrap().preview_port, None);
        let old = serde_json::from_str::<Scripts>(
            r#"{"setup":"","cleanup":"","dev":"npm run dev","copy_files":""}"#,
        )
        .unwrap();
        assert_eq!(old.preview_port, None);
    }
}
