//! hub 的 HTTP / WebSocket 接口上传来传去的数据。服务端序列化、前端（Leptos，wasm）反序列化，两边用同一份定义。

use serde::{Deserialize, Serialize};

use crate::{NormalizedEntry, SessionId, WorkspaceId};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStat {
    pub added: u64,
    pub removed: u64,
    pub files: u64,
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

/// hub 经 `/api/ws` 推给每个界面的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    Entry {
        workspace_id: WorkspaceId,
        session_id: SessionId,

        entry: Box<NormalizedEntry>,
    },

    WorkspacesChanged,

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
