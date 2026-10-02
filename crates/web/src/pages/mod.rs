mod runtimes;
mod workspace;
mod workspaces;

use leptos::prelude::*;

pub use runtimes::RuntimesPage;
pub use workspace::WorkspacePage;
pub use workspaces::WorkspacesPage;

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <div class="page">
            <div class="empty">"这一页还没搬到新界面，"<a href="/">"去旧界面看"</a></div>
        </div>
    }
}
