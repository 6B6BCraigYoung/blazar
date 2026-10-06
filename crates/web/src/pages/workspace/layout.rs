use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use web_sys::PointerEvent;

use crate::storage;

const KEY: &str = "blazar.v3.ws.layout";
const EX: (f64, f64) = (180.0, 480.0);
const AUX: (f64, f64) = (300.0, 1200.0);
const CODE_MIN: f64 = 360.0;
const PANEL_MIN: f64 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub ex: f64,
    pub aux: f64,
    pub panel: f64,
    pub hide_ex: bool,
    pub hide_aux: bool,
    pub hide_panel: bool,

    pub panel_max: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            ex: 0.191,
            aux: 0.309,
            panel: 0.382,
            hide_ex: false,
            hide_aux: false,
            hide_panel: false,
            panel_max: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Explorer,
    Panel,
    Aux,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Sizes {
    pub ex: f64,
    pub aux: f64,
    pub panel: f64,
}

#[derive(Clone, Copy)]
pub struct LayoutState {
    pub lay: RwSignal<Layout>,

    pub area: RwSignal<(f64, f64)>,
}

impl LayoutState {
    pub fn new() -> Self {
        let mut lay: Layout = storage::load(KEY).unwrap_or_default();
        if storage::load::<Layout>(KEY).is_none() {
            let w = window()
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(1440.0);
            lay.hide_aux = w <= 1180.0;
            lay.hide_ex = w <= 860.0;
        }
        Self {
            lay: RwSignal::new(lay),
            area: RwSignal::new((1440.0, 900.0)),
        }
    }

    pub fn save(self) {
        storage::save(KEY, &self.lay.get_untracked());
    }

    pub fn toggle(self, r: Region) {
        self.lay.update(|l| match r {
            Region::Explorer => l.hide_ex = !l.hide_ex,
            Region::Panel => l.hide_panel = !l.hide_panel,
            Region::Aux => l.hide_aux = !l.hide_aux,
        });
        self.save();
    }

    pub fn sizes(self) -> Sizes {
        let l = self.lay.get();
        let (w, h) = self.area.get();
        let mut ex = if l.hide_ex {
            0.0
        } else {
            (l.ex * w).clamp(EX.0, EX.1)
        };
        let mut aux = if l.hide_aux {
            0.0
        } else {
            (l.aux * w).clamp(AUX.0, AUX.1)
        };
        let mut over = ex + aux + CODE_MIN + 20.0 - w;
        if over > 0.0 && aux > 0.0 {
            let d = over.min(aux - AUX.0);
            aux -= d;
            over -= d;
        }
        if over > 0.0 && ex > 0.0 {
            ex -= over.min(ex - EX.0);
        }
        let panel = (l.panel * h).clamp(PANEL_MIN, (h * 0.7).min(h - 160.0).max(PANEL_MIN));
        Sizes { ex, aux, panel }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Explorer,

    Aux,

    Panel,
}

#[component]
pub fn Splitter(edge: Edge, state: LayoutState) -> impl IntoView {
    let start = StoredValue::new(None::<(f64, Sizes)>);
    let horizontal = edge != Edge::Panel;
    let coord = move |e: &PointerEvent| {
        if horizontal {
            f64::from(e.client_x())
        } else {
            f64::from(e.client_y())
        }
    };
    let down = move |e: PointerEvent| {
        e.prevent_default();
        if let Some(t) = e
            .current_target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
        {
            let _ = t.set_pointer_capture(e.pointer_id());
        }
        start.set_value(Some((coord(&e), state.sizes())));
        let _ = document().body().map(|b| {
            b.class_list()
                .add_1(if horizontal { "drag-x" } else { "drag-y" })
        });
    };
    let mv = move |e: PointerEvent| {
        let Some((p0, base)) = start.get_value() else {
            return;
        };
        let d = coord(&e) - p0;
        let (w, h) = state.area.get_untracked();
        let room = w - CODE_MIN - 20.0;
        state.lay.update(|l| match edge {
            Edge::Explorer => {
                let other = if l.hide_aux { 0.0 } else { base.aux };
                l.ex = (base.ex + d).clamp(EX.0, EX.1.min(room - other).max(EX.0)) / w.max(1.0);
            }
            Edge::Aux => {
                let other = if l.hide_ex { 0.0 } else { base.ex };
                l.aux =
                    (base.aux - d).clamp(AUX.0, AUX.1.min(room - other).max(AUX.0)) / w.max(1.0);
            }
            Edge::Panel => {
                l.panel = (base.panel - d)
                    .clamp(PANEL_MIN, (h * 0.7).min(h - 160.0).max(PANEL_MIN))
                    / h.max(1.0);
            }
        });
    };
    let up = move |_: PointerEvent| {
        if start.get_value().is_some() {
            start.set_value(None);
            let _ = document()
                .body()
                .map(|b| b.class_list().remove_2("drag-x", "drag-y"));
            state.save();
        }
    };
    let region = match edge {
        Edge::Explorer => Region::Explorer,
        Edge::Aux => Region::Aux,
        Edge::Panel => Region::Panel,
    };
    let hidden = move || {
        let l = state.lay.get();
        match region {
            Region::Explorer => l.hide_ex,
            Region::Aux => l.hide_aux,
            Region::Panel => l.hide_panel,
        }
    };
    view! {
        <div class="split" data-dir=if horizontal { "x" } else { "y" }
            data-collapsed=move || hidden().to_string()
            title="拖动调整大小，双击收起"
            on:pointerdown=down on:pointermove=mv on:pointerup=up on:pointercancel=up
            on:dblclick=move |_| state.toggle(region)>
        </div>
    }
}
