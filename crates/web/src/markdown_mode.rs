#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownMode {
    Preview,
    Source,
}

impl MarkdownMode {
    pub fn from_stored(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("source" | "\"source\"") => Self::Source,
            _ => Self::Preview,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Source => "source",
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub fn load() -> MarkdownMode {
    MarkdownMode::from_stored(crate::storage::load_raw("blazar.md.mode").as_deref())
}

#[cfg(target_arch = "wasm32")]
pub fn save(mode: MarkdownMode) {
    crate::storage::save_raw("blazar.md.mode", mode.as_str());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_mode_reads_settings_and_legacy_workspace_values() {
        for value in ["source", "\"source\""] {
            assert_eq!(MarkdownMode::from_stored(Some(value)), MarkdownMode::Source);
        }
        for value in ["preview", "\"preview\"", "invalid", ""] {
            assert_eq!(
                MarkdownMode::from_stored(Some(value)),
                MarkdownMode::Preview
            );
        }
        assert_eq!(MarkdownMode::from_stored(None), MarkdownMode::Preview);
    }

    #[test]
    fn both_markdown_modes_round_trip_through_the_shared_storage_format() {
        for mode in [MarkdownMode::Preview, MarkdownMode::Source] {
            assert_eq!(MarkdownMode::from_stored(Some(mode.as_str())), mode);
        }
    }
}
