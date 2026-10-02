use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::components::dialog::{self, DialogHost};
use crate::components::new_workspace::NewWorkspace;
use crate::components::palette::{AlertsDialog, Palette};
use crate::components::sidebar::Sidebar;
use crate::components::term_dialog::{self, TermDialogHost};
use crate::components::toast::{self, ToastHost};
use crate::pages::*;
use crate::realtime;

#[component]
pub fn App() -> impl IntoView {
    let bus = realtime::provide();
    let app = crate::app_state::provide(bus);
    provide_mesh();
    dialog::provide();
    toast::provide();
    term_dialog::provide();
    view! {
        <Router>
            <div class="shell" data-side=move || if app.side_collapsed.get() { "off" } else { "on" }>
                <button class="side-toggle" aria-label="展开侧栏" title="展开侧栏" on:click=move |_| app.side_collapsed.set(false)>"☰"</button>
                <Sidebar/>
                <main class="main">
                    <Routes fallback=NotFound>
                        <Route path=path!("/") view=WorkspacesPage/>
                        <Route path=path!("/running") view=RunningPage/>
                        <Route path=path!("/waiting") view=WaitingPage/>
                        <Route path=path!("/project/:name") view=ProjectPage/>
                        <Route path=path!("/runtimes") view=RuntimesPage/>
                        <Route path=path!("/runtimes/:id") view=RuntimeDetailPage/>
                        <Route path=path!("/w/:id") view=WorkspacePage/>
                        <Route path=path!("/nodes") view=NodesPage/>
                        <Route path=path!("/nodes/:name") view=NodeDetailPage/>
                        <Route path=path!("/inbox") view=InboxPage/>
                        <Route path=path!("/tasks") view=TasksPage/>
                        <Route path=path!("/autopilots") view=AutopilotsPage/>
                        <Route path=path!("/agents") view=AgentsPage/>
                        <Route path=path!("/agents/new") view=AgentCreatePage/>
                        <Route path=path!("/agent/:id") view=AgentProfilePage/>
                        <Route path=path!("/skills") view=SkillsPage/>
                        <Route path=path!("/library") view=SkillsPage/>
                        <Route path=path!("/apps") view=AppsPage/>
                        <Route path=path!("/apps/:id") view=AppDetailPage/>
                        <Route path=path!("/usage") view=UsagePage/>
                        <Route path=path!("/settings") view=SettingsPage/>
                        <Route path=path!("/workspaces") view=WorkspacesPage/>
                        <Route path=path!("/workspaces/:id") view=WorkspacePage/>
                        <Route path=path!("/accounts") view=RuntimesPage/>
                        <Route path=path!("/mesh") view=NodesPage/>
                    </Routes>
                </main>
            </div>
            <NewWorkspace/>
            <MeshHost/>
            <Palette/>
            <AlertsDialog/>
            <TermDialogHost/>
            <DialogHost/>
            <ToastHost/>
        </Router>
    }
}
