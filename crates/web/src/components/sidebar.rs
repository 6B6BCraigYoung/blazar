use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{self, StateSnapshot};
use crate::realtime::use_bus;

#[component]
pub fn Sidebar() -> impl IntoView {
    let bus = use_bus();
    let state = LocalResource::new(move || {
        bus.workspaces.track();
        api::get::<StateSnapshot>("/api/state")
    });
    let workspaces = move || {
        state
            .get()
            .and_then(Result::ok)
            .map(|s| s.workspaces)
            .unwrap_or_default()
    };

    view! {
        <nav class="side">
            <div class="brand"><i></i>"Blazar"</div>
            <div class="grp">
                <A href="/v2/" exact=true attr:class="nav">"工作区"
                    <span class="n">{move || workspaces().len()}</span>
                </A>
                <A href="/v2/runtimes" attr:class="nav">"运行时"</A>
            </div>
            <div class="grp">
                <span>"最近"</span>
                <For
                    each=move || workspaces().into_iter().take(12)
                    key=|w| (w.id.clone(), w.activity.clone(), w.name.clone())
                    children=|w| view! {
                        <a class="nav" href=format!("/v2/w/{}", w.id) title=w.path.clone()>
                            <span class="dot" data-act=w.activity.clone()></span>
                            {w.name.clone()}
                        </a>
                    }
                />
            </div>
            <div class="foot">
                <a href="/">"回到旧界面"</a>
                <span>"新界面预览 · 逐页迁移中"</span>
            </div>
        </nav>
    }
}
