use std::collections::{HashMap, HashSet};

pub type RowKey = (String, u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    generation: u64,
    thread: Option<String>,
}

#[derive(Clone, Default)]
pub struct HistoryRequests {
    generation: u64,
    thread: Option<String>,
}

impl HistoryRequests {
    pub fn begin(&mut self, thread: Option<String>) -> (bool, HistoryRequest) {
        let changed = self.thread != thread;
        self.thread = thread.clone();
        self.generation = self.generation.wrapping_add(1);
        (
            changed,
            HistoryRequest {
                generation: self.generation,
                thread,
            },
        )
    }

    pub fn accepts(&self, request: &HistoryRequest) -> bool {
        self.thread == request.thread && self.generation == request.generation
    }
}

pub fn merge_history<T: Clone>(
    mut history: Vec<T>,
    current: &[T],
    baseline: &HashSet<RowKey>,
    key: impl Fn(&T) -> RowKey,
) -> Vec<T> {
    let mut seen = HashSet::new();
    history.retain(|row| seen.insert(key(row)));
    history.extend(
        current
            .iter()
            .filter(|row| {
                let key = key(row);
                !baseline.contains(&key) && seen.insert(key)
            })
            .cloned(),
    );
    let mut sessions = HashMap::new();
    for row in &history {
        let next = sessions.len();
        sessions.entry(key(row).0).or_insert(next);
    }
    history.sort_by_cached_key(|row| {
        let (session, seq) = key(row);
        (sessions.get(&session).copied().unwrap_or_default(), seq)
    });
    history
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Row(&'static str, u64, bool);

    fn key(row: &Row) -> RowKey {
        (row.0.into(), row.1)
    }

    #[test]
    fn history_keeps_live_rows_received_while_the_request_was_in_flight() {
        let before = [Row("session-a", 1, false)];
        let baseline = before.iter().map(key).collect();
        let current = [
            Row("session-a", 1, false),
            Row("session-a", 2, false),
            Row("session-b", 1, false),
        ];
        let merged = merge_history(vec![Row("session-a", 1, true)], &current, &baseline, key);
        assert_eq!(
            merged,
            vec![
                Row("session-a", 1, true),
                Row("session-a", 2, false),
                Row("session-b", 1, false)
            ]
        );
    }

    #[test]
    fn history_deduplicates_live_echoes_and_orders_each_session_by_sequence() {
        let merged = merge_history(
            vec![Row("session-a", 1, false), Row("session-a", 3, true)],
            &[Row("session-a", 3, false), Row("session-a", 2, false)],
            &HashSet::new(),
            key,
        );
        assert_eq!(
            merged,
            vec![
                Row("session-a", 1, false),
                Row("session-a", 2, false),
                Row("session-a", 3, true)
            ]
        );
    }

    #[test]
    fn older_history_responses_cannot_overwrite_a_newer_load_or_reopened_thread() {
        let mut requests = HistoryRequests::default();
        let (changed, first) = requests.begin(Some("thread-a".into()));
        assert!(changed);
        let (changed, second) = requests.begin(Some("thread-a".into()));
        assert!(!changed);
        assert!(!requests.accepts(&first));
        assert!(requests.accepts(&second));
        requests.begin(Some("thread-b".into()));
        requests.begin(Some("thread-a".into()));
        assert!(!requests.accepts(&second));
    }
}
