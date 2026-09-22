use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use blazar_core_types::{NormalizedEntry, SessionId, WorkspaceId};
use blazar_db::Db;
use blazar_netmesh::{CliInvocation, EasyTierMesh};
use blazar_transport::{LocalTransport, NodeTransport, SshTransport};
use serde::Serialize;
use tokio::sync::{RwLock, broadcast};

#[derive(Debug, Clone, Serialize)]
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

pub struct RunningSession {
    pub workspace_id: WorkspaceId,
    pub task: tokio::task::JoinHandle<()>,

    pub killer: Option<blazar_transport::ProcessKiller>,

    pub live: Option<Arc<crate::run::Live>>,
}

pub struct AdmitGuard {
    pending: Arc<Mutex<HashSet<WorkspaceId>>>,
    workspace: WorkspaceId,
}

impl Drop for AdmitGuard {
    fn drop(&mut self) {
        if let Ok(mut p) = self.pending.lock() {
            p.remove(&self.workspace);
        }
    }
}

pub struct AppState {
    pub db: Db,
    pub bus: broadcast::Sender<ServerEvent>,
    pub running: RwLock<HashMap<SessionId, RunningSession>>,

    pending: Arc<Mutex<HashSet<WorkspaceId>>>,

    pub mesh_via: String,
    pub mesh_container: Option<String>,

    pub mesh_ctx: crate::mesh::MeshCtx,
}

impl AppState {
    pub fn new(
        db: Db,
        mesh_via: String,
        mesh_container: Option<String>,
        mesh_ctx: crate::mesh::MeshCtx,
    ) -> Arc<Self> {
        let (bus, _) = broadcast::channel(4096);
        Arc::new(Self {
            db,
            bus,
            running: RwLock::new(HashMap::new()),
            pending: Arc::new(Mutex::new(HashSet::new())),
            mesh_via,
            mesh_container,
            mesh_ctx,
        })
    }

    pub async fn admit(self: &Arc<Self>, workspace: WorkspaceId) -> Result<AdmitGuard, String> {
        if let Some((sid, _)) = self
            .running
            .read()
            .await
            .iter()
            .find(|(_, r)| r.workspace_id == workspace)
        {
            return Err(format!("该工作区已有会话 {sid} 在运行"));
        }
        {
            let mut pending = self.pending.lock().map_err(|_| "准入表已损坏".to_owned())?;
            if !pending.insert(workspace) {
                return Err("该工作区正有一个会话在启动".to_owned());
            }
        }
        Ok(AdmitGuard {
            pending: self.pending.clone(),
            workspace,
        })
    }

    pub fn transport(&self, node: &str) -> Arc<dyn NodeTransport> {
        if node == "local" {
            Arc::new(LocalTransport)
        } else {
            Arc::new(SshTransport::new(node))
        }
    }

    pub fn mesh(&self) -> EasyTierMesh {
        if self.mesh_via == "local"
            && self.mesh_container.is_none()
            && let Some((via, inv)) = self.mesh_ctx.topology_source()
        {
            return EasyTierMesh::new(self.transport(&via), inv);
        }
        let invocation = match &self.mesh_container {
            Some(c) => CliInvocation::docker(c),

            None if self.mesh_via == "local" && self.mesh_ctx.layout.joined() => {
                self.mesh_ctx.layout.invocation()
            }
            None => CliInvocation::direct(),
        };
        EasyTierMesh::new(self.transport(&self.mesh_via), invocation)
    }

    pub fn emit(&self, mut ev: ServerEvent) {
        if let ServerEvent::Entry { entry, .. } = &mut ev {
            blazar_core_types::sanitize::sanitize(&mut entry.kind);
        }
        let _ = self.bus.send(ev);
    }
}
