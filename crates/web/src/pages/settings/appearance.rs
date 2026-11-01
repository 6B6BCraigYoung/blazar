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
            <div class="theme-tiles" id="settings-theme" role="radiogroup" aria-label="主题">
                {[("system", "跟随系统"), ("light", "浅色"), ("dark", "深色")]
                    .into_iter()
                    .map(|(value, label)| {
                        let on = move || text(&p.get(), "theme") == value;
                        view! {
                            <button
                                type="button"
                                class="theme-tile"
                                role="radio"
                                data-theme-choice=value
                                aria-checked=move || on().to_string()
                                on:click=move |_| change("theme", json!(value))
                            >
                                <span class="theme-shot" data-look=value aria-hidden="true">
                                    <span class="theme-pane" data-look="light">
                                        <i class="side"></i>
                                        <i class="line"></i>
                                        <i class="line short"></i>
                                        <i class="dot"></i>
                                    </span>
                                    <span class="theme-pane" data-look="dark">
                                        <i class="side"></i>
                                        <i class="line"></i>
                                        <i class="line short"></i>
                                        <i class="dot"></i>
                                    </span>
                                </span>
                                <span class="theme-name">{label}</span>
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
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
