use std::collections::{HashMap, HashSet};

use blazar_core_types::api::{BoardRow, Progress};

use crate::vocab::{self, Tone};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub running: usize,
    pub waiting: usize,
    pub failed: usize,
    pub idle: usize,
}

impl Tally {
    pub fn add(&mut self, activity: &str, cards: u32) {
        match vocab::state(activity).1 {
            Tone::Run => self.running += 1,
            Tone::Attention => self.waiting += 1,
            Tone::Failed => self.failed += 1,
            Tone::Other if cards > 0 => self.waiting += 1,
            Tone::Other => self.idle += 1,
        }
    }
}

#[must_use]
pub fn stashed(row: &BoardRow) -> bool {
    row.archived || row.path.contains("/brain/")
}

pub type Placed = Vec<(BoardRow, usize)>;

#[must_use]
pub fn arrange(rows: &[BoardRow]) -> (Placed, Placed) {
    let ids: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let mut kids: HashMap<Option<&str>, Vec<&BoardRow>> = HashMap::new();
    for r in rows {
        let up = r
            .parent
            .as_deref()
            .filter(|p| *p != r.id && ids.contains(p));
        kids.entry(up).or_default().push(r);
    }
    for list in kids.values_mut() {
        list.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.name.cmp(&b.name))
        });
    }
    let mut shown = Vec::new();
    let mut folded = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut roots: Vec<&BoardRow> = kids.get(&None).cloned().unwrap_or_default();
    roots.extend(rows.iter());
    let mut stack: Vec<(&BoardRow, usize, Option<usize>)> =
        roots.iter().rev().map(|r| (*r, 0, None)).collect();
    while let Some((r, depth, base)) = stack.pop() {
        if !seen.insert(r.id.as_str()) {
            continue;
        }
        let base = base.or_else(|| stashed(r).then_some(depth));
        match base {
            Some(b) => folded.push((r.clone(), depth - b)),
            None => shown.push((r.clone(), depth)),
        }
        if let Some(list) = kids.get(&Some(r.id.as_str())) {
            for c in list.iter().rev() {
                stack.push((*c, depth + 1, base));
            }
        }
    }
    (shown, folded)
}

#[must_use]
pub fn main_node(rows: &[(BoardRow, usize)]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (r, _) in rows {
        match counts.iter_mut().find(|(n, _)| *n == r.node) {
            Some((_, c)) => *c += 1,
            None => counts.push((r.node.as_str(), 1)),
        }
    }
    counts
        .iter()
        .max_by_key(|(_, c)| *c)
        .map(|(n, _)| (*n).to_owned())
        .unwrap_or_default()
}

#[must_use]
pub fn percent(p: &Progress) -> u32 {
    if p.total == 0 {
        return 0;
    }
    (u64::from(p.done.min(p.total)) * 100 / u64::from(p.total)) as u32
}

#[must_use]
pub fn fraction(p: &Progress) -> String {
    if p.unit.trim().is_empty() {
        format!("{}/{}", p.done, p.total)
    } else {
        format!("{}/{} {}", p.done, p.total, p.unit.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, parent: Option<&str>, at: &str) -> BoardRow {
        BoardRow {
            id: id.into(),
            name: id.into(),
            node: "x8".into(),
            path: format!("/srv/{id}"),
            parent: parent.map(str::to_owned),
            activity: "idle".into(),
            updated_at: Some(at.into()),
            ..Default::default()
        }
    }

    #[test]
    fn children_sit_under_their_parent_and_stashes_fold_away() {
        let mut stash = row("stash", None, "9");
        stash.path = "/data/brain/s1/visual-check".into();
        let mut old = row("old", Some("repo"), "1");
        old.archived = true;
        let rows = vec![
            row("repo", None, "5"),
            row("cam", Some("repo"), "4"),
            row("deep", Some("cam"), "3"),
            old,
            row("solo", None, "7"),
            stash,
            row("orphan", Some("gone"), "2"),
            row("under-old", Some("old"), "8"),
        ];
        let (shown, folded) = arrange(&rows);
        let names: Vec<(&str, usize)> = shown.iter().map(|(r, d)| (r.id.as_str(), *d)).collect();
        assert_eq!(
            names,
            [
                ("solo", 0),
                ("repo", 0),
                ("cam", 1),
                ("deep", 2),
                ("orphan", 0)
            ]
        );
        let away: Vec<(&str, usize)> = folded.iter().map(|(r, d)| (r.id.as_str(), *d)).collect();
        assert_eq!(away, [("stash", 0), ("old", 0), ("under-old", 1)]);
        let mut mixed = shown.clone();
        mixed[0].0.node = "local".into();
        assert_eq!(main_node(&mixed), "x8");
        assert_eq!(main_node(&[]), "");
    }

    #[test]
    fn a_parent_loop_still_lists_every_row_once() {
        let rows = vec![row("a", Some("b"), "1"), row("b", Some("a"), "2")];
        let (shown, folded) = arrange(&rows);
        assert_eq!(shown.len() + folded.len(), 2);
    }

    #[test]
    fn tallies_count_waiting_cards_and_progress_reads_as_a_fraction() {
        let mut t = Tally::default();
        for (a, cards) in [
            ("running", 0),
            ("asking", 0),
            ("errored", 0),
            ("idle", 2),
            ("idle", 0),
        ] {
            t.add(a, cards);
        }
        assert_eq!(
            t,
            Tally {
                running: 1,
                waiting: 2,
                failed: 1,
                idle: 1
            }
        );
        let p = Progress {
            done: 5,
            total: 30,
            unit: "件".into(),
            ..Default::default()
        };
        assert_eq!((percent(&p), fraction(&p)), (16, "5/30 件".to_owned()));
        let over = Progress {
            done: 40,
            total: 30,
            ..Default::default()
        };
        assert_eq!((percent(&over), fraction(&over)), (100, "40/30".to_owned()));
        assert_eq!(percent(&Progress::default()), 0);
    }
}
