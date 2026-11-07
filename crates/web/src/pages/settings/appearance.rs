use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::prefs::{PrefGroup, PrefRow, Switch};
use crate::components::toast::{flash, toast_error};

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
            <MenubarRow />
        </PrefGroup>
    }
}

#[component]
fn MenubarRow() -> impl IntoView {
    let ready = RwSignal::new(false);
    let on = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let notice = RwSignal::new(None::<String>);
    if crate::desktop::available() {
        leptos::task::spawn_local(async move {
            let Ok(v) = crate::desktop::invoke("menubar", &json!({})).await else {
                return;
            };
            if v["available"].as_bool() != Some(true) {
                return;
            }
            on.set(v["enabled"].as_bool().unwrap_or(true));
            if v["notice"].is_object() {
                notice.set(Some(crate::menubar_notice::notice(
                    v["notice"]["stuck_secs"].as_u64(),
                )));
                let _ = crate::desktop::invoke("menubar_noticed", &json!({})).await;
            }
            ready.set(true);
        });
    }
    let change = move |v: bool| {
        on.set(v);
        busy.set(true);
        leptos::task::spawn_local(async move {
            match crate::desktop::invoke("set_menubar", &json!({ "enabled": v })).await {
                Ok(next) => {
                    on.set(next["enabled"].as_bool().unwrap_or(v));
                    notice.set(None);
                    flash("已保存");
                }
                Err(e) => {
                    on.set(!v);
                    toast_error("菜单栏图标没改成", &e);
                }
            }
            busy.set(false);
        });
    };
    move || {
        ready.get().then(|| {
            view! {
                <PrefRow
                    label="在菜单栏显示图标"
                    detail="从菜单栏打开 Blazar、发起新对话、切换账号。"
                    for_id="settings-menubar"
                    notice=Signal::derive(move || notice.get())
                >
                    <Switch
                        id="settings-menubar"
                        checked=on
                        disabled=busy
                        on_change=Callback::new(change)
                    />
                </PrefRow>
            }
        })
    }
}
