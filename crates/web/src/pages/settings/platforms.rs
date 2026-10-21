use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::dialog::{Choice, ask};
use crate::components::status::{InlineError, LoadingState};

use super::{flag, rows, save, text};

#[component]
pub(super) fn PlatformSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let key = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/skills/market/providers")
    });
    view! {
        <section class="card settings-card">
            <h3>"技能来源"</h3>
            <p>
                <a href="/skills?tab=market">"搜索和安装技能 →"</a>
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
                    rows(&v)
                        .into_iter()
                        .map(|p| {
                            view! {
                                <div class="settings-row">
                                    <div class="settings-label">
                                        <b>{text(&p, "label")}</b>
                                        <span>{text(&p, "note")}</span>
                                    </div>
                                    <span>
                                        {if flag(&p, "has_key") {
                                            "已配置密钥"
                                        } else {
                                            "未配置密钥"
                                        }}
                                    </span>
                                    <a
                                        href=text(&p, "home")
                                        target="_blank"
                                        rel="noopener noreferrer"
                                    >
                                        "打开 ↗"
                                    </a>
                                </div>
                            }
                        })
                        .collect_view()
                        .into_any()
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
                        if key.get_untracked().trim().is_empty()
                            && ask(
                                    "删除技能来源密钥",
                                    "留空保存将删除已有的 SkillsMP API key。",
                                    vec![
                                        Choice::plain("保留密钥"),
                                        Choice::danger("删除密钥"),
                                    ],
                                )
                                .await != Some(1)
                        {
                            busy.set(false);
                            return;
                        }
                        if save(
                                "PUT",
                                "/api/skills/market/key",
                                json!({"provider":"skillsmp","key":key.get_untracked().trim()}),
                            )
                            .await
                        {
                            key.set(String::new());
                            rev.update(|n| *n += 1);
                        }
                        busy.set(false);
                    });
                }
            >
                <p class="muted">"密钥只写不读。留空保存会删除已有密钥。"</p>
                <label class="settings-field">
                    "SkillsMP API key"
                    <input
                        class="settings-input"
                        type="password"
                        autocomplete="new-password"
                        prop:value=move || key.get()
                        disabled=move || busy.get()
                        on:input=move |e| key.set(event_target_value(&e))
                    />
                </label>
                <div>
                    <button class="btn primary" type="submit" disabled=move || busy.get()>
                        {move || if busy.get() { "保存中…" } else { "保存密钥" }}
                    </button>
                </div>
            </form>
        </section>
    }
}
