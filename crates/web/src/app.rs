use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::components::sidebar::Sidebar;
use crate::pages::{NotFound, RuntimesPage, WorkspacesPage};
use crate::realtime;

#[component]
pub fn App() -> impl IntoView {
    let bus = realtime::provide();
    view! {
        <Router base="/v2">
            <div class="shell">
                <Sidebar/>
                <main class="main">
                    <Routes fallback=NotFound>
                        <Route path=path!("/") view=WorkspacesPage/>
                        <Route path=path!("/runtimes") view=RuntimesPage/>
                    </Routes>
                </main>
            </div>
            <div class="conn" data-up=move || bus.connected.get().to_string()>
                {move || if bus.connected.get() { "已连接" } else { "正在重连 hub…" }}
            </div>
        </Router>
    }
}
