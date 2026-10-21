use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::components::{
    dialog::{Choice, ask},
    toast::toast,
};
use crate::{api, app_state::use_app};

use super::{flag, rows, save, text};

#[component]
pub(super) fn RuleSettings() -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let tool = RwSignal::new(String::new());
    let pattern = RwSignal::new(String::new());
    let ws = RwSignal::new(String::new());
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/approval-rules")
    });
    view! {
        <section class="card settings-card" data-ui-guard="">
            <h3>"自动批准规则"</h3>
            <p class="muted">"匹配的操作无需逐次批准，仍会记录在对话中。"</p>
            <details class="settings-details">
                <summary>"规则匹配说明"</summary>
                <p class="muted">
                    "默认没有规则；含 shell 控制符的命令不会自动批准。"
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
                                title="暂无自动批准规则"
                                detail="需要时，填写可信任的工具和适用范围。"
                            />
                        }
                            .into_any()
                    } else {
                        list.into_iter()
                            .map(move |r| {
                                let id = text(&r, "id");
                                let del = id.clone();
                                view! {
                                    <div class="settings-row">
                                        <label class="settings-actions">
                                            <input
                                                type="checkbox"
                                                aria-label="启用规则"
                                                prop:checked=flag(&r, "enabled")
                                                disabled=move || busy.get()
                                                on:change=move |e| {
                                                    let enabled = event_target_checked(&e);
                                                    let id = id.clone();
                                                    busy.set(true);
                                                    leptos::task::spawn_local(async move {
                                                        save(
                                                                "PUT",
                                                                &format!("/api/approval-rules/{}", api::enc(&id)),
                                                                json!({"enabled":enabled}),
                                                            )
                                                            .await;
                                                        rev.update(|n| *n += 1);
                                                        busy.set(false);
                                                    });
                                                }
                                            />
                                            <code>
                                                {format!("{} {}", text(&r, "tool"), text(&r, "pattern"))}
                                            </code>
                                            <span class="muted">
                                                {format!(
                                                    "{} · 命中 {} 次",
                                                    r["workspace_name"]
                                                        .as_str()
                                                        .unwrap_or(
                                                            if r["workspace_id"].is_null() {
                                                                "全局"
                                                            } else {
                                                                "工作区已删除"
                                                            },
                                                        ),
                                                    r["hits"],
                                                )}
                                            </span>
                                        </label>
                                        <button
                                            class="btn danger"
                                            disabled=move || busy.get()
                                            on:click=move |_| {
                                                let id = del.clone();
                                                busy.set(true);
                                                leptos::task::spawn_local(async move {
                                                    if ask(
                                                            "删除自动批准规则",
                                                            "删除后，匹配的操作将按当前权限模式处理。",
                                                            vec![
                                                                Choice::plain("保留规则"),
                                                                Choice::danger("删除规则"),
                                                            ],
                                                        )
                                                        .await == Some(1)
                                                        && save(
                                                                "DELETE",
                                                                &format!("/api/approval-rules/{}", api::enc(&id)),
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
                    if tool.get_untracked().trim().is_empty() {
                        toast("请填写工具名称");
                        return;
                    }
                    busy.set(true);
                    leptos::task::spawn_local(async move {
                        let workspace = ws.get_untracked();
                        if save(
                                "POST",
                                "/api/approval-rules",
                                json!(
                                    {"tool":tool.get_untracked().trim(),"pattern":pattern.get_untracked().trim(),"workspace_id":if workspace.is_empty(){None}else{Some(workspace)}}
                                ),
                            )
                            .await
                        {
                            tool.set(String::new());
                            pattern.set(String::new());
                            rev.update(|n| *n += 1);
                        }
                        busy.set(false);
                    });
                }
            >
                <fieldset disabled=move || busy.get()>
                    <input
                        class="settings-input"
                        aria-label="工具名称"
                        placeholder="Read / Edit / Bash / mcp__x__*"
                        required
                        prop:value=move || tool.get()
                        on:input=move |e| tool.set(event_target_value(&e))
                    />
                    <input
                        class="settings-input"
                        aria-label="路径或命令前缀"
                        placeholder="路径或命令前缀（可空）"
                        prop:value=move || pattern.get()
                        on:input=move |e| pattern.set(event_target_value(&e))
                    />
                    <select
                        class="settings-input"
                        aria-label="规则范围"
                        prop:value=move || ws.get()
                        on:change=move |e| ws.set(event_target_value(&e))
                    >
                        <option value="" selected=move || ws.get().is_empty()>
                            "全局"
                        </option>
                        {move || {
                            app.in_tree_order()
                                .into_iter()
                                .map(|(depth, w)| {
                                    let id = w.id.clone();
                                    let label = crate::app_state::indented_label(depth, &w);
                                    view! {
                                        <option value=w.id.clone() selected=move || ws.get() == id>
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                    <div>
                        <button class="btn primary" type="submit">
                            {move || if busy.get() { "保存中…" } else { "添加规则" }}
                        </button>
                    </div>
                </fieldset>
            </form>
        </section>
    }
}
