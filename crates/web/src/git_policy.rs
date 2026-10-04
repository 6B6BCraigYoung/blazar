pub fn valid_pr_title(gh_available: bool, title: &str) -> bool {
    !gh_available || !title.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_pr_fallback_accepts_an_empty_title() {
        assert!(valid_pr_title(false, ""));
        assert!(valid_pr_title(false, "  "));
    }

    #[test]
    fn direct_pr_creation_requires_a_title() {
        assert!(!valid_pr_title(true, ""));
        assert!(!valid_pr_title(true, " \n"));
        assert!(valid_pr_title(true, "Fix task routing"));
    }
}
