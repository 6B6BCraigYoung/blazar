use leptos::prelude::*;
use leptos_router::components::{ParentRoute, Route, Router, Routes};
use leptos_router::path;

use crate::components::dialog::{self, DialogHost};
use crate::components::new_workspace::NewWorkspace;
use crate::components::palette::{AlertsDialog, Palette};
use crate::components::popups::{self, PopHost};
use crate::components::sidebar::Sidebar;
use crate::components::term_dialog::{self, TermDialogHost};
use crate::components::toast::{self, ToastHost};
use crate::components::undo::{self, UndoHost};
use crate::pages::*;
use crate::realtime;

#[component]
fn Main(children: Children) -> impl IntoView {
    let main = NodeRef::<leptos::html::Main>::new();
    let path = leptos_router::hooks::use_location().pathname;
    Effect::new(move |_| {
        path.track();
        if let Some(m) = main.get_untracked() {
            m.set_scroll_top(0);
            m.set_scroll_left(0);
        }
    });
    view! {
        <main
            class="main"
            node_ref=main
            on:scroll=move |_| {
                if let Some(m) = main.get_untracked()
                    && m.scroll_left() != 0
                {
                    m.set_scroll_left(0);
                    crate::client_log::note("layout", "main scrolled sideways, reset");
                }
            }
        >
            {children()}
        </main>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let bus = realtime::provide();
    let app = crate::app_state::provide(bus);
    provide_mesh();
    dialog::provide();
    crate::files_js::install_leave_confirm();
    toast::provide();
    crate::components::downloads::provide();
    crate::images_js::install();
    popups::provide();
    undo::provide();
    term_dialog::provide();
    let side = move || {
        if app.side_collapsed.get() {
            "off"
        } else if app.side_rail.get() {
            "rail"
        } else {
            "on"
        }
    };
    Effect::new(move |_| {
        if let Some(root) = document().document_element() {
            let _ = root.set_attribute("data-side", side());
        }
    });
    view! {
        <Router>
            <div class="shell" data-side=side>
                <button
                    class="side-toggle"
                    aria-label="展开侧栏"
                    title="展开侧栏"
                    on:click=move |_| app.side_collapsed.set(false)
                >
                    "☰"
                </button>
                <Sidebar />
                <Main>
                    <Routes fallback=NotFound>
                        <ParentRoute path=path!("") view=ConsolePage>
                            <Route path=path!("/") view=|| () />
                            <Route path=path!("/chats/:thread") view=|| () />
                        </ParentRoute>
                        <Route path=path!("/running") view=WorkspacesPage />
                        <Route path=path!("/waiting") view=WorkspacesPage />
                        <Route path=path!("/project/:name") view=ProjectPage />
                        <Route path=path!("/runtimes") view=RuntimesPage />
                        <Route path=path!("/runtimes/:id") view=RuntimeDetailPage />
                        <Route path=path!("/w/:id") view=WorkspacePage />
                        <Route path=path!("/nodes") view=NodesPage />
                        <Route path=path!("/nodes/:name") view=NodeDetailPage />
                        <Route path=path!("/chats") view=ChatsPage />
                        <Route path=path!("/inbox") view=InboxPage />
                        <Route path=path!("/tasks") view=TasksPage />
                        <Route path=path!("/autopilots") view=AutopilotsPage />
                        <Route path=path!("/jobs") view=JobsPage />
                        <Route path=path!("/bots") view=BotsPage />
                        <Route path=path!("/bots/:id") view=BotPage />
                        <Route path=path!("/agents") view=AgentsPage />
                        <Route path=path!("/agents/new") view=AgentCreatePage />
                        <Route path=path!("/agent/:id") view=AgentProfilePage />
                        <Route path=path!("/skills") view=SkillsPage />
                        <Route path=path!("/library") view=SkillsPage />
                        <Route path=path!("/apps") view=AppsPage />
                        <Route path=path!("/apps/:id") view=AppDetailPage />
                        <Route path=path!("/usage") view=UsagePage />
                        <Route path=path!("/settings") view=SettingsPage />
                        <Route path=path!("/workspaces") view=WorkspacesPage />
                        <Route path=path!("/workspaces/:id") view=WorkspacePage />
                        <Route path=path!("/accounts") view=RuntimesPage />
                        <Route path=path!("/mesh") view=NodesPage />
                    </Routes>
                </Main>
            </div>
            <NewWorkspace />
            <InspectorHost />
            <MeshHost />
            <Palette />
            <AlertsDialog />
            <TermDialogHost />
            <DialogHost />
            <crate::components::downloads::DownloadHost />
            <ToastHost />
            <PopHost />
            <UndoHost />
        </Router>
    }
}
