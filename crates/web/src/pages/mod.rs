mod runtimes;
mod workspace;
pub mod workspaces;

use leptos::prelude::*;

pub use runtimes::RuntimesPage;
pub use workspace::WorkspacePage;
pub use workspaces::{ProjectPage, RunningPage, WaitingPage, WorkspacesPage};

#[component]
pub fn NotFound() -> impl IntoView {
    // /v2/inbox → 旧界面的 /#/inbox
    let loc = window().location().pathname().unwrap_or_default();
    let old = format!(
        "/#/{}",
        loc.trim_start_matches("/v2").trim_start_matches('/')
    );
    view! {
        <div class="page">
            <div class="empty">"这一页还没搬到新界面，"<a href=old>"去旧界面看"</a></div>
        </div>
    }
}
