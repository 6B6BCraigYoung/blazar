use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::components::dialog::{self, DialogHost};
use crate::components::sidebar::Sidebar;
use crate::components::toast::{self, ToastHost};
use crate::pages::{NotFound, RuntimesPage, WorkspacePage, WorkspacesPage};
use crate::realtime;

#[component]
pub fn App() -> impl IntoView {
    realtime::provide();
    dialog::provide();
    toast::provide();
    view! {
        <Router base="/v2">
            <div class="shell">
                <Sidebar/>
                <main class="main">
                    <Routes fallback=NotFound>
                        <Route path=path!("/") view=WorkspacesPage/>
                        <Route path=path!("/runtimes") view=RuntimesPage/>
                        <Route path=path!("/w/:id") view=WorkspacePage/>
                    </Routes>
                </main>
            </div>
            <DialogHost/>
            <ToastHost/>
        </Router>
    }
}
