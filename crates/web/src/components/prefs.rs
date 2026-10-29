use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::modal::Modal;

#[component]
pub fn PrefGroup(
    #[prop(optional, into)] title: Option<String>,
    #[prop(optional, into)] note: Option<String>,
    #[prop(optional, into)] aside: Option<String>,
    #[prop(default = false)] guard: bool,
    #[prop(optional, into)] trailing: Option<ViewFn>,
    children: Children,
) -> impl IntoView {
    view! {
        <section class="pref-group" data-ui-guard=guard.then_some("")>
            {title
                .map(|t| {
                    view! {
                        <h3 class="pref-heading">
                            <span>{t}</span>
                            {aside.map(|a| view! { <span class="pref-aside">{a}</span> })}
                            {trailing.map(|v| v.run())}
                        </h3>
                    }
                })}
            {note.map(|n| view! { <p class="pref-note">{n}</p> })}
            <div class="pref-rows">{children()}</div>
        </section>
    }
}

#[component]
pub fn PrefRow(
    #[prop(into)] label: String,
    #[prop(optional, into)] detail: Option<String>,
    #[prop(optional, into)] for_id: Option<String>,
    #[prop(optional, into)] enabled: Option<Signal<bool>>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let off = move || enabled.is_some_and(|e| !e.get());
    let detail = detail.filter(|d| !d.is_empty());
    view! {
        <div class="pref-row" data-off=move || off().then_some("")>
            <label class="pref-label" for=for_id>
                <span class="pref-row-title">{label}</span>
                {detail.map(|d| view! { <span class="pref-row-detail">{d}</span> })}
            </label>
            <div class="pref-control">{children.map(|c| c())}</div>
        </div>
    }
}

#[component]
pub fn Switch(
    #[prop(optional, into)] id: Option<String>,
    #[prop(into)] checked: Signal<bool>,
    #[prop(into)] on_change: Callback<bool>,
    #[prop(optional, into)] disabled: Signal<bool>,
    #[prop(optional, into)] label: Option<String>,
) -> impl IntoView {
    view! {
        <input
            type="checkbox"
            role="switch"
            class="switch"
            id=id
            aria-label=label
            prop:checked=move || checked.get()
            disabled=move || disabled.get()
            on:change=move |e| on_change.run(event_target_checked(&e))
        />
    }
}

#[component]
pub fn Sheet(
    #[prop(into)] title: String,
    on_close: Callback<()>,
    #[prop(optional, into)] foot: Option<ViewFn>,
    #[prop(default = "dlg sheet")] class: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <Modal label=title.clone() class=class mask_class="dlg-mask sheet-mask" on_close=on_close>
            <h3>{title}</h3>
            <div class="sheet-body">{children()}</div>
            {foot.map(|f| view! { <div class="dlg-foot">{f.run()}</div> })}
        </Modal>
    }
}

#[component]
pub fn PrefAdvanced(
    #[prop(into)] title: String,
    #[prop(default = false)] wide: bool,
    #[prop(default = "高级…")] button: &'static str,
    children: ChildrenFn,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let title = StoredValue::new(title);
    let class = if wide { "dlg sheet wide" } else { "dlg sheet" };
    view! {
        <div class="pref-row pref-more">
            <button
                type="button"
                class="pref-more-btn"
                aria-haspopup="dialog"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.set(true)
            >
                {button}
            </button>
        </div>
        <Show when=move || {
            open.get()
        }>
            {
                let children = children.clone();
                view! {
                    <Sheet
                        title=title.get_value()
                        class=class
                        on_close=Callback::new(move |()| open.set(false))
                        foot=move || {
                            view! {
                                <button
                                    type="button"
                                    class="btn primary"
                                    data-modal-initial-focus=""
                                    on:click=move |_| open.set(false)
                                >
                                    "完成"
                                </button>
                            }
                        }
                    >
                        {children()}
                    </Sheet>
                }
            }
        </Show>
    }
}

fn in_field(e: &ev::KeyboardEvent) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|t| {
            t.closest("input, textarea, select")
                .ok()
                .flatten()
                .is_some()
        })
}

#[component]
pub fn PrefList(
    #[prop(into)] label: String,
    selected: RwSignal<Option<String>>,
    on_add: Callback<()>,
    on_remove: Callback<String>,
    #[prop(optional)] on_edit: Option<Callback<String>>,
    #[prop(optional, into)] busy: Signal<bool>,
    children: Children,
) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let rows = move || -> Vec<web_sys::HtmlElement> {
        root.get_untracked()
            .and_then(|r| r.query_selector_all(".pref-item").ok())
            .map(|list| {
                (0..list.length())
                    .filter_map(|i| list.get(i))
                    .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let current = move |rows: &[web_sys::HtmlElement]| {
        let id = selected.get_untracked()?;
        rows.iter()
            .position(|r| r.get_attribute("data-id").as_deref() == Some(id.as_str()))
    };
    let add_label = format!("添加{label}");
    let remove_label = format!("移除选中的{label}");
    let edit_label = format!("编辑选中的{label}");
    view! {
        <div class="pref-list">
            <div
                class="pref-items"
                role="listbox"
                aria-label=label
                tabindex="0"
                node_ref=root
                on:focus=move |e| {
                    let onto_self = e
                        .target()
                        .is_some_and(|t| { e.current_target().is_some_and(|c| t == c) });
                    if !onto_self {
                        return;
                    }
                    let rows = rows();
                    if let Some(row) = current(&rows).and_then(|i| rows.get(i)).or(rows.first()) {
                        let _ = row.focus();
                    }
                }
                on:keydown=move |e: ev::KeyboardEvent| {
                    if e.is_composing() || e.default_prevented() || in_field(&e) {
                        return;
                    }
                    let key = e.key();
                    match key.as_str() {
                        "ArrowDown" | "ArrowUp" => {
                            let rows = rows();
                            if rows.is_empty() {
                                return;
                            }
                            e.prevent_default();
                            let next = match (current(&rows), key == "ArrowDown") {
                                (None, true) => 0,
                                (None, false) => rows.len() - 1,
                                (Some(i), true) => (i + 1).min(rows.len() - 1),
                                (Some(i), false) => i.saturating_sub(1),
                            };
                            let _ = rows[next].focus();
                        }
                        "Backspace" | "Delete" => {
                            if let Some(id) = selected.get_untracked() && !busy.get_untracked() {
                                e.prevent_default();
                                on_remove.run(id);
                            }
                        }
                        "Enter" => {
                            if let (Some(edit), Some(id)) = (on_edit, selected.get_untracked())
                                && !busy.get_untracked()
                            {
                                e.prevent_default();
                                edit.run(id);
                            }
                        }
                        _ => {}
                    }
                }
            >
                {children()}
            </div>
            <div class="pref-bar">
                <button
                    type="button"
                    class="pref-bar-btn"
                    aria-label=add_label.clone()
                    title=add_label
                    disabled=move || busy.get()
                    on:click=move |_| on_add.run(())
                >
                    "+"
                </button>
                <span class="sep" aria-hidden="true"></span>
                <button
                    type="button"
                    class="pref-bar-btn"
                    aria-label=remove_label.clone()
                    title=remove_label
                    disabled=move || busy.get() || selected.get().is_none()
                    on:click=move |_| {
                        if let Some(id) = selected.get_untracked() {
                            on_remove.run(id);
                        }
                    }
                >
                    "−"
                </button>
                {on_edit
                    .map(|edit| {
                        view! {
                            <span class="sep" aria-hidden="true"></span>
                            <button
                                type="button"
                                class="pref-bar-btn text"
                                aria-label=edit_label.clone()
                                title=edit_label
                                disabled=move || busy.get() || selected.get().is_none()
                                on:click=move |_| {
                                    if let Some(id) = selected.get_untracked() {
                                        edit.run(id);
                                    }
                                }
                            >
                                "编辑…"
                            </button>
                        }
                    })}
            </div>
        </div>
    }
}

#[component]
pub fn PrefItem(
    #[prop(into)] id: String,
    selected: RwSignal<Option<String>>,
    #[prop(optional)] on_open: Option<Callback<String>>,
    #[prop(into)] title: String,
    #[prop(optional, into)] detail: String,
    #[prop(default = false)] code: bool,
    #[prop(optional, into)] trailing: Option<ViewFn>,
) -> impl IntoView {
    let (id_sel, id_focus, id_open) = (id.clone(), id.clone(), id.clone());
    let title = if code {
        view! { <code class="pref-item-title">{title}</code> }.into_any()
    } else {
        view! { <span class="pref-item-title">{title}</span> }.into_any()
    };
    view! {
        <div
            class="pref-item"
            role="option"
            tabindex="-1"
            data-id=id
            aria-selected=move || (selected.get().as_deref() == Some(id_sel.as_str())).to_string()
            on:focusin=move |_| selected.set(Some(id_focus.clone()))
            on:dblclick=move |_| {
                if let Some(open) = on_open {
                    open.run(id_open.clone());
                }
            }
        >
            <div class="pref-item-main">
                {title}
                {(!detail.is_empty())
                    .then(|| view! { <span class="pref-item-detail">{detail}</span> })}
            </div>
            {trailing.map(|t| view! { <div class="pref-item-trailing">{t.run()}</div> })}
        </div>
    }
}

#[component]
pub fn PrefEmpty(#[prop(into)] text: String) -> impl IntoView {
    view! { <div class="pref-empty">{text}</div> }
}
