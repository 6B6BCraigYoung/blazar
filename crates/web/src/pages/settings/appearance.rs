use leptos::prelude::*;
use serde_json::{Value, json};

use super::{flag, set_ui, text, ui_prefs};

#[component]
pub(super) fn AppearanceSettings() -> impl IntoView {
    let p = RwSignal::new(ui_prefs());
    let change = move |k: &'static str, v: Value| {
        set_ui(k, v.clone());
        p.update(|p| p[k] = v);
    };
    view! {
        <section class="card settings-card">
            <h3>"外观"</h3>
            <p class="muted">"更改后自动保存，仅对这台设备生效。"</p>
            <div class="settings-row">
                <label for="settings-theme">"主题"</label>
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
            </div>
        </section>
        <section class="card settings-card">
            <h3>"编辑器"</h3>
            <div class="settings-row">
                <label for="settings-font">"字号"</label>
                <div class="settings-actions">
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
                    <span>{move || p.get()["fontSize"].to_string()}</span>
                </div>
            </div>
            <div class="settings-row">
                <label for="settings-minimap">"小地图"</label>
                <input
                    id="settings-minimap"
                    type="checkbox"
                    prop:checked=move || flag(&p.get(), "minimap")
                    on:change=move |e| change("minimap", json!(event_target_checked(&e)))
                />
            </div>
            <div class="settings-row">
                <label for="settings-wrap">"自动换行"</label>
                <input
                    id="settings-wrap"
                    type="checkbox"
                    prop:checked=move || flag(&p.get(), "wordWrap")
                    on:change=move |e| change("wordWrap", json!(event_target_checked(&e)))
                />
            </div>
        </section>
    }
}
