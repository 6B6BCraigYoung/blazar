use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::prefs::{PrefGroup, PrefRow, Switch};

use super::{flag, set_ui, text, ui_prefs};

#[component]
pub(super) fn AppearanceSettings() -> impl IntoView {
    let p = RwSignal::new(ui_prefs());
    let change = move |k: &'static str, v: Value| {
        set_ui(k, v.clone());
        p.update(|p| p[k] = v);
    };
    view! {
        <PrefGroup note="仅对这台设备生效。">
            <PrefRow label="主题" for_id="settings-theme">
                <select
                    id="settings-theme"
                    class="settings-input"
                    prop:value=move || text(&p.get(), "theme")
                    on:change=move |e| change("theme", json!(event_target_value(&e)))
                >
                    <option value="system" selected=move || text(&p.get(), "theme") == "system">
                        "跟随系统"
                    </option>
                    <option value="light" selected=move || text(&p.get(), "theme") == "light">
                        "浅色"
                    </option>
                    <option value="dark" selected=move || text(&p.get(), "theme") == "dark">
                        "深色"
                    </option>
                </select>
            </PrefRow>
        </PrefGroup>
        <PrefGroup title="编辑器">
            <PrefRow label="字号" for_id="settings-font">
                <input
                    id="settings-font"
                    type="range"
                    min="11"
                    max="18"
                    step="0.5"
                    prop:value=move || p.get()["fontSize"].as_f64().unwrap_or(12.5)
                    on:input=move |e| {
                        if let Ok(n) = event_target_value(&e).parse::<f64>() {
                            change("fontSize", json!(n));
                        }
                    }
                />
                <span class="pref-value">{move || p.get()["fontSize"].to_string()}</span>
            </PrefRow>
            <PrefRow label="小地图" for_id="settings-minimap">
                <Switch
                    id="settings-minimap"
                    checked=Signal::derive(move || flag(&p.get(), "minimap"))
                    on_change=move |v: bool| change("minimap", json!(v))
                />
            </PrefRow>
            <PrefRow label="自动换行" for_id="settings-wrap">
                <Switch
                    id="settings-wrap"
                    checked=Signal::derive(move || flag(&p.get(), "wordWrap"))
                    on_change=move |v: bool| change("wordWrap", json!(v))
                />
            </PrefRow>
        </PrefGroup>
    }
}
