use serde::{Deserialize, Serialize};

use crate::{NormalizedEntry, SessionId, WorkspaceId};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStat {
    pub added: u64,
    pub removed: u64,
    pub files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub is_dir: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<ChangeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirItem {
    pub name: String,
    pub is_dir: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirItems {
    pub items: Vec<DirItem>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub size: u64,

    pub too_large: bool,

    pub binary: bool,

    #[serde(default)]
    pub mtime: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    pub saved: bool,

    pub mtime: u64,
    pub size: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeView {
    pub name: String,
    pub transport: String,
    pub ipv4: Option<String>,
    pub status: String,
    pub latency_ms: Option<f64>,
    pub cost: Option<String>,
    pub workspace_count: i64,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<String>,
}

impl NodeView {
    #[must_use]
    pub fn is_self_mesh(&self) -> bool {
        self.network.as_deref() == Some("easytier")
            && self
                .cost
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case("local"))
    }

    #[must_use]
    pub fn is_personal(&self) -> bool {
        self.role.as_deref() == Some("personal")
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.network.as_deref() == Some("easytier") && self.role.is_none() && !self.is_self_mesh()
    }

    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.is_personal() && !self.is_pending() && !self.is_self_mesh()
    }
}

#[must_use]
pub fn valid_bot_handle(handle: &str) -> bool {
    let h = handle.trim();
    (1..=32).contains(&h.len())
        && h.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !h.starts_with('-')
        && !h.ends_with('-')
}

#[must_use]
pub fn machine_label(node: &str) -> &str {
    if node == "local" { "本机" } else { node }
}

#[must_use]
pub fn workspace_title<'a>(name: &'a str, node: &str) -> &'a str {
    name.strip_suffix(format!(" @ {node}").as_str())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(name)
}

#[must_use]
pub fn workspace_label(name: &str, node: &str) -> String {
    format!("{}（{}）", workspace_title(name, node), machine_label(node))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceView {
    pub id: String,
    pub name: String,
    pub node: String,
    pub path: String,
    pub project: Option<String>,
    pub activity: String,
    pub last_active_at: Option<String>,
    pub session_id: Option<String>,

    pub diff: Option<DiffStat>,

    pub diff_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub nodes: Vec<NodeView>,
    pub workspaces: Vec<WorkspaceView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    Entry {
        workspace_id: WorkspaceId,
        session_id: SessionId,

        entry: Box<NormalizedEntry>,
    },

    WorkspacesChanged,

    ResyncRequired,

    NodesChanged,

    TasksChanged,
    AutopilotsChanged,
    AccountsChanged,
    InboxChanged,
    BotsChanged,
    FactsChanged,
    JobsChanged,

    ScriptsChanged {
        workspace_id: WorkspaceId,
    },

    QueueChanged {
        workspace_id: WorkspaceId,
    },

    QueueSent {
        workspace_id: WorkspaceId,

        queued_thread: Option<String>,
        thread_id: String,
        session_id: String,
    },

    SessionTitled {
        workspace_id: WorkspaceId,
        session_id: SessionId,
        title: String,
    },

    UiCommand {
        id: u64,
        command: serde_json::Value,
    },
}

impl ServerEvent {
    #[must_use]
    pub fn persisted(&self) -> bool {
        match self {
            Self::WorkspacesChanged
            | Self::NodesChanged
            | Self::TasksChanged
            | Self::AutopilotsChanged
            | Self::AccountsChanged
            | Self::InboxChanged
            | Self::BotsChanged
            | Self::FactsChanged
            | Self::JobsChanged
            | Self::SessionTitled { .. } => true,
            Self::Entry { .. }
            | Self::ResyncRequired
            | Self::ScriptsChanged { .. }
            | Self::QueueChanged { .. }
            | Self::QueueSent { .. }
            | Self::UiCommand { .. } => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<i64>,
    #[serde(flatten)]
    pub event: ServerEvent,
}

impl EventFrame {
    #[must_use]
    pub fn live(event: ServerEvent) -> Self {
        Self {
            event_id: None,
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_handles_are_short_lowercase_words_joined_by_hyphens() {
        for good in ["trainer", "gpu-0-watch", "a", &"a".repeat(32)] {
            assert!(valid_bot_handle(good), "{good}");
        }
        for bad in [
            "",
            "Bad Handle!",
            "-lead",
            "tail-",
            "under_score",
            &"a".repeat(33),
        ] {
            assert!(!valid_bot_handle(bad), "{bad}");
        }
    }

    #[test]
    fn workspace_labels_name_the_machine_once() {
        assert_eq!(machine_label("local"), "本机");
        assert_eq!(machine_label("gpu-0"), "gpu-0");
        assert_eq!(workspace_title("主控 @ gpu-0", "gpu-0"), "主控");
        assert_eq!(workspace_title("主控 @ gpu-0", "gpu-1"), "主控 @ gpu-0");
        assert_eq!(workspace_title(" @ gpu-0", "gpu-0"), " @ gpu-0");
        assert_eq!(workspace_label("主控 @ gpu-0", "gpu-0"), "主控（gpu-0）");
        assert_eq!(workspace_label("blazar", "local"), "blazar（本机）");
    }

    #[test]
    fn resync_event_round_trips_without_changing_existing_event_tags() {
        let event = serde_json::json!({"kind":"resync_required"});
        assert!(matches!(
            serde_json::from_value::<ServerEvent>(event.clone()).unwrap(),
            ServerEvent::ResyncRequired
        ));
        assert_eq!(
            serde_json::to_value(ServerEvent::ResyncRequired).unwrap(),
            event
        );
        assert_eq!(
            serde_json::to_value(ServerEvent::WorkspacesChanged).unwrap(),
            serde_json::json!({"kind":"workspaces_changed"})
        );
    }

    #[test]
    fn file_versions_preserve_legacy_response_compatibility() {
        let file: FileContent = serde_json::from_value(serde_json::json!({
            "path": "note.txt", "content": "note", "size": 4,
            "too_large": false, "binary": false, "mtime": 12
        }))
        .unwrap();
        assert_eq!(file.version, None);
        let mut written: Written = serde_json::from_value(serde_json::json!({
            "saved": true, "mtime": 12, "size": 4
        }))
        .unwrap();
        assert_eq!(written.version, None);
        written.version = Some("a".repeat(64));
        let value = serde_json::to_value(&written).unwrap();
        assert_eq!(value["mtime"], 12);
        assert_eq!(serde_json::from_value::<Written>(value).unwrap(), written);
    }

    #[test]
    fn server_events_round_trip_with_the_kind_tag() {
        let ev = ServerEvent::AccountsChanged;
        let s = serde_json::to_string(&ev).unwrap();
        assert_eq!(s, r#"{"kind":"accounts_changed"}"#);
        assert!(matches!(
            serde_json::from_str::<ServerEvent>(&s).unwrap(),
            ServerEvent::AccountsChanged
        ));
    }

    fn every_event() -> Vec<(ServerEvent, bool)> {
        let ws = WorkspaceId::new();
        let sid = SessionId::new();
        vec![
            (
                ServerEvent::Entry {
                    workspace_id: ws,
                    session_id: sid,
                    entry: Box::new(NormalizedEntry {
                        seq: 1,
                        ts: chrono::Utc::now(),
                        parent_tool_use_id: None,
                        kind: crate::EntryKind::Finished(crate::Outcome::Interrupted),
                    }),
                },
                false,
            ),
            (ServerEvent::WorkspacesChanged, true),
            (ServerEvent::ResyncRequired, false),
            (ServerEvent::NodesChanged, true),
            (ServerEvent::TasksChanged, true),
            (ServerEvent::AutopilotsChanged, true),
            (ServerEvent::AccountsChanged, true),
            (ServerEvent::InboxChanged, true),
            (ServerEvent::BotsChanged, true),
            (ServerEvent::FactsChanged, true),
            (ServerEvent::JobsChanged, true),
            (ServerEvent::ScriptsChanged { workspace_id: ws }, false),
            (ServerEvent::QueueChanged { workspace_id: ws }, false),
            (
                ServerEvent::QueueSent {
                    workspace_id: ws,
                    queued_thread: None,
                    thread_id: "t".into(),
                    session_id: "s".into(),
                },
                false,
            ),
            (
                ServerEvent::SessionTitled {
                    workspace_id: ws,
                    session_id: sid,
                    title: "改名".into(),
                },
                true,
            ),
            (
                ServerEvent::UiCommand {
                    id: 7,
                    command: serde_json::json!({"op": "errors"}),
                },
                false,
            ),
        ]
    }

    #[test]
    fn only_low_frequency_state_changes_are_persisted() {
        let all = every_event();
        let kinds: std::collections::BTreeSet<String> = all
            .iter()
            .map(|(e, _)| {
                serde_json::to_value(e).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(kinds.len(), 16, "每种事件都要在这里登记是否持久化");
        for (event, persisted) in all {
            assert_eq!(event.persisted(), persisted, "{event:?}");
            if persisted {
                let value = serde_json::to_value(&event).unwrap();
                let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
                for key in keys {
                    assert!(
                        ["kind", "workspace_id", "session_id", "title"].contains(&key.as_str()),
                        "持久化事件多了字段 {key}，先确认里面没有密钥或令牌"
                    );
                }
            }
        }
    }

    #[test]
    fn frames_carry_an_event_id_only_when_persisted() {
        for (event, _) in every_event() {
            let plain = serde_json::to_value(&event).unwrap();
            let live = serde_json::to_value(EventFrame::live(event.clone())).unwrap();
            assert_eq!(live, plain);
            let framed = EventFrame {
                event_id: Some(42),
                event,
            };
            let text = serde_json::to_string(&framed).unwrap();
            let back: EventFrame = serde_json::from_str(&text).unwrap();
            assert_eq!(back.event_id, Some(42));
            assert_eq!(serde_json::to_value(&back.event).unwrap(), plain);
            assert!(serde_json::from_str::<ServerEvent>(&text).is_ok());
        }
        let ui: EventFrame =
            serde_json::from_str(r#"{"kind":"ui_command","id":3,"command":null}"#).unwrap();
        assert_eq!(ui.event_id, None);
        assert!(matches!(ui.event, ServerEvent::UiCommand { id: 3, .. }));
    }
}
