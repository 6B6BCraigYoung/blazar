use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::{PrefGroup, PrefRow};
use crate::components::toast::toast_error;

use super::{set_ui, text, ui_prefs};

#[component]
pub(super) fn ChatSettings() -> impl IntoView {
    let ui = ui_prefs();
    view! {
        <PrefGroup title="对话">
            <PrefRow label="发送键" for_id="settings-send">
                <select
                    id="settings-send"
                    class="settings-input"
                    prop:value=text(&ui, "sendKey")
                    on:change=|e| set_ui("sendKey", json!(event_target_value(&e)))
                >
                    <option value="enter" selected=text(&ui, "sendKey") == "enter">
                        "Enter 发送，Shift+Enter 换行"
                    </option>
                    <option value="mod" selected=text(&ui, "sendKey") == "mod">
                        "⌘ / Ctrl + Enter 发送，Enter 换行"
                    </option>
                </select>
            </PrefRow>
        </PrefGroup>
        <ModelDefaults />
    }
}

fn effort_name(e: &str) -> &str {
    match e {
        "" => "默认强度",
        "minimal" => "最低",
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        "xhigh" => "很高",
        "max" => "最高",
        "ultra" => "极限",
        other => other,
    }
}

const RUNTIMES: [(&str, &str); 2] = [("claude", "Claude Code"), ("codex", "Codex")];

/// What a new conversation starts with when its workspace has no default of its own.
#[component]
fn ModelDefaults() -> impl IntoView {
    let global = RwSignal::new(Value::Null);
    spawn_local(async move {
        if let Ok(v) = api::get::<Value>("/api/model-defaults").await {
            let _ = global.try_set(v["global"].clone());
        }
    });
    view! {
        <PrefGroup
            title="默认模型"
            note="新对话先用工作区的默认，工作区没设时用这里的。已有的对话沿用自己上一轮的模型。"
        >
            {RUNTIMES
                .iter()
                .map(|&(rt, label)| view! { <ModelDefaultRow rt label global /> })
                .collect_view()}
        </PrefGroup>
    }
}

#[component]
fn ModelDefaultRow(
    rt: &'static str,
    label: &'static str,
    global: RwSignal<Value>,
) -> impl IntoView {
    let models = RwSignal::new(Vec::<Value>::new());
    spawn_local(async move {
        if let Ok(v) = api::get::<Value>(&format!("/api/runtimes/{rt}/models")).await {
            let _ = models.try_set(v["models"].as_array().cloned().unwrap_or_default());
        }
    });
    let field = move |k: &str| global.with(|g| g[rt][k].as_str().unwrap_or_default().to_owned());
    let efforts = move || {
        let id = field("model");
        models.with(|l| {
            l.iter()
                .find(|m| m["id"].as_str().unwrap_or_default() == id)
                .or_else(|| l.first())
                .and_then(|m| m["efforts"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|e| e.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
    };
    let save = move |model: String, effort: String| {
        let body = json!({
            "runtime": rt,
            "model": Some(model.clone()).filter(|m| !m.is_empty()),
            "effort": Some(effort.clone()).filter(|e| !e.is_empty()),
        });
        global.update(|g| {
            if !g.is_object() {
                *g = json!({});
            }
            g[rt] = json!({ "model": model, "effort": effort });
        });
        spawn_local(async move {
            if let Err(e) = api::send::<Value>("PUT", "/api/model-defaults", &body).await {
                toast_error("默认模型没存上", &e);
            }
        });
    };
    let model_id = format!("settings-model-{rt}");
    view! {
        <PrefRow label=label for_id=model_id.clone()>
            <span class="settings-pair">
                <select
                    id=model_id
                    class="settings-input"
                    aria-label=format!("{label} 的默认模型")
                    on:change=move |e| {
                        let m = event_target_value(&e);
                        let keep = field("effort");
                        save(m, keep);
                    }
                >
                    {move || {
                        let cur = field("model");
                        models
                            .get()
                            .into_iter()
                            .map(|m| {
                                let id = m["id"].as_str().unwrap_or_default().to_owned();
                                let name = m["label"].as_str().unwrap_or_default().to_owned();
                                let name = if id.is_empty() {
                                    "跟随运行时默认".to_owned()
                                } else {
                                    name
                                };
                                let on = id == cur;
                                view! {
                                    <option value=id selected=on>
                                        {name}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
                <select
                    class="settings-input"
                    aria-label=format!("{label} 的默认思考强度")
                    on:change=move |e| {
                        let effort = event_target_value(&e);
                        save(field("model"), effort);
                    }
                >
                    {move || {
                        let cur = field("effort");
                        std::iter::once(String::new())
                            .chain(efforts())
                            .map(|e| {
                                let on = e == cur;
                                let name = effort_name(&e).to_owned();
                                view! {
                                    <option value=e selected=on>
                                        {name}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
            </span>
        </PrefRow>
    }
}
