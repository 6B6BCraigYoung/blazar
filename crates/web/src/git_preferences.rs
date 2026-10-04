#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitPreferences {
    pub draft_pr: bool,
    pub show_ai: bool,
}

impl GitPreferences {
    pub fn from_values(draft_pr: Option<bool>, show_ai: Option<bool>) -> Self {
        Self {
            draft_pr: draft_pr.unwrap_or(false),
            show_ai: show_ai.unwrap_or(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_git_preferences_control_the_pr_form_and_ai_actions() {
        assert_eq!(
            GitPreferences::from_values(Some(true), Some(false)),
            GitPreferences {
                draft_pr: true,
                show_ai: false,
            }
        );
        assert_eq!(
            GitPreferences::from_values(Some(false), Some(true)),
            GitPreferences {
                draft_pr: false,
                show_ai: true,
            }
        );
    }

    #[test]
    fn absent_git_preferences_preserve_server_defaults() {
        assert_eq!(
            GitPreferences::from_values(None, None),
            GitPreferences {
                draft_pr: false,
                show_ai: true,
            }
        );
    }
}
