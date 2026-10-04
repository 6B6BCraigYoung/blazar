use std::cell::Cell;

#[derive(Default)]
pub struct Latest {
    generation: Cell<u64>,
}

impl Latest {
    pub fn begin(&self) -> u64 {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        generation
    }

    pub fn accepts(&self, generation: u64) -> bool {
        self.generation.get() == generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_diff_success_or_failure_cannot_replace_a_newer_comparison() {
        let latest = Latest::default();
        let old = latest.begin();
        let new = latest.begin();
        let mut visible = "new branch diff";
        assert!(latest.accepts(new));
        for result in ["old working tree diff", "old network error"] {
            if latest.accepts(old) {
                visible = result;
            }
        }
        assert_eq!(visible, "new branch diff");
    }

    #[test]
    fn refreshes_of_the_same_comparison_still_reject_older_results() {
        let latest = Latest::default();
        let first = latest.begin();
        let refreshed = latest.begin();
        assert!(latest.accepts(refreshed));
        assert!(!latest.accepts(first));
        latest.begin();
        assert!(!latest.accepts(refreshed));
    }
}
