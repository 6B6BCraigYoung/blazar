use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::menu::menu_keydown;

#[derive(Clone, Copy, PartialEq)]
pub struct MenuAt {
    pub x: i32,
    pub y: i32,
    pub index: usize,
}

impl MenuAt {
    pub fn from(e: &ev::MouseEvent, index: usize) -> Self {
        Self {
            x: e.client_x(),
            y: e.client_y(),
            index,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowKey {
    Open,
    Inspect,
    Delete,
}

#[derive(Clone, Copy)]
pub struct Rows {
    pub sel: RwSignal<Option<usize>>,
    pub menu: RwSignal<Option<MenuAt>>,
}

impl Default for Rows {
    fn default() -> Self {
        Self {
            sel: RwSignal::new(None),
            menu: RwSignal::new(None),
        }
    }
}

pub fn in_field(e: &ev::KeyboardEvent) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|t| {
            t.closest("input, textarea, select, [contenteditable]")
                .ok()
                .flatten()
                .is_some()
        })
}

pub fn overlay_open() -> bool {
    document()
        .query_selector("[data-modal-root], .ctx-menu, .menu")
        .ok()
        .flatten()
        .is_some()
}

pub fn list_keys_apply(e: &ev::KeyboardEvent) -> bool {
    !e.default_prevented() && !e.is_composing() && !in_field(e) && !overlay_open()
}

fn row_element(row: &str, i: usize) -> Option<web_sys::Element> {
    document()
        .query_selector(&format!("{row}[data-index=\"{i}\"]"))
        .ok()
        .flatten()
}

pub fn focus_row(row: &str, i: usize) {
    let Some(el) = row_element(row, i) else {
        return;
    };
    let target = if el.has_attribute("tabindex") {
        el
    } else {
        el.query_selector("a[href], button")
            .ok()
            .flatten()
            .unwrap_or(el)
    };
    if let Ok(t) = target.dyn_into::<web_sys::HtmlElement>() {
        let _ = t.focus();
    }
}

fn on_row_itself(e: &ev::KeyboardEvent, row: &str) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|t| {
            t.matches(&format!("{row}[data-index]")).unwrap_or(false)
                && !t.matches("a, button").unwrap_or(false)
        })
}

impl Rows {
    pub fn selected(self, i: usize) -> bool {
        self.sel.get() == Some(i)
    }

    pub fn menu_on(self, i: usize) -> bool {
        self.menu.with(|m| m.is_some_and(|m| m.index == i))
    }

    pub fn open_menu(self, e: &ev::MouseEvent, i: usize) {
        e.prevent_default();
        self.sel.set(Some(i));
        self.menu.set(Some(MenuAt::from(e, i)));
    }

    pub fn close_menu(self) {
        self.menu.set(None);
    }

    pub fn listen(
        self,
        row: &'static str,
        count: impl Fn() -> usize + 'static,
        act: impl Fn(RowKey, usize) + 'static,
    ) {
        let keys = window_event_listener(ev::keydown, move |e| {
            if !list_keys_apply(&e) {
                return;
            }
            let n = count();
            if n == 0 {
                return;
            }
            let meta = e.meta_key() || e.ctrl_key();
            let key = e.key();
            match key.as_str() {
                "ArrowDown" | "ArrowUp" if !meta && !e.alt_key() => {
                    e.prevent_default();
                    let down = key == "ArrowDown";
                    let next = match self.sel.get_untracked() {
                        None if down => 0,
                        None => n - 1,
                        Some(i) if down => (i + 1).min(n - 1),
                        Some(i) => i.saturating_sub(1),
                    };
                    self.sel.set(Some(next));
                    focus_row(row, next);
                }
                "Enter" if !meta && on_row_itself(&e, row) => {
                    if let Some(i) = self.sel.get_untracked() {
                        e.prevent_default();
                        act(RowKey::Open, i);
                    }
                }
                _ => {
                    let which = match crate::shortcuts::action(&e) {
                        Some("inspect") => RowKey::Inspect,
                        Some("delete") => RowKey::Delete,
                        _ => return,
                    };
                    if let Some(i) = self.sel.get_untracked().filter(|i| *i < n) {
                        e.prevent_default();
                        act(which, i);
                    }
                }
            }
        });
        on_cleanup(move || keys.remove());
    }
}

#[derive(Clone)]
pub struct RowAction {
    pub label: String,
    pub hint: Option<String>,
    pub danger: bool,
    pub disabled: bool,
    pub href: Option<String>,
    pub run: Option<Callback<()>>,
}

impl RowAction {
    #[must_use]
    pub fn new(label: impl Into<String>, run: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            label: label.into(),
            hint: None,
            danger: false,
            disabled: false,
            href: None,
            run: Some(Callback::new(move |()| run())),
        }
    }

    #[must_use]
    pub fn open(href: impl Into<String>) -> Self {
        Self {
            label: "打开".into(),
            hint: Some(crate::shortcuts::label("Enter")),
            danger: false,
            disabled: false,
            href: Some(href.into()),
            run: None,
        }
    }

    #[must_use]
    pub fn opens(run: impl Fn() + Send + Sync + 'static) -> Self {
        Self::new("打开", run).hint(crate::shortcuts::label("Enter"))
    }

    #[must_use]
    pub fn inspect(run: impl Fn() + Send + Sync + 'static) -> Self {
        Self::new("简介", run).key("inspect")
    }

    #[must_use]
    pub fn danger(label: impl Into<String>, run: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            danger: true,
            ..Self::new(label, run)
        }
    }

    #[must_use]
    pub fn delete(label: impl Into<String>, run: impl Fn() + Send + Sync + 'static) -> Self {
        Self::danger(label, run).key("delete")
    }

    #[must_use]
    pub fn key(mut self, id: &str) -> Self {
        let text = crate::shortcuts::label(&crate::shortcuts::current(id));
        self.hint = (!text.is_empty()).then_some(text);
        self
    }

    #[must_use]
    pub fn hint(mut self, text: impl Into<String>) -> Self {
        self.hint = Some(text.into());
        self
    }

    #[must_use]
    pub fn when(mut self, enabled: bool) -> Self {
        self.disabled = !enabled;
        self
    }
}

#[component]
pub fn RowMenu(
    at: MenuAt,
    #[prop(into)] label: String,
    groups: Vec<Vec<RowAction>>,
    on_close: Callback<()>,
    #[prop(optional, into)] note: Option<String>,
) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let close = move || on_close.run(());
    let close_down = window_event_listener(ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.closest(".ctx-menu").ok().flatten())
            .is_some();
        if !inside {
            close();
        }
    });
    let close_blur = window_event_listener(ev::blur, move |_| close());
    on_cleanup(move || {
        close_down.remove();
        close_blur.remove();
    });
    gloo_timers::callback::Timeout::new(0, move || {
        if let Some(first) = root
            .try_get_untracked()
            .flatten()
            .and_then(|r| r.query_selector("[role=\"menuitem\"]").ok().flatten())
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = first.focus();
        }
    })
    .forget();
    let groups: Vec<Vec<RowAction>> = groups.into_iter().filter(|g| !g.is_empty()).collect();
    let items: usize = groups.iter().map(Vec::len).sum();
    let height = (items as i32) * 26 + (groups.len().saturating_sub(1) as i32) * 9 + 12;
    let w = window()
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let h = window()
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let (x, y) = (at.x.min(w - 220).max(0), at.y.min(h - height - 8).max(0));
    view! {
        <div
            class="ctx-menu row-menu"
            role="menu"
            aria-label=label
            node_ref=root
            style=format!("left:{x}px;top:{y}px")
            on:keydown=move |e| {
                if let Some(r) = root.get_untracked() {
                    menu_keydown(&e, r.unchecked_ref(), Callback::new(move |_| close()));
                }
            }
        >
            {groups
                .into_iter()
                .enumerate()
                .map(|(gi, group)| {
                    view! {
                        {(gi > 0).then(|| view! { <div class="menu-sep" role="separator"></div> })}
                        {group.into_iter().map(|a| item_view(a, on_close)).collect_view()}
                    }
                })
                .collect_view()}
            {note.map(|t| view! { <div class="menu-note">{t}</div> })}
        </div>
    }
}

fn item_view(a: RowAction, on_close: Callback<()>) -> AnyView {
    let hint = a.hint.map(|h| view! { <kbd>{h}</kbd> });
    match a.href {
        Some(href) => view! {
            <a
                role="menuitem"
                class="ctx-item"
                class:danger=a.danger
                href=href
                on:click=move |_| on_close.run(())
            >
                <span>{a.label}</span>
                {hint}
            </a>
        }
        .into_any(),
        None => view! {
            <button
                type="button"
                role="menuitem"
                class="ctx-item"
                class:danger=a.danger
                disabled=a.disabled
                on:click=move |_| {
                    on_close.run(());
                    if let Some(run) = a.run {
                        run.run(());
                    }
                }
            >
                <span>{a.label}</span>
                {hint}
            </button>
        }
        .into_any(),
    }
}
