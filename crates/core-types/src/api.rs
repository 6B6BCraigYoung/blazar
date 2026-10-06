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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
