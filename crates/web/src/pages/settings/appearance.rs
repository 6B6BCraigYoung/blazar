use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::prefs::{PrefGroup, PrefRow};

use super::{FONT_DEFAULT, set_ui, text, ui_prefs};

#[component]
pub(super) fn AppearanceSettings() -> impl IntoView {
    let p = RwSignal::new(ui_prefs());
    let change = move |k: &'static str, v: Value| {
        set_ui(k, v.clone());
        p.update(|p| p[k] = v);
    };
    let size = move || p.get()["fontSize"].as_f64().unwrap_or(FONT_DEFAULT);
    view! {
        <PrefGroup title="显示" aside="仅对这台设备生效">
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
            <PrefRow label="代码字号" for_id="settings-font">
                <input
                    id="settings-font"
                    type="range"
                    min="11"
                    max="18"
                    step="1"
                    prop:value=size
                    on:input=move |e| {
                        if let Ok(n) = event_target_value(&e).parse::<u32>() {
                            change("fontSize", json!(n));
                        }
                    }
                />
                <span class="pref-value">{move || format!("{} px", size())}</span>
            </PrefRow>
        </PrefGroup>
    }
}
