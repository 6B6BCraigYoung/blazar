use crate::components::status::StateOf;
use crate::{api, app_state::use_app, realtime::use_bus};
use blazar_core_types::api::ServerEvent;
use leptos::prelude::*;
use serde_json::{Value, json};

pub fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
pub fn arr(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
pub fn nonempty(s: String) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        Value::String(s)
    }
}
pub fn assignee(v: &Value) -> String {
    if !s(v, "agent_profile").is_empty() {
        format!("p:{}", s(v, "agent_profile"))
    } else if !s(v, "runtime").is_empty() {
        format!("r:{}", s(v, "runtime"))
    } else {
        String::new()
    }
}
pub fn assignee_patch(who: &str) -> Value {
    json!({"agent_profile": who.strip_prefix("p:"), "runtime": who.strip_prefix("r:")})
}
pub fn who(v: &Value) -> String {
    if s(v, "agent_name").is_empty() {
        s(v, "runtime")
    } else {
        format!("{} {}", s(v, "agent_avatar"), s(v, "agent_name"))
            .trim()
            .to_owned()
    }
}
pub fn meta(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}
pub fn thread_link(ws: &str, thread: &str) -> String {
    format!("/w/{}?thread={}", api::enc(ws), api::enc(thread))
}
pub fn badge(v: &str) -> AnyView {
    view! { <StateOf state=v.to_owned() /> }.into_any()
}
pub fn work_revision(topic: &'static str) -> RwSignal<u32> {
    let rev = RwSignal::new(0u32);
    let bus = use_bus();
    let id = bus.subscribe(move |e| {
        if matches!(
            (topic, e),
            ("inbox", ServerEvent::InboxChanged)
                | ("tasks", ServerEvent::TasksChanged)
                | ("autopilots", ServerEvent::AutopilotsChanged)
                | ("jobs", ServerEvent::JobsChanged)
        ) {
            rev.try_update(|n| *n = n.wrapping_add(1));
        }
    });
    on_cleanup(move || bus.unsubscribe(id));
    Effect::new(move |_| {
        bus.resyncs.track();
        rev.update(|n| *n = n.wrapping_add(1));
    });
    rev
}
pub fn refresh(rev: RwSignal<u32>) {
    rev.try_update(|n| *n = n.wrapping_add(1));
}
pub fn text_field(label: &'static str, value: RwSignal<String>, rows: u32) -> AnyView {
    if rows > 0 {
        view! {
            <label class="work-field">
                <span>{label}</span>
                <textarea
                    class="input"
                    rows=rows
                    prop:value=move || value.get()
                    on:input=move |e| value.set(event_target_value(&e))
                ></textarea>
            </label>
        }
        .into_any()
    } else {
        view! {
            <label class="work-field">
                <span>{label}</span>
                <input
                    class="input"
                    prop:value=move || value.get()
                    on:input=move |e| value.set(event_target_value(&e))
                />
            </label>
        }
        .into_any()
    }
}
pub fn select_field(
    label: &'static str,
    value: RwSignal<String>,
    options: &[(&str, &str)],
) -> AnyView {
    let options = options
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect::<Vec<_>>();
    let guarded = options.iter().any(|(k, _)| k == "bypassPermissions");
    view! {
        <label class="work-field">
            <span>{label}</span>
            <select
                class="input"
                data-ui-guard=guarded.then_some("")
                prop:value=move || value.get()
                on:change=move |e| value.set(event_target_value(&e))
            >
                {options
                    .into_iter()
                    .map(|(k, v)| {
                        let selected = k.clone();
                        view! {
                            <option value=k selected=move || value.get() == selected>
                                {v}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>
        </label>
    }
    .into_any()
}
#[component]
pub fn WorkspaceField(
    value: RwSignal<String>,
    #[prop(default = false)] locked: bool,
) -> impl IntoView {
    let app = use_app();
    view! {
        <label class="work-field">
            <span>"工作区"</span>
            <select
                class="input"
                disabled=locked
                prop:value=move || value.get()
                on:change=move |e| value.set(event_target_value(&e))
            >
                <option value="" selected=move || value.get().is_empty()>
                    "未选择"
                </option>
                {move || {
                    app.in_tree_order()
                        .into_iter()
                        .map(|(depth, w)| {
                            let selected = w.id.clone();
                            let label = crate::app_state::indented_label(depth, &w);
                            view! {
                                <option value=w.id selected=move || value.get() == selected>
                                    {label}
                                </option>
                            }
                        })
                        .collect_view()
                }}
            </select>
            {locked
                .then(|| {
                    view! { <small class="muted">"已开始的任务不能更换工作区"</small> }
                })}
        </label>
    }
}
#[component]
pub fn AssigneeField(value: RwSignal<String>) -> impl IntoView {
    let opts = LocalResource::new(|| async {
        let (profiles, runtimes) = futures::join!(
            api::get::<Vec<Value>>("/api/agent-profiles"),
            api::get::<api::Runtimes>("/api/runtimes")
        );
        (profiles, runtimes)
    });
    view! {
        <label class="work-field">
            <span>"指派给"</span>
            <select
                class="input"
                prop:value=move || value.get()
                on:change=move |e| value.set(event_target_value(&e))
            >
                <option value="" selected=move || value.get().is_empty()>
                    "未指派"
                </option>
                {move || {
                    opts.get()
                        .map(|o| {
                            let mut choices = Vec::<(String, String)>::new();
                            if let Ok(p) = &o.0 {
                                for a in p {
                                    choices
                                        .push((
                                            format!("p:{}", s(a, "id")),
                                            format!("{} {}", s(a, "avatar"), s(a, "name")),
                                        ));
                                }
                            }
                            if let Ok(r) = &o.1 {
                                for a in &r.runtimes {
                                    if a.installed {
                                        choices.push((format!("r:{}", a.id), a.label.clone()));
                                    }
                                }
                            }
                            let current = value.get_untracked();
                            if !current.is_empty() && !choices.iter().any(|(k, _)| k == &current) {
                                choices
                                    .push((
                                        current.clone(),
                                        format!("{current}（当前指派）"),
                                    ));
                            }
                            choices
                                .into_iter()
                                .map(|(k, v)| {
                                    let selected = k.clone();
                                    view! {
                                        <option value=k selected=move || value.get() == selected>
                                            {v}
                                        </option>
                                    }
                                })
                                .collect_view()
                        })
                }}
            </select>
            {move || {
                opts.get()
                    .and_then(|o| {
                        o.0
                            .as_ref()
                            .err()
                            .or_else(|| o.1.as_ref().err())
                            .map(|e| {
                                view! {
                                    <small class="bad">
                                        {format!("指派列表加载失败：{e}")}
                                    </small>
                                }
                            })
                    })
            }}
        </label>
    }
}
pub async fn copy(text: String) {
    match wasm_bindgen_futures::JsFuture::from(window().navigator().clipboard().write_text(&text))
        .await
    {
        Ok(_) => crate::components::toast::toast("已复制"),
        Err(_) => crate::components::toast::toast("复制失败，请手动选中复制"),
    }
}
