use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::status::{InlineError, LoadingState};

use super::{flag, save, text};

#[component]
pub(super) fn GitSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/settings"));
    move || match data.get() {
        None => view! { <LoadingState text="正在加载设置…" /> }.into_any(),
        Some(Err(e)) => view! {
            <InlineError
                message=format!("无法加载设置：{e}")
                retry=Callback::new(move |_| data.refetch())
            />
        }
        .into_any(),
        Some(Ok(p)) => view! { <GitForm initial=p["git"].clone() /> }.into_any(),
    }
}

#[component]
fn GitForm(initial: Value) -> impl IntoView {
    let prefix = RwSignal::new(text(&initial, "branch_prefix"));
    let draft = RwSignal::new(flag(&initial, "pr_draft"));
    let ai = RwSignal::new(flag(&initial, "ai_draft"));
    let busy = RwSignal::new(false);
    let failed = RwSignal::new(false);
    view! {
        <section class="card settings-card">
            <h3>"Git 与 Pull Request"</h3>
            <form
                class="settings-form"
                on:submit=move |e| {
                    e.prevent_default();
                    if busy.get_untracked() {
                        return;
                    }
                    busy.set(true);
                    failed.set(false);
                    leptos::task::spawn_local(async move {
                        let saved = save(
                                "PUT",
                                "/api/settings",
                                json!(
                                    {"git":{"branch_prefix":prefix.get_untracked().trim(),"pr_draft":draft.get_untracked(),"ai_draft":ai.get_untracked()}}
                                ),
                            )
                            .await;
                        failed.set(!saved);
                        busy.set(false);
                    });
                }
            >
                <fieldset disabled=move || busy.get()>
                    <label class="settings-field">
                        "隔离工作区分支前缀"
                        <input
                            class="settings-input"
                            prop:value=move || prefix.get()
                            on:input=move |e| prefix.set(event_target_value(&e))
                        />
                    </label>
                    <span class="muted">
                        {move || format!("示例：{}fix-login", prefix.get())}
                    </span>
                    <label>
                        <input
                            type="checkbox"
                            prop:checked=move || draft.get()
                            on:change=move |e| draft.set(event_target_checked(&e))
                        />
                        " 默认创建草稿 PR"
                    </label>
                    <label>
                        <input
                            type="checkbox"
                            prop:checked=move || ai.get()
                            on:change=move |e| ai.set(event_target_checked(&e))
                        />
                        " 启用 AI 起草提交信息和 PR 描述"
                    </label>
                    <div>
                        <button type="submit" class="btn primary">
                            {move || if busy.get() { "保存中…" } else { "保存设置" }}
                        </button>
                    </div>
                </fieldset>
            </form>
            <Show when=move || failed.get()>
                <InlineError message="未能保存设置，请重试。" />
            </Show>
        </section>
    }
}
