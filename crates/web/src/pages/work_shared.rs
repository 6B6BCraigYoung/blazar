use crate::action_gate::ActionGate;
use crate::components::status::StateOf;
use crate::components::toast::toast;
use crate::{api, app_state::use_app, realtime::use_bus};
use blazar_core_types::api::ServerEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

pub fn decide_approval(
    gate: RwSignal<ActionGate>,
    id: String,
    allow: bool,
    answers: Option<Value>,
    done: impl Fn() + 'static,
) {
    send_decision(gate, id, allow, answers, true, done);
}

fn send_decision(
    gate: RwSignal<ActionGate>,
    id: String,
    allow: bool,
    answers: Option<Value>,
    announce: bool,
    done: impl Fn() + 'static,
) {
    if !gate.write().begin(&id) {
        return;
    }
    spawn_local(async move {
        let mut body = json!({ "allow": allow, "message": "" });
        if let Some(a) = answers {
            body["answers"] = a;
        }
        match api::send::<Value>("POST", &format!("/api/approvals/{id}"), &body).await {
            Ok(r) if r["delivered"].as_bool() == Some(true) => {
                if announce {
                    toast(if allow { "已允许" } else { "已拒绝" });
                }
            }
            Ok(r) => toast(r["reason"].as_str().unwrap_or("没能送达").to_owned()),
            Err(e) => toast(e.to_string()),
        }
        let _ = gate.try_update(|g| {
            g.complete(&id, false);
        });
        done();
    });
}

pub fn allow_always(
    gate: RwSignal<ActionGate>,
    id: String,
    request: Value,
    workspace: String,
    done: impl Fn() + 'static,
) {
    spawn_local(async move {
        let (tool, pattern) = crate::chat_model::always_rule(&request);
        let what = if pattern.is_empty() {
            tool.clone()
        } else {
            format!("{tool} {pattern}…")
        };
        let ok = crate::components::dialog::ask(
            "不再询问",
            &format!(
                "在这个工作区里总是允许「{what}」？\n\n以后同样的操作不再问你。在「设置 → 自动批准」里能看到这条规则，随时关掉或删除。"
            ),
            vec![
                crate::components::dialog::Choice::plain("取消"),
                crate::components::dialog::Choice::plain("总是允许"),
            ],
        )
        .await;
        if ok != Some(1) {
            return;
        }
        match api::send::<Value>(
            "POST",
            "/api/approval-rules",
            &json!({ "tool": tool, "pattern": pattern, "workspace_id": workspace }),
        )
        .await
        {
            Ok(r) => {
                let rule = r["id"].as_str().unwrap_or_default().to_owned();
                send_decision(gate, id, true, None, false, move || {
                    offer_rule_undo(rule.clone(), what.clone());
                    done();
                });
            }
            Err(e) => toast(crate::vocab::error(&e.to_string(), None)),
        }
    });
}

/// A new always-allow rule is announced with a way to take it back on the spot; the
/// approval itself stays allowed, only later ones go back to asking.
fn offer_rule_undo(rule: String, what: String) {
    if rule.is_empty() {
        toast(format!("已允许，以后不再问「{what}」"));
        return;
    }
    crate::components::toast::offer(
        format!("已允许，以后不再问「{what}」"),
        "撤销",
        8000,
        move || {
            let rule = rule.clone();
            spawn_local(async move {
                let path = format!("/api/approval-rules/{}", api::enc(&rule));
                match api::send::<Value>("DELETE", &path, &json!({})).await {
                    Ok(_) => toast("下次还会问你"),
                    Err(e) => toast(crate::vocab::error(&e.to_string(), None)),
                }
            });
        },
    );
}

pub fn approval_key_hint(i: usize) -> String {
    (i + 1).to_string()
}

pub use crate::components::rows::{in_field, list_keys_apply, overlay_open};

pub fn approval_summary(request: &Value, root: &str) -> (String, String) {
    let tool = crate::chat_model::tool_name(request["tool_name"].as_str().unwrap_or(""));
    if tool == "AskUserQuestion" {
        let q = request["input"]["questions"][0]["question"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        return ("提问".into(), q);
    }
    let label = if crate::chat_model::SHELL_TOOLS.contains(&tool.as_str()) {
        "Bash".to_owned()
    } else if tool.is_empty() {
        "工具".to_owned()
    } else if crate::chat_model::is_fleet(request["tool_name"].as_str().unwrap_or("")) {
        fleet_label(&tool, &request["input"])
    } else {
        tool
    };
    (label, crate::chat_model::approval_what(request, root))
}

/// Outside a conversation a fleet call reads in Chinese and names where the work goes,
/// e.g. 「派活 → nebula-web」; the English tool words stay inside the chat.
fn fleet_label(tool: &str, input: &Value) -> String {
    let verb = match tool {
        "send_prompt" => "派活",
        "copy_files" => "拷文件",
        "job_spawn" => "起后台任务",
        "create_workspace" => return "建工作区".to_owned(),
        "job_kill" => return "结束后台任务".to_owned(),
        _ => return "调度".to_owned(),
    };
    let id = crate::chat_model::fleet_target(tool, input).unwrap_or_default();
    let name = use_context::<crate::app_state::AppData>()
        .and_then(|app| app.workspace(&id))
        .map(|w| w.name);
    match name {
        Some(n) => format!("{verb} → {n}"),
        None => verb.to_owned(),
    }
}

pub fn approval_line(request: &Value) -> String {
    let tool = crate::chat_model::tool_name(request["tool_name"].as_str().unwrap_or(""));
    let input = &request["input"];
    let field = |k: &str| {
        input[k]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.lines().next().unwrap_or_default().to_owned())
    };
    if crate::chat_model::SHELL_TOOLS.contains(&tool.as_str()) {
        return match (field("description"), field("command")) {
            (Some(d), _) => d,
            (None, Some(c)) => format!("运行 {}", c.chars().take(60).collect::<String>()),
            (None, None) => "运行一条命令".to_owned(),
        };
    }
    let file = field("file_path")
        .or_else(|| field("notebook_path"))
        .or_else(|| field("path"));
    let base = |f: String| f.rsplit('/').next().unwrap_or(&f).to_owned();
    match tool.as_str() {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            file.map_or_else(|| "改一个文件".to_owned(), |f| format!("改 {}", base(f)))
        }
        "Read" => file.map_or_else(|| "读一个文件".to_owned(), |f| format!("读 {}", base(f))),
        "WebFetch" | "WebSearch" => "访问网页".to_owned(),
        _ => {
            let (label, what) = approval_summary(request, "");
            if what.is_empty() {
                label
            } else {
                format!("{label} {what}")
            }
        }
    }
}

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
pub fn filter_select(
    label: &'static str,
    value: RwSignal<String>,
    options: &[(&str, &str)],
) -> AnyView {
    let options = options
        .iter()
        .map(|(k, v)| (k.to_string(), format!("{label}：{v}")))
        .collect::<Vec<_>>();
    view! {
        <select
            class="filter"
            aria-label=label
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
    }
    .into_any()
}
#[component]
pub fn WorkspaceFilter(value: RwSignal<String>) -> impl IntoView {
    let app = use_app();
    view! {
        <select
            class="filter"
            aria-label="工作区"
            prop:value=move || value.get()
            on:change=move |e| value.set(event_target_value(&e))
        >
            <option value="" selected=move || value.get().is_empty()>
                "工作区：全部"
            </option>
            {move || {
                app.in_tree_order()
                    .into_iter()
                    .map(|(depth, w)| {
                        let selected = w.id.clone();
                        let label = format!(
                            "工作区：{}",
                            crate::app_state::indented_label(depth, &w),
                        );
                        view! {
                            <option value=w.id selected=move || value.get() == selected>
                                {label}
                            </option>
                        }
                    })
                    .collect_view()
            }}
        </select>
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_read_as_one_short_line() {
        let req = |tool: &str, input: Value| json!({ "tool_name": tool, "input": input });
        assert_eq!(
            approval_line(&req(
                "Bash",
                json!({ "command": "python train.py --config a.yaml", "description": "跑一轮训练" })
            )),
            "跑一轮训练"
        );
        assert_eq!(
            approval_line(&req(
                "Bash",
                json!({ "command": "python train.py --config a.yaml" })
            )),
            "运行 python train.py --config a.yaml"
        );
        assert_eq!(
            approval_line(&req("Edit", json!({ "file_path": "/srv/app/src/main.rs" }))),
            "改 main.rs"
        );
        assert_eq!(
            approval_line(&req("WebFetch", json!({ "url": "https://x.invalid" }))),
            "访问网页"
        );
        assert_eq!(approval_line(&req("Read", json!({}))), "读一个文件");
    }
}
