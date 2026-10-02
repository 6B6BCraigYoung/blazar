use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::components::dialog::{self, DialogHost};
use crate::components::new_workspace::NewWorkspace;
use crate::components::palette::{AlertsDialog, Palette};
use crate::components::sidebar::Sidebar;
use crate::components::toast::{self, ToastHost};
use crate::pages::{
    NotFound, ProjectPage, RunningPage, RuntimesPage, WaitingPage, WorkspacePage, WorkspacesPage,
};
use crate::realtime;

#[component]
pub fn App() -> impl IntoView {
    let bus = realtime::provide();
    crate::app_state::provide(bus);
    dialog::provide();
    toast::provide();
    view! {
        <Router base="/v2">
            <div class="shell">
                <Sidebar/>
                <main class="main">
                    <Routes fallback=NotFound>
                        <Route path=path!("/") view=WorkspacesPage/>
                        <Route path=path!("/running") view=RunningPage/>
                        <Route path=path!("/waiting") view=WaitingPage/>
                        <Route path=path!("/project/:name") view=ProjectPage/>
                        <Route path=path!("/runtimes") view=RuntimesPage/>
                        <Route path=path!("/w/:id") view=WorkspacePage/>
                    </Routes>
                </main>
            </div>
            <NewWorkspace/>
            <Palette/>
            <AlertsDialog/>
            <DialogHost/>
            <ToastHost/>
        </Router>
    }
}
