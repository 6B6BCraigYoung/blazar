use leptos::ev;
use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_navigate, use_query_map};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use crate::app_state::use_app;
use crate::components::icon;
use crate::components::page_head::PageHead;
use crate::components::toast::{flash, toast_error};
use crate::{api, storage};

mod about;
mod appearance;
mod chat;
mod git;
mod hooks;
mod notify;
mod resume;
mod rules;
mod shortcuts;
mod snippets;

use about::AboutSettings;
use appearance::AppearanceSettings;
use chat::ChatSettings;
use git::GitSettings;
use hooks::HookSettings;
pub(super) use notify::NOTIFY_KINDS;
use notify::NotifySettings;
use resume::AutoResumeSettings;
use rules::RuleSettings;
use shortcuts::ShortcutSettings;
use snippets::SnippetSettings;

pub(crate) const SECTIONS: &[(&str, &str, &str)] = &[
    ("appearance", "外观", icon::PALETTE),
    ("chat", "对话与运行", icon::CHAT),
    ("notify", "提醒", icon::BELL),
    ("shortcuts", "快捷键", icon::KEYBOARD),
    ("rules", "自动批准", icon::CHECK),
    ("snippets", "片段", icon::LINES),
    ("hooks", "自动执行脚本", icon::TERMINAL),
    ("about", "数据与关于", icon::INFO),
];

/// 旧的 `/settings?s=…` 链接：挪走的分区跳到新位置，并进来的分区跳到所在分区。
fn moved(section: &str) -> Option<&'static str> {
    Some(match section {
        "quota" => "/runtimes/claude",
        "git" => "/settings?s=chat",
        "platforms" => "/skills?tab=market",
        "lark" => "/apps/lark",
        "fleet" | "hub" => "/nodes",
        "bots" | "alerts" => "/bots",
        _ => return None,
    })
}

pub(super) const FONT_DEFAULT: f64 = 13.0;

pub(super) fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().into()
}
pub(super) fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
pub(super) fn rows(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

pub fn apply_ui_preferences() {
    let prefs = ui_prefs();
    if let Some(root) = document().document_element() {
        let _ = root.set_attribute("data-theme", prefs["theme"].as_str().unwrap_or("system"));
    }
    if let Ok(event) = web_sys::Event::new("blazar:preferences") {
        let _ = window().dispatch_event(&event);
    }
}

fn ui_prefs() -> Value {
    let mut defaults = json!({"v":2,"theme":"system","fontSize":FONT_DEFAULT,"minimap":false,"wordWrap":true,"sendKey":"enter"});
    if let Some(Value::Object(mut saved)) = storage::load::<Value>("blazar.ui") {
        if saved.get("v").and_then(Value::as_u64) != Some(2) {
            saved.remove("minimap");
            saved.remove("wordWrap");
            saved.remove("v");
        }
        defaults.as_object_mut().unwrap().extend(saved);
    }
    let size = defaults["fontSize"]
        .as_f64()
        .unwrap_or(FONT_DEFAULT)
        .round()
        .clamp(11.0, 18.0);
    defaults["fontSize"] = json!(size);
    defaults
}

/// 编辑器的自动换行不再是设置项，在编辑器工具栏切换，记住上次的选择。
pub fn word_wrap() -> bool {
    ui_prefs()["wordWrap"].as_bool().unwrap_or(true)
}

pub fn set_word_wrap(on: bool) -> bool {
    let mut p = ui_prefs();
    p["wordWrap"] = json!(on);
    storage::save("blazar.ui", &p);
    apply_ui_preferences();
    on
}

fn set_ui(key: &str, value: Value) {
    let mut p = ui_prefs();
    p[key] = value;
    storage::save("blazar.ui", &p);
    apply_ui_preferences();
    flash("已保存");
}

pub(super) async fn save(method: &str, path: &str, body: Value) -> bool {
    match api::send::<Value>(method, path, &body).await {
        Ok(_) => {
            flash("已保存");
            true
        }
        Err(e) => {
            toast_error("没保存上", &e);
            false
        }
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let app = use_app();
    let query = use_query_map();
    let section = move || {
        query.with(|q| match q.get("s").as_deref() {
            None => "appearance".into(),
            Some(s) => s.to_owned(),
        })
    };
    let go = use_navigate();
    Effect::new(move |_| {
        if let Some(to) = moved(&section()) {
            go(
                to,
                NavigateOptions {
                    replace: true,
                    ..Default::default()
                },
            );
        }
    });
    let title = Signal::derive(move || {
        let current = section();
        SECTIONS
            .iter()
            .find(|(id, _, _)| *id == current)
            .map_or("设置", |(_, label, _)| label)
            .to_owned()
    });
    let navigate = use_navigate();
    let esc = window_event_listener(ev::keydown, move |e| {
        if e.key() != "Escape" || e.default_prevented() || e.is_composing() {
            return;
        }
        if document()
            .query_selector("[data-modal-root], .ctx-menu")
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }
        let in_field = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| {
                t.closest("input, textarea, select, [contenteditable], .keycap")
                    .ok()
                    .flatten()
                    .is_some()
            });
        if in_field {
            return;
        }
        e.prevent_default();
        navigate(&app.settings_from.get_untracked(), Default::default());
    });
    on_cleanup(move || esc.remove());
    view! {
        <div class="page settings-page">
            <PageHead title />
            <div class="settings-body">
                {move || match section().as_str() {
                    "chat" | "git" => {
                        view! {
                            <ChatSettings />
                            <AutoResumeSettings />
                            <GitSettings />
                        }
                            .into_any()
                    }
                    "notify" => view! { <NotifySettings /> }.into_any(),
                    "shortcuts" => view! { <ShortcutSettings /> }.into_any(),
                    "rules" => view! { <RuleSettings /> }.into_any(),
                    "snippets" => view! { <SnippetSettings /> }.into_any(),
                    "hooks" => view! { <HookSettings /> }.into_any(),
                    "about" => {
                        view! {
                            <super::transfer::TransferSettings />
                            <AboutSettings />
                        }
                            .into_any()
                    }
                    s if moved(s).is_some() => ().into_any(),
                    _ => view! { <AppearanceSettings /> }.into_any(),
                }}
            </div>
        </div>
    }
}
