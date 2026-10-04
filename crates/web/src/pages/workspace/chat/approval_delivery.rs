use std::collections::HashSet;

#[derive(Clone, Default)]
pub struct ApprovalDeliveries {
    pending: HashSet<String>,
    delivered: HashSet<String>,
}

impl ApprovalDeliveries {
    pub fn begin(&mut self, id: &str) -> bool {
        if self.delivered.contains(id) || !self.pending.insert(id.into()) {
            return false;
        }
        true
    }

    pub fn finish(&mut self, id: &str, delivered: bool) {
        self.pending.remove(id);
        if delivered {
            self.delivered.insert(id.into());
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.delivered.contains(id)
    }

    pub fn clear(&mut self) {
        self.delivered.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_card_and_its_input_remain_mounted_until_delivery_succeeds() {
        let mut deliveries = ApprovalDeliveries::default();
        assert!(deliveries.begin("approval-a"));
        assert!(!deliveries.contains("approval-a"));
        assert!(!deliveries.begin("approval-a"));
        deliveries.finish("approval-a", false);
        assert!(!deliveries.contains("approval-a"));
        assert!(deliveries.begin("approval-a"));
        deliveries.finish("approval-a", true);
        assert!(deliveries.contains("approval-a"));
        assert!(!deliveries.begin("approval-a"));
    }

    #[test]
    fn history_refresh_does_not_allow_duplicate_in_flight_approval_submission() {
        let mut deliveries = ApprovalDeliveries::default();
        assert!(deliveries.begin("approval-a"));
        deliveries.clear();
        assert!(!deliveries.begin("approval-a"));
        deliveries.finish("approval-a", false);
        assert!(deliveries.begin("approval-a"));
    }
}
