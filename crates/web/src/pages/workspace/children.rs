use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::app_state::use_app;

use super::activity_label;

#[component]
pub fn Children(id: String) -> impl IntoView {
    let app = use_app();
    let open = RwSignal::new(false);
    let wid = StoredValue::new(id);
    let list = move || {
        let id = wid.get_value();
        app.workspaces()
            .into_iter()
            .filter(|w| w.parent.as_deref() == Some(id.as_str()))
            .collect::<Vec<_>>()
    };
    let name = move || {
        let id = wid.get_value();
        app.workspaces()
            .into_iter()
            .find(|w| w.id == id)
            .map(|w| w.name)
            .unwrap_or_default()
    };
    let close = window_event_listener(leptos::ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.closest(".ws-children").ok().flatten())
            .is_some();
        if !inside {
            open.set(false);
        }
    });
    on_cleanup(move || close.remove());
    let add = move || {
        open.set(false);
        app.new_ws_parent.set(Some((wid.get_value(), name())));
        app.new_ws.set(true);
    };
    view! {
        <div class="ws-children">
            <button type="button" class="btn small" aria-haspopup="menu" aria-expanded=move || open.get().to_string() on:click=move |_| open.update(|o| *o = !*o)>
                "子工作区"
                {move || { let n = list().len(); (n > 0).then(|| view! { <span class="gchip">{n}</span> }) }}
            </button>
            {move || open.get().then(|| view! {
                <div class="menu ws-children-menu" role="menu" on:keydown=move |e| if e.key() == "Escape" { open.set(false) }>
                    {move || {
                        let l = list();
                        if l.is_empty() {
                            view! { <div class="muted small ws-child-empty">"还没有子工作区。加一台机器，智能体就能把活派过去。"</div> }.into_any()
                        } else {
                            l.into_iter().map(|w| {
                                let shown = app.shown_activity(&w);
                                view! {
                                    <a class="ws-child" href=format!("/w/{}", w.id) on:click=move |_| open.set(false)>
                                        <span class="nm">{w.name.clone()}</span>
                                        <span class="muted small">{w.node.clone()}</span>
                                        <span class="state-pill" data-act=shown.clone()>{activity_label(&shown).to_owned()}</span>
                                    </a>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                    <button type="button" role="menuitem" on:click=move |_| add()>"＋ 添加子工作区"</button>
                </div>
            })}
        </div>
    }
}
