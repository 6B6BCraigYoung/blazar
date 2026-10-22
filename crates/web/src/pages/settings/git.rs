use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::{PrefGroup, PrefRow, Switch};
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
    let put = move |body: Value, undo: Box<dyn FnOnce() + 'static>| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            if !save("PUT", "/api/settings", body).await {
                undo();
            }
            busy.set(false);
        });
    };
    view! {
        <PrefGroup title="Git 与 Pull Request">
            <PrefRow
                label="分支前缀"
                detail="新建隔离工作区时，分支名以它开头。"
                for_id="settings-branch-prefix"
            >
                <div class="pref-stack">
                    <input
                        id="settings-branch-prefix"
                        class="settings-input"
                        autocomplete="off"
                        spellcheck="false"
                        disabled=move || busy.get()
                        prop:value=move || prefix.get()
                        on:change=move |e| {
                            let next = event_target_value(&e).trim().to_owned();
                            let old = prefix.get_untracked();
                            if next == old {
                                event_target::<web_sys::HtmlInputElement>(&e).set_value(&next);
                                return;
                            }
                            prefix.set(next.clone());
                            put(
                                json!({"git":{"branch_prefix":next}}),
                                Box::new(move || prefix.set(old)),
                            );
                        }
                    />
                    <span class="pref-hint">
                        {move || format!("示例：{}fix-login", prefix.get())}
                    </span>
                </div>
            </PrefRow>
            <PrefRow label="默认创建草稿 PR" for_id="settings-pr-draft">
                <Switch
                    id="settings-pr-draft"
                    checked=draft
                    disabled=busy
                    on_change=move |v: bool| {
                        draft.set(v);
                        put(json!({"git":{"pr_draft":v}}), Box::new(move || draft.set(!v)));
                    }
                />
            </PrefRow>
            <PrefRow label="AI 起草提交信息和 PR 描述" for_id="settings-ai-draft">
                <Switch
                    id="settings-ai-draft"
                    checked=ai
                    disabled=busy
                    on_change=move |v: bool| {
                        ai.set(v);
                        put(json!({"git":{"ai_draft":v}}), Box::new(move || ai.set(!v)));
                    }
                />
            </PrefRow>
        </PrefGroup>
    }
}
