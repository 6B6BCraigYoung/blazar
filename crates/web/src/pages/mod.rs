mod agents;
mod apps;
mod autopilots;
mod bots;
mod chats;
pub(crate) mod inbox;
mod jobs;
mod nodes;
pub mod runtimes;
mod settings;
mod skills;
mod tasks;
mod transfer;
mod usage;
pub(crate) mod work_shared;
mod workspace;
pub mod workspaces;

use leptos::prelude::*;

pub use nodes::{MeshHost, NodeDetailPage, NodesPage, provide_mesh};
pub use runtimes::{RuntimeDetailPage, RuntimesPage};
pub(crate) use settings::SECTIONS;
pub use settings::{SettingsPage, apply_ui_preferences, set_word_wrap, word_wrap};
pub use skills::SkillsPage;
pub use tasks::TasksPage;
pub use usage::UsagePage;
pub use workspace::WorkspacePage;
pub use workspaces::{HomePage, InspectorHost, ProjectPage, WorkspacesPage};

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <div class="page">
            <crate::components::page_head::PageHead title="页面不存在" />
            <div class="empty">"链接可能已失效。"<a href="/">"返回中控台"</a></div>
        </div>
    }
}
pub use agents::{AgentCreatePage, AgentProfilePage, AgentsPage};
pub use apps::{AppDetailPage, AppsPage};
pub use autopilots::AutopilotsPage;
pub use bots::{BotPage, BotsPage};
pub use chats::{ChatsPage, ConsolePage};
pub use inbox::InboxPage;
pub use jobs::JobsPage;
