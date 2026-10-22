use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use serde_json::{Value, json};

use crate::components::page_head::PageHead;
use crate::components::{
    dialog::{Choice, ask},
    toast::toast,
};
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
    toast("已保存");
}

pub(super) async fn save(method: &str, path: &str, body: Value) -> bool {
    match api::send::<Value>(method, path, &body).await {
        Ok(_) => {
            toast("已保存");
            true
        }
        Err(e) => {
            toast(e.to_string());
            false
        }
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let query = use_query_map();
    let refresh = RwSignal::new(0u32);
    let reloading = RwSignal::new(false);
    let section = move || query.with(|q| q.get("s").unwrap_or_else(|| "appearance".into()));
    view! {
        <div class="page settings-page">
            <PageHead title="设置">
                <button
                    class="btn ghost"
                    disabled=move || reloading.get()
                    on:click=move |_| {
                        if reloading.get_untracked() {
                            return;
                        }
                        reloading.set(true);
                        leptos::task::spawn_local(async move {
                            if ask(
                                    "重新加载设置",
                                    "尚未保存的表单内容将丢失。",
                                    vec![
                                        Choice::plain("继续编辑"),
                                        Choice::danger("重新加载"),
                                    ],
                                )
                                .await == Some(1)
                            {
                                refresh.update(|n| *n += 1);
                            }
                            reloading.set(false);
                        });
                    }
                >
                    "重新加载"
                </button>
            </PageHead>
            <div class="settings-layout">
                <nav class="settings-nav" aria-label="设置分类">
                    {[
                        ("appearance", "外观"),
                        ("chat", "对话与编辑"),
                        ("notify", "提醒"),
                        ("shortcuts", "快捷键"),
                        ("rules", "自动批准"),
                        ("snippets", "片段"),
                        ("hooks", "自动执行脚本"),
                        ("git", "Git 与 PR"),
                        ("platforms", "技能来源"),
                        ("about", "数据与关于"),
                    ]
                        .into_iter()
                        .map(move |(id, label)| {
                            view! {
                                <a
                                    href=format!("/settings?s={id}")
                                    aria-current=move || (section() == id).then_some("page")
                                >
                                    {label}
                                </a>
                            }
                        })
                        .collect_view()}
                </nav>
                <div class="settings-body">
                    {move || {
                        refresh.get();
                        match section().as_str() {
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
                        }
                    }}
                </div>
            </div>
        </div>
    }
}
