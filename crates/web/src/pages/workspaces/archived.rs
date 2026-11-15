use blazar_core_types::api::WorkspaceView;
use leptos::prelude::*;

use crate::api;
use crate::app_state::use_app;
use crate::fmt;

use super::restore_workspace;

#[component]
pub(super) fn ArchivedShelf() -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0u32);
    let list = LocalResource::new(move || {
        rev.track();
        app.workspaces();
        async move {
            api::get::<Vec<WorkspaceView>>("/api/workspaces?archived=true")
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|w| w.archived)
                .collect::<Vec<_>>()
        }
    });
    let count = move || list.get().map_or(0, |l| l.len());
    view! {
        <Show when=move || { count() > 0 }>
            <details class="ws-archived">
                <summary>{move || format!("已归档 {}", count())}</summary>
                <div class="ws-archived-list">
                    {move || {
                        list.get()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|w| {
                                let label = fmt::ws_label(&w.name, &w.node);
                                let id = w.id.clone();
                                let again = label.clone();
                                view! {
                                    <div class="ws-archived-row">
                                        <span class="nm">{label}</span>
                                        <span class="where">{w.path.clone()}</span>
                                        <span class="grow"></span>
                                        <button
                                            type="button"
                                            class="btn small"
                                            on:click=move |_| {
                                                restore_workspace(
                                                    app,
                                                    id.clone(),
                                                    again.clone(),
                                                    move || rev.update(|n| *n += 1),
                                                )
                                            }
                                        >
                                            "恢复"
                                        </button>
                                    </div>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </details>
        </Show>
    }
}
