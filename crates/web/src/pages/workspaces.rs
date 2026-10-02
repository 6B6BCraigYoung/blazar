use leptos::prelude::*;

use crate::api::{self, StateSnapshot, WorkspaceView};
use crate::fmt;
use crate::realtime::use_bus;

#[component]
pub fn WorkspacesPage() -> impl IntoView {
    let bus = use_bus();
    let state = LocalResource::new(move || {
        bus.workspaces.track();
        bus.nodes.track();
        api::get::<StateSnapshot>("/api/state")
    });

    view! {
        <div class="page">
            <div class="page-head"><h1>"工作区"</h1></div>
            {move || match state.get() {
                None => view! { <div class="empty">"加载中…"</div> }.into_any(),
                Some(Err(e)) => view! { <div class="empty err-line">{e.to_string()}</div> }.into_any(),
                Some(Ok(s)) if s.workspaces.is_empty() => {
                    view! { <div class="empty">"还没有工作区"</div> }.into_any()
                }
                Some(Ok(s)) => view! {
                    <div class="ws-list">
                        {s.workspaces.into_iter().map(card).collect_view()}
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

fn card(w: WorkspaceView) -> impl IntoView {
    let diff = w.diff.filter(|d| d.files > 0).map(|d| {
        view! {
            <div class="diff">
                {format!("{} 个文件 ", d.files)}
                <span class="a">{format!("+{}", d.added)}</span>" "
                <span class="d">{format!("-{}", d.removed)}</span>
            </div>
        }
    });
    view! {
        <a class="ws-card" href=format!("/#/workspaces/{}", w.id)>
            <div class="top">
                <span class="dot side-dot" data-act=w.activity.clone()></span>
                <b>{w.name}</b>
                <span class="node">{w.node}</span>
            </div>
            <div class="path">{w.path}</div>
            {diff}
            <div class="muted" style="font-size:12px">
                {w.last_active_at.as_deref().map(fmt::ago).unwrap_or_default()}
            </div>
        </a>
    }
}
