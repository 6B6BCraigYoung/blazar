use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use web_sys::PointerEvent;

use crate::storage;

const KEY: &str = "blazar.v4.ws.layout";
const EX: (f64, f64) = (200.0, 480.0);
const CHAT_MIN: f64 = 480.0;
const INSP_MIN: f64 = 360.0;
const PANEL_MIN: f64 = 90.0;
pub const NARROW: f64 = 1200.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    Editor,
    Changes,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Changes {
    Diff,
    Git,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub ex: f64,
    pub insp: f64,
    pub panel: f64,
    pub files: bool,
    pub inspect: bool,
    pub tab: Tab,
    pub changes: Changes,
    pub term: bool,
    pub max: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            ex: 260.0,
            insp: 0.618,
            panel: 0.382,
            files: false,
            inspect: false,
            tab: Tab::Editor,
            changes: Changes::Diff,
            term: false,
            max: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct LayoutState {
    pub lay: RwSignal<Layout>,
    key: StoredValue<String>,
}

fn narrow() -> bool {
    window()
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .is_some_and(|w| w < NARROW)
}

impl LayoutState {
    pub fn new(ws: &str) -> Self {
        let key = format!("{KEY}.{ws}");
        Self {
            lay: RwSignal::new(storage::load(&key).unwrap_or_default()),
            key: StoredValue::new(key),
        }
    }

    fn change(self, f: impl FnOnce(&mut Layout)) {
        self.lay.update(f);
        storage::save(&self.key.get_value(), &self.lay.get_untracked());
    }

    pub fn toggle_files(self) {
        self.change(|l| l.files = !l.files);
    }

    pub fn close_files(self) {
        self.change(|l| l.files = false);
    }

    pub fn close_term(self) {
        self.change(|l| l.term = false);
    }

    pub fn toggle_term(self) {
        self.change(|l| l.term = !l.term);
    }

    pub fn toggle_inspect(self) {
        self.change(|l| {
            l.inspect = !l.inspect;
            l.max &= l.inspect;
        });
    }

    pub fn open(self, tab: Tab) {
        self.change(|l| {
            l.inspect = true;
            l.tab = tab;
        });
    }

    pub fn open_changes(self, view: Changes) {
        self.change(|l| {
            l.inspect = true;
            l.tab = Tab::Changes;
            l.changes = view;
        });
    }

    pub fn show_changes(self, view: Changes) {
        self.change(|l| l.changes = view);
    }

    pub fn close(self) {
        self.change(|l| {
            l.inspect = false;
            l.max = false;
        });
    }

    pub fn set_max(self, max: bool) {
        self.change(|l| l.max = max);
    }

    pub fn reveal_chat(self) {
        let l = self.lay.get_untracked();
        if l.max || (l.inspect && narrow()) {
            self.change(|l| {
                l.max = false;
                l.inspect &= !narrow();
            });
        }
    }

    pub fn style(self) -> String {
        let l = self.lay.get();
        format!(
            "--ex-w:{}px;--insp:{};--panel:{}",
            l.ex.clamp(EX.0, EX.1),
            l.insp,
            l.panel
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Files,
    Inspector,
    Terminal,
}

fn size_of(el: &web_sys::Element, horizontal: bool) -> f64 {
    let r = el.get_bounding_client_rect();
    if horizontal { r.width() } else { r.height() }
}

#[component]
pub fn Splitter(edge: Edge, state: LayoutState) -> impl IntoView {
    let start = StoredValue::new(None::<(f64, f64, f64)>);
    let horizontal = edge != Edge::Terminal;
    let coord = move |e: &PointerEvent| {
        if horizontal {
            f64::from(e.client_x())
        } else {
            f64::from(e.client_y())
        }
    };
    let down = move |e: PointerEvent| {
        e.prevent_default();
        let Some(t) = e
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let _ = t.set_pointer_capture(e.pointer_id());
        let pane = match edge {
            Edge::Files => t.previous_element_sibling(),
            Edge::Inspector | Edge::Terminal => t.next_element_sibling(),
        };
        let Some(pane) = pane else { return };
        let whole = t.parent_element().map_or(0.0, |p| size_of(&p, horizontal));
        start.set_value(Some((coord(&e), size_of(&pane, horizontal), whole)));
        let _ = document().body().map(|b| {
            b.class_list()
                .add_1(if horizontal { "drag-x" } else { "drag-y" })
        });
    };
    let mv = move |e: PointerEvent| {
        let Some((p0, base, whole)) = start.get_value() else {
            return;
        };
        let d = coord(&e) - p0;
        let whole = whole.max(1.0);
        state.lay.update(|l| match edge {
            Edge::Files => l.ex = (base + d).clamp(EX.0, EX.1),
            Edge::Inspector => {
                l.insp = (base - d).clamp(INSP_MIN, (whole - CHAT_MIN).max(INSP_MIN)) / whole;
            }
            Edge::Terminal => {
                l.panel = (base - d).clamp(PANEL_MIN, (whole * 0.7).max(PANEL_MIN)) / whole;
            }
        });
    };
    let up = move |_: PointerEvent| {
        if start.get_value().is_some() {
            start.set_value(None);
            let _ = document()
                .body()
                .map(|b| b.class_list().remove_2("drag-x", "drag-y"));
            state.change(|_| ());
        }
    };
    let collapse = move |_| match edge {
        Edge::Files => state.close_files(),
        Edge::Inspector => state.close(),
        Edge::Terminal => state.close_term(),
    };
    view! {
        <div
            class="split"
            data-dir=if horizontal { "x" } else { "y" }
            data-edge=match edge {
                Edge::Files => "files",
                Edge::Inspector => "inspector",
                Edge::Terminal => "terminal",
            }
            title="拖动调整大小，双击收起"
            on:pointerdown=down
            on:pointermove=mv
            on:pointerup=up
            on:pointercancel=up
            on:dblclick=collapse
        ></div>
    }
}
