use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use blazar_core_types::{SessionId, WorkspaceId};
use blazar_db::Db;
use blazar_netmesh::{CliInvocation, EasyTierMesh};
use blazar_transport::{LocalTransport, NodeTransport, SshTransport};
use tokio::sync::{RwLock, broadcast};

pub use blazar_core_types::api::{EventFrame, ServerEvent};

pub fn ssh_control_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".blazar")
        .join("ssh-cm")
}

pub fn dev_instance() -> bool {
    std::env::var("BLAZAR_DEV_INSTANCE").is_ok_and(|v| v == "1")
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
    pub auth: std::sync::OnceLock<crate::auth::Session>,
    pub services: crate::services::Services,
    pub db: Db,
    pub bus: broadcast::Sender<EventFrame>,
    pub events: crate::events::EventLog,
    pub running: RwLock<HashMap<SessionId, RunningSession>>,

    pending: Arc<Mutex<HashSet<WorkspaceId>>>,

    pub mesh_via: String,
    pub mesh_container: Option<String>,

    pub mesh_ctx: crate::mesh::MeshCtx,

    pub proxy: crate::proxy::ProxyState,

    pub fleet: crate::fleet::FleetState,
    pub sims: crate::sims::SimsState,
    pub bots: crate::bots::BotsState,

    pub ui_driver: crate::ui_driver::UiDriver,

    pub dev_instance: bool,
}

impl AppState {
    pub fn new(
        db: Db,
        mesh_via: String,
        mesh_container: Option<String>,
        mesh_ctx: crate::mesh::MeshCtx,
    ) -> Arc<Self> {
        Self::with_services(
            db,
            mesh_via,
            mesh_container,
            mesh_ctx,
            crate::services::Services::Interactive,
        )
    }

    pub fn with_services(
        db: Db,
        mesh_via: String,
        mesh_container: Option<String>,
        mesh_ctx: crate::mesh::MeshCtx,
        services: crate::services::Services,
    ) -> Arc<Self> {
        let (bus, _) = broadcast::channel(4096);
        let events = crate::events::EventLog::default();
        events.start(db.clone(), bus.clone());
        Arc::new(Self {
            auth: std::sync::OnceLock::new(),
            services,
            db,
            bus,
            events,
            running: RwLock::new(HashMap::new()),
            pending: Arc::new(Mutex::new(HashSet::new())),
            mesh_via,
            mesh_container,
            mesh_ctx,
            proxy: crate::proxy::ProxyState::default(),
            fleet: crate::fleet::FleetState::default(),
            sims: crate::sims::SimsState::default(),
            bots: crate::bots::BotsState::default(),
            ui_driver: crate::ui_driver::UiDriver::from_env(),
            dev_instance: dev_instance(),
        })
    }

    pub async fn admit(self: &Arc<Self>, workspace: WorkspaceId) -> Result<AdmitGuard, String> {
        let running = self.running.read().await;
        if let Some((sid, _)) = running.iter().find(|(_, r)| r.workspace_id == workspace) {
            return Err(format!(
                "Session {sid} is already running in this workspace"
            ));
        }
        let guard = self.quiesce(workspace)?;
        drop(running);
        let persisted: Option<Option<String>> = sqlx::query_scalar(
            "SELECT (SELECT id FROM sessions WHERE workspace_id = w.id AND status = 'running' LIMIT 1)
             FROM workspaces w WHERE w.id = ?1",
        )
        .bind(workspace.to_string())
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| format!("Couldn't confirm the workspace's run state; restore the database and retry: {e}"))?;
        let Some(persisted) = persisted else {
            return Err("The workspace no longer exists".to_owned());
        };
        if let Some(sid) = persisted {
            return Err(format!(
                "Session {sid} is already running or waiting to reconnect in this workspace"
            ));
        }
        Ok(guard)
    }

    pub fn quiesce(&self, workspace: WorkspaceId) -> Result<AdmitGuard, String> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "Admission table is corrupted".to_owned())?;
        if !pending.insert(workspace) {
            return Err(
                "A session is starting or another operation is in progress in this workspace"
                    .to_owned(),
            );
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
            Arc::new(SshTransport::new(node).multiplexed(ssh_control_dir()))
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

            None if self.mesh_via == "local" && self.mesh_ctx.joined() => {
                self.mesh_ctx.layout.invocation()
            }
            None => CliInvocation::direct(),
        };
        EasyTierMesh::new(self.transport(&self.mesh_via), invocation)
    }

    pub fn mesh_is_hub_host(&self) -> bool {
        self.mesh_via == "local"
            && self.mesh_container.is_none()
            && self
                .mesh_ctx
                .topology_source()
                .is_none_or(|(via, _)| via == "local")
    }

    pub fn emit(&self, mut ev: ServerEvent) {
        if let ServerEvent::Entry { entry, .. } = &mut ev {
            blazar_core_types::sanitize::sanitize(&mut entry.kind);
        }
        let ev = if ev.persisted() {
            match self.events.publish(ev) {
                Ok(()) => return,
                Err(ev) => ev,
            }
        } else {
            ev
        };
        let _ = self.bus.send(EventFrame::live(ev));
    }
}
