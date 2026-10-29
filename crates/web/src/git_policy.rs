pub fn valid_pr_title(gh_available: bool, title: &str) -> bool {
    !gh_available || !title.trim().is_empty()
}

/// 当前分支就是要合到的那条（`main` 对 `main` 或 `origin/main`）：这时没有「合到哪里」可言。
pub fn on_target(branch: &str, target: &str) -> bool {
    !branch.is_empty() && target.strip_prefix("origin/").unwrap_or(target) == branch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_branch_has_nowhere_to_merge() {
        assert!(on_target("main", "origin/main"));
        assert!(on_target("main", "main"));
        assert!(!on_target("feat", "origin/main"));
        assert!(!on_target("", "origin/main"));
        assert!(!on_target("main", ""));
    }

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
