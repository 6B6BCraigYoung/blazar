#[path = "../src/pages/workspace/chat/history.rs"]
mod history;

#[test]
fn empty_history_drops_old_snapshot_rows_but_keeps_new_live_events() {
    let baseline = std::collections::HashSet::from([("session-a".to_owned(), 1)]);
    let current = [
        ("session-a", 1, "already visible before the history request"),
        (
            "session-a",
            2,
            "received live while the history request waited",
        ),
    ];
    let merged = history::merge_history(Vec::new(), &current, &baseline, |row| {
        (row.0.to_owned(), row.1)
    });
    assert_eq!(
        merged,
        vec![(
            "session-a",
            2,
            "received live while the history request waited"
        )]
    );
}
