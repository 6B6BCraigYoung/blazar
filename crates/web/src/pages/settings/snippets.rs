use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::dialog::{Choice, ask};
use crate::components::status::{EmptyState, InlineError, LoadingState};

use super::{rows, save, text};

#[component]
pub(super) fn SnippetSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let edit = RwSignal::new(None::<String>);
    let name = RwSignal::new(String::new());
    let body = RwSignal::new(String::new());
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/snippets")
    });
    view! {
        <section class="card settings-card">
            <h3>"片段"</h3>
            <p class="muted">
                "把验收清单、代码规范等常用内容存下来，在对话里输入 @ 选择片段。"
            </p>
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
                                title="保存第一个片段"
                                detail="添加常用内容，在对话中输入 @ 即可引用。"
                            />
                        }
                            .into_any()
                    } else {
                        list.into_iter()
                            .map(move |s| {
                                let id = text(&s, "id");
                                let editing = s.clone();
                                view! {
                                    <div class="settings-row">
                                        <div class="settings-label">
                                            <b>{format!("@{}", text(&s, "name"))}</b>
                                            <span>
                                                {text(&s, "body").chars().take(100).collect::<String>()}
                                            </span>
                                        </div>
                                        <button
                                            class="btn"
                                            disabled=move || busy.get()
                                            on:click=move |_| {
                                                edit.set(Some(text(&editing, "id")));
                                                name.set(text(&editing, "name"));
                                                body.set(text(&editing, "body"));
                                            }
                                        >
                                            "编辑"
                                        </button>
                                        <button
                                            class="btn danger"
                                            disabled=move || busy.get()
                                            on:click=move |_| {
                                                let id = id.clone();
                                                busy.set(true);
                                                leptos::task::spawn_local(async move {
                                                    if ask(
                                                            "删除片段",
                                                            "删除后不能恢复。",
                                                            vec![Choice::plain("取消"), Choice::danger("删除")],
                                                        )
                                                        .await == Some(1)
                                                        && save(
                                                                "DELETE",
                                                                &format!("/api/snippets/{}", api::enc(&id)),
                                                                json!({}),
                                                            )
                                                            .await
                                                    {
                                                        if edit.get_untracked().as_ref() == Some(&id) {
                                                            edit.set(None);
                                                            name.set(String::new());
                                                            body.set(String::new());
                                                        }
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
                    busy.set(true);
                    leptos::task::spawn_local(async move {
                        let id = edit.get_untracked();
                        let path = id
                            .as_ref()
                            .map(|id| format!("/api/snippets/{}", api::enc(id)))
                            .unwrap_or_else(|| "/api/snippets".into());
                        if save(
                                if id.is_some() { "PUT" } else { "POST" },
                                &path,
                                json!(
                                    {"name":name.get_untracked().trim(),"body":body.get_untracked()}
                                ),
                            )
                            .await
                        {
                            edit.set(None);
                            name.set(String::new());
                            body.set(String::new());
                            rev.update(|n| *n += 1);
                        }
                        busy.set(false);
                    });
                }
            >
                <fieldset disabled=move || busy.get()>
                    <label class="settings-field">
                        "名称"
                        <input
                            class="settings-input"
                            required
                            maxlength="40"
                            prop:value=move || name.get()
                            on:input=move |e| name.set(event_target_value(&e))
                        />
                    </label>
                    <label class="settings-field">
                        "内容"
                        <textarea
                            class="settings-textarea"
                            required
                            prop:value=move || body.get()
                            on:input=move |e| body.set(event_target_value(&e))
                        ></textarea>
                    </label>
                    <div class="settings-actions">
                        <button type="submit" class="btn primary">
                            {move || {
                                if busy.get() {
                                    "保存中…"
                                } else if edit.get().is_some() {
                                    "保存片段"
                                } else {
                                    "新建片段"
                                }
                            }}
                        </button>
                        <button
                            type="button"
                            class="btn"
                            on:click=move |_| {
                                edit.set(None);
                                name.set(String::new());
                                body.set(String::new());
                            }
                        >
                            {move || {
                                if edit.get().is_some() { "取消编辑" } else { "清空内容" }
                            }}
                        </button>
                    </div>
                </fieldset>
            </form>
        </section>
    }
}
