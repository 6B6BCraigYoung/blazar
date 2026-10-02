mod agents;
mod apps;
mod autopilots;
mod inbox;
mod nodes;
pub mod runtimes;
mod settings;
mod skills;
mod tasks;
mod usage;
mod work_shared;
mod workspace;
pub mod workspaces;

use leptos::prelude::*;

pub use nodes::{MeshHost, NodeDetailPage, NodesPage, provide_mesh};
pub use runtimes::{RuntimeDetailPage, RuntimesPage};
pub use settings::{SettingsPage, apply_ui_preferences};
pub use skills::SkillsPage;
pub use tasks::TasksPage;
pub use usage::UsagePage;
pub use workspace::WorkspacePage;
pub use workspaces::{ProjectPage, RunningPage, WaitingPage, WorkspacesPage};

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <div class="page">
            <h1>"页面不存在"</h1>
            <div class="empty">"链接可能已失效。"<a href="/v2/">"返回工作区"</a></div>
        </div>
    }
}
pub use agents::{AgentCreatePage, AgentProfilePage, AgentsPage};
pub use apps::{AppDetailPage, AppsPage};
pub use autopilots::AutopilotsPage;
pub use inbox::InboxPage;
