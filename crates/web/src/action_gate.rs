use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct ActionGate {
    pending: HashSet<String>,
}

impl ActionGate {
    pub fn begin(&mut self, key: &str) -> bool {
        self.pending.insert(key.to_owned())
    }

    pub fn busy(&self, key: &str) -> bool {
        self.pending.contains(key)
    }

    pub fn complete(&mut self, key: &str, success: bool) -> bool {
        self.pending.remove(key) && success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_submissions_are_blocked_until_the_first_finishes() {
        let mut gate = ActionGate::default();
        assert!(gate.begin("form"));
        assert!(gate.busy("form"));
        assert!(!gate.begin("form"));
        assert!(gate.complete("form", true));
        assert!(!gate.busy("form"));
        assert!(gate.begin("form"));
    }

    #[test]
    fn failed_submission_keeps_the_form_open_and_allows_retry() {
        let mut gate = ActionGate::default();
        let draft = "unsaved title and body";
        assert!(gate.begin("form"));
        let close = gate.complete("form", false);
        assert!(!close);
        assert_eq!(draft, "unsaved title and body");
        assert!(gate.begin("form"));
        assert!(gate.complete("form", true));
        assert!(!gate.complete("form", true));
    }

    #[test]
    fn independent_approval_cards_do_not_block_each_other() {
        let mut gate = ActionGate::default();
        assert!(gate.begin("approval-a"));
        assert!(gate.begin("approval-b"));
        gate.complete("approval-a", false);
        assert!(!gate.busy("approval-a"));
        assert!(gate.busy("approval-b"));
    }
}
