use std::collections::HashMap;
use std::rc::Rc;

use blazar_core_types::{EntryKind, McpServerStatus, NormalizedEntry, RateLimitWindow, TokenUsage};
use serde::Deserialize;
use serde_json::Value;

mod approvals;
mod builder;
mod finish;
mod groups;
mod tools;

pub use approvals::{always_rule, approval_choices, approval_title, approval_what, first_open};
pub use builder::Model;
#[cfg(test)]
pub use builder::build;
pub use finish::step_sig;
pub use groups::{Group, group_sig, group_steps};
pub use tools::{edit_diff, rel_path, tool_name, write_preview};

#[derive(Debug, Clone)]
pub struct Row {
    pub session_id: String,
    pub seq: u64,

    pub ts: i64,
    pub parent: Option<String>,
    pub rewound: bool,
    pub elided: bool,
    pub kind: EntryKind,
}

#[derive(Deserialize)]
pub struct HistRow {
    session_id: String,
    seq: u64,
    #[serde(default)]
    ts: String,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    rewound: bool,
    #[serde(default)]
    elided: bool,
    kind: EntryKind,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum HistItem {
    Row(Box<HistRow>),
    Unreadable(serde::de::IgnoredAny),
}

impl HistItem {
    pub fn into_row(self) -> Option<Row> {
        let Self::Row(h) = self else {
            return None;
        };
        let h = *h;
        Some(Row {
            ts: parse_ts(&h.ts),
            kind: h.kind,
            session_id: h.session_id,
            seq: h.seq,
            parent: h.parent,
            rewound: h.rewound,
            elided: h.elided,
        })
    }
}

fn parse_ts(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map_or(0, |t| t.timestamp_millis())
}

impl Row {
    pub fn from_live(session_id: String, e: NormalizedEntry) -> Self {
        Self {
            session_id,
            seq: e.seq,
            ts: e.ts.timestamp_millis(),
            parent: e.parent_tool_use_id.map(|t| t.0),
            rewound: false,
            elided: false,
            kind: e.kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum St {
    Run,
    Ok,
    Err,
}

impl St {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Ok => "ok",
            Self::Err => "err",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Todo {
    pub content: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Extra {
    None,

    Cmd(String),
    Todos(Vec<Todo>),

    Diff {
        rows: Vec<(char, String)>,
        more: usize,
        added: usize,
        removed: usize,
    },
    Files(Vec<String>),
    Plan(String),
    Fleet(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Res {
    pub shown: String,
    pub full: String,
    pub more: usize,

    pub cls: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub label: String,
    pub arg: String,
    pub state: St,
    pub extra: Extra,
    pub result: Option<Res>,
    pub ms: Option<u64>,

    pub sub: Vec<Step>,
    pub sub_calls: usize,
    pub sub_last: String,
    pub sub_elided: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Thinking { text: String, secs: i64 },
    Tool(Box<Tool>),

    Text(String),

    Bg(String),

    Note(String),

    Orphan(Res),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fold {
    pub steps: Vec<Step>,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    User {
        sid: String,
        seq: u64,
        text: String,
        first: bool,
        long: bool,
    },
    Assistant {
        text: String,
    },
    Fold(Fold),
    Error {
        text: String,
        by_account: bool,
    },
    Warn {
        denied: Vec<String>,
    },
    Meta {
        text: String,
        bad: bool,
    },
    Rewound {
        turns: usize,
        items: Vec<Item>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,

    pub sig: u64,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub id: String,
    pub sid: String,
    pub request: Value,
    pub ask: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub items: Vec<Rc<Item>>,
    frozen: Rc<HashMap<String, usize>>,
    index: HashMap<String, usize>,
    pub pending: Vec<Pending>,
    pub todos: Option<Vec<Todo>>,
    pub usage: Option<TokenUsage>,
    pub model: String,
    pub mcp: Vec<McpServerStatus>,
    pub rate: Vec<RateLimitWindow>,

    pub last_session: Option<String>,

    pub turn_tokens: u64,
}

impl Transcript {
    pub fn item(&self, key: &str) -> Option<&Item> {
        self.index
            .get(key)
            .or_else(|| self.frozen.get(key))
            .and_then(|i| self.items.get(*i))
            .map(Rc::as_ref)
    }
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod synthetic;

#[cfg(test)]
mod incremental_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_skips_entries_it_cannot_read() {
        let items: Vec<HistItem> = serde_json::from_str(
            r#"[
                {"session_id":"s","seq":1,"ts":"2026-10-19T00:00:00Z","cursor":"s:1",
                 "parent":null,"rewound":false,"kind":{"type":"user_message","text":"hi"}},
                {"session_id":"s","seq":2,"kind":{"type":"from_a_newer_hub","x":1}},
                {"session_id":"s","seq":3,"parent":"t1","kind":{"type":"thinking","text":"hm"}}
            ]"#,
        )
        .unwrap();
        let rows: Vec<Row> = items.into_iter().filter_map(HistItem::into_row).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts, parse_ts("2026-10-19T00:00:00Z"));
        assert!(matches!(&rows[0].kind, EntryKind::UserMessage { text } if text == "hi"));
        assert_eq!((rows[1].seq, rows[1].parent.as_deref()), (3, Some("t1")));
    }
}
