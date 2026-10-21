use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::components::{
    dialog::{Choice, ask},
    toast::toast,
};

use super::{flag, rows, save, text};

fn hook_event_label(event: &str) -> &str {
    match event {
        "turn_start" => "一轮开始前",
        "turn_end" => "一轮结束",
        "session_start" => "会话建立",
        "session_end" => "会话结束",
        "error" => "出错",
        other => other,
    }
}

fn hook_target_label(target: &str) -> &str {
    match target {
        "hub" => "Blazar 所在机器",
        "node" => "工作区所在机器",
        other => other,
    }
}

#[component]
pub(super) fn HookSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let event = RwSignal::new("turn_start".to_owned());
    let target = RwSignal::new("hub".to_owned());
    let command = RwSignal::new(String::new());
    let timeout = RwSignal::new(30u32);
    let blocking = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/hooks")
    });
    view! {
        <section class="card settings-card">
            <h3>"自动执行脚本"</h3>
            <p class="muted">"在会话开始、结束或出错时执行脚本。"</p>
            <details class="settings-details">
                <summary>"执行说明与变量"</summary>
                <p class="muted">
                    "一轮开始前的脚本可在失败时阻止执行。变量：BLAZAR_EVENT、BLAZAR_WORKSPACE、BLAZAR_NODE、BLAZAR_CWD、BLAZAR_SESSION、BLAZAR_TOOL。"
                </p>
            </details>
            {move || match data.get() {
                None => view! { <LoadingState text="正在加载设置…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=format!("无法加载设置：{e}")
                            retry=Callback::new(move |_| data.refetch())
                        />
                    }
                        .into_any()
                }
                Some(Ok(v)) => {
                    let list = rows(&v);
                    if list.is_empty() {
                        view! {
                            <EmptyState
                                title="暂无自动执行脚本"
                                detail="需要时，添加脚本并选择执行时点。"
                            />
                        }
                            .into_any()
                    } else {
                        list.into_iter()
                            .map(move |h| {
                                let id = text(&h, "id");
                                view! {
                                    <div class="settings-row">
                                        <div class="settings-label">
                                            <code>{text(&h, "command")}</code>
                                            <span>
                                                {format!(
                                                    "{} · {}{}",
                                                    hook_event_label(&text(&h, "event")),
                                                    hook_target_label(&text(&h, "target")),
                                                    if flag(&h, "blocking") {
                                                        " · 失败时阻止执行"
                                                    } else {
                                                        ""
                                                    },
                                                )}
                                            </span>
                                        </div>
                                        <button
                                            class="btn danger"
                                            disabled=move || busy.get()
                                            on:click=move |_| {
                                                let id = id.clone();
                                                busy.set(true);
                                                leptos::task::spawn_local(async move {
                                                    if ask(
                                                            "删除自动执行脚本",
                                                            "删除后不再自动执行这个脚本。",
                                                            vec![
                                                                Choice::plain("保留脚本"),
                                                                Choice::danger("删除脚本"),
                                                            ],
                                                        )
                                                        .await == Some(1)
                                                        && save(
                                                                "DELETE",
                                                                &format!("/api/hooks/{}", api::enc(&id)),
                                                                json!({}),
                                                            )
                                                            .await
                                                    {
                                                        rev.update(|n| *n += 1);
                                                    }
                                                    busy.set(false);
                                                });
                                            }
                                        >
                                            "删除"
                                        </button>
                                    </div>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }
            }}
            <form
                class="settings-form"
                on:submit=move |e| {
                    e.prevent_default();
                    if busy.get_untracked() {
                        return;
                    }
                    if command.get_untracked().trim().is_empty() {
                        toast("请填写命令");
                        return;
                    }
                    busy.set(true);
                    leptos::task::spawn_local(async move {
                        if save(
                                "POST",
                                "/api/hooks",
                                json!(
                                    {"event":event.get_untracked(),"target":target.get_untracked(),"command":command.get_untracked().trim(),"blocking":blocking.get_untracked()&&event.get_untracked()=="turn_start","timeout_secs":timeout.get_untracked()}
                                ),
                            )
                            .await
                        {
                            command.set(String::new());
                            rev.update(|n| *n += 1);
                        }
                        busy.set(false);
                    });
                }
            >
                <fieldset disabled=move || busy.get()>
                    <label class="settings-field">
                        "时点"
                        <select
                            class="settings-input"
                            prop:value=move || event.get()
                            on:change=move |e| event.set(event_target_value(&e))
                        >
                            <option value="turn_start" selected=move || event.get() == "turn_start">
                                "一轮开始前"
                            </option>
                            <option value="turn_end" selected=move || event.get() == "turn_end">
                                "一轮结束"
                            </option>
                            <option
                                value="session_start"
                                selected=move || event.get() == "session_start"
                            >
                                "会话建立"
                            </option>
                            <option
                                value="session_end"
                                selected=move || event.get() == "session_end"
                            >
                                "会话结束"
                            </option>
                            <option value="error" selected=move || event.get() == "error">
                                "出错"
                            </option>
                        </select>
                    </label>
                    <label class="settings-field">
                        "执行位置"
                        <select
                            class="settings-input"
                            prop:value=move || target.get()
                            on:change=move |e| target.set(event_target_value(&e))
                        >
                            <option value="hub" selected=move || target.get() == "hub">
                                "Blazar 所在机器"
                            </option>
                            <option value="node" selected=move || target.get() == "node">
                                "工作区所在机器"
                            </option>
                        </select>
                    </label>
                    <label class="settings-field">
                        "命令"
                        <input
                            class="settings-input"
                            required
                            prop:value=move || command.get()
                            on:input=move |e| command.set(event_target_value(&e))
                        />
                    </label>
                    <label class="settings-field">
                        "超时（秒）"
                        <input
                            class="settings-input"
                            type="number"
                            min="1"
                            max="600"
                            value="30"
                            on:input=move |e| {
                                timeout.set(event_target_value(&e).parse().unwrap_or(30))
                            }
                        />
                    </label>
                    <label>
                        <input
                            type="checkbox"
                            disabled=move || event.get() != "turn_start"
                            prop:checked=move || blocking.get()
                            on:change=move |e| blocking.set(event_target_checked(&e))
                        />
                        " 失败时阻止执行"
                    </label>
                    <div>
                        <button class="btn primary" type="submit">
                            {move || if busy.get() { "保存中…" } else { "添加脚本" }}
                        </button>
                    </div>
                </fieldset>
            </form>
        </section>
    }
}
