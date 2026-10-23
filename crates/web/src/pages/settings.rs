use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use crate::app_state::use_app;
use crate::components::icon;
use crate::components::page_head::PageHead;
use crate::components::toast::{flash, toast};
use crate::{api, storage};

mod about;
mod appearance;
mod chat;
mod git;
mod hooks;
mod hub;
mod notify;
mod platforms;
mod rules;
mod shortcuts;
mod snippets;

use about::AboutSettings;
use appearance::AppearanceSettings;
use chat::{ChatSettings, FleetSettings};
use git::GitSettings;
use hooks::HookSettings;
use hub::HubSettings;
pub(super) use notify::NOTIFY_KINDS;
use notify::NotifySettings;
use platforms::PlatformSettings;
use rules::RuleSettings;
use shortcuts::ShortcutSettings;
use snippets::SnippetSettings;

pub(crate) const SECTIONS: &[(&str, &str, &str)] = &[
    ("appearance", "外观", icon::PALETTE),
    ("chat", "对话与编辑", icon::CHAT),
    ("notify", "提醒", icon::BELL),
    ("shortcuts", "快捷键", icon::KEYBOARD),
    ("rules", "自动批准", icon::CHECK),
    ("snippets", "片段", icon::LINES),
    ("hooks", "自动执行脚本", icon::TERMINAL),
    ("git", "Git 与 PR", icon::BRANCH),
    ("platforms", "技能来源", icon::SPARK),
    ("about", "数据与关于", icon::INFO),
];

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
    let mut defaults = json!({"v":2,"theme":"system","fontSize":12.5,"minimap":false,"wordWrap":true,"sendKey":"enter"});
    if let Some(Value::Object(mut saved)) = storage::load::<Value>("blazar.ui") {
        if saved.get("v").and_then(Value::as_u64) != Some(2) {
            saved.remove("minimap");
            saved.remove("wordWrap");
            saved.remove("v");
        }
        defaults.as_object_mut().unwrap().extend(saved);
    }
    defaults
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
            toast(format!("没保存上：{e}"));
            false
        }
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let app = use_app();
    let query = use_query_map();
    let section = move || query.with(|q| q.get("s").unwrap_or_else(|| "appearance".into()));
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
            <PageHead title="设置" />
            <div class="settings-body">
                {move || match section().as_str() {
                    "chat" => {
                        view! {
                            <ChatSettings />
                            <FleetSettings />
                        }
                            .into_any()
                    }
                    "notify" => view! { <NotifySettings /> }.into_any(),
                    "shortcuts" => view! { <ShortcutSettings /> }.into_any(),
                    "rules" => view! { <RuleSettings /> }.into_any(),
                    "snippets" => view! { <SnippetSettings /> }.into_any(),
                    "hooks" => view! { <HookSettings /> }.into_any(),
                    "git" => view! { <GitSettings /> }.into_any(),
                    "platforms" => view! { <PlatformSettings /> }.into_any(),
                    "about" => {
                        view! {
                            <HubSettings />
                            <super::transfer::TransferSettings />
                            <AboutSettings />
                        }
                            .into_any()
                    }
                    "lark" => {
                        view! {
                            <a class="btn" href="/apps/lark">
                                "打开飞书 / Lark 设置"
                            </a>
                        }
                            .into_any()
                    }
                    _ => view! { <AppearanceSettings /> }.into_any(),
                }}
            </div>
        </div>
    }
}
