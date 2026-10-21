use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::markdown_mode::{self, MarkdownMode};
use crate::{api, storage};

use super::{flag, save, set_ui, text, ui_prefs};

#[component]
pub(super) fn ChatSettings() -> impl IntoView {
    let ui = ui_prefs();
    let diff =
        storage::load::<Value>("blazar.diffprefs").unwrap_or(json!({"view":"unified","w":false}));
    let md = markdown_mode::load().as_str();
    let set_diff = |k: &str, v: Value| {
        let mut d = storage::load::<Value>("blazar.diffprefs").unwrap_or(json!({}));
        d[k] = v;
        storage::save("blazar.diffprefs", &d);
        toast("已保存");
    };
    view! {
        <section class="card settings-card">
            <h3>"对话与编辑"</h3>
            <div class="settings-row">
                <label for="settings-send">"发送键"</label>
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
            </div>
            <div class="settings-row">
                <label for="settings-md">"打开 Markdown 时"</label>
                <select
                    id="settings-md"
                    class="settings-input"
                    prop:value=md
                    on:change=|e| {
                        markdown_mode::save(
                            MarkdownMode::from_stored(Some(&event_target_value(&e))),
                        );
                        toast("已保存");
                    }
                >
                    <option value="preview" selected=md == "preview">
                        "先看预览"
                    </option>
                    <option value="source" selected=md == "source">
                        "先看源码"
                    </option>
                </select>
            </div>
            <div class="settings-row">
                <label for="settings-diff">"差异默认视图"</label>
                <select
                    id="settings-diff"
                    class="settings-input"
                    prop:value=text(&diff, "view")
                    on:change=move |e| set_diff("view", json!(event_target_value(&e)))
                >
                    <option value="unified" selected=text(&diff, "view") == "unified">
                        "统一"
                    </option>
                    <option value="split" selected=text(&diff, "view") == "split">
                        "并排"
                    </option>
                </select>
            </div>
            <div class="settings-row">
                <label for="settings-whitespace">"差异里忽略空白"</label>
                <input
                    id="settings-whitespace"
                    type="checkbox"
                    prop:checked=flag(&diff, "w")
                    on:change=move |e| set_diff("w", json!(event_target_checked(&e)))
                />
            </div>
        </section>
    }
}

#[component]
pub(super) fn FleetSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let busy = RwSignal::new(false);
    move || match data.get() {
        None => view! { <LoadingState text="正在加载设置…" /> }.into_any(),
        Some(Err(e)) => view! {
            <InlineError
                message=format!("无法加载设置：{e}")
                retry=Callback::new(move |_| data.refetch())
            />
        }
        .into_any(),
        Some(Ok(p)) => {
            let on = RwSignal::new(flag(&p["fleet"], "enabled"));
            let limit = RwSignal::new(p["bots"]["max_concurrent"].as_u64().unwrap_or(6));
            view! {
                <section class="card settings-card">
                    <h3>"多机调度"</h3>
                    <p class="muted">
                        "打开后，Blazar 启动的每个会话都会挂上调度工具，不管它跑在哪台机器上：列出机器、建工作区、给任意机器上的智能体发指令、等它跑完、读它的记录和改动、在工作区之间拷文件。远端机器经本机 hub 中转，自己不需要别的机器的密钥；每个会话一枚独立令牌，结束即作废。下一轮启动生效。"
                    </p>
                    <div class="settings-row">
                        <label for="settings-fleet">
                            "给 Blazar 启动的智能体挂上调度工具"
                        </label>
                        <input
                            id="settings-fleet"
                            type="checkbox"
                            prop:checked=move || on.get()
                            disabled=move || busy.get()
                            on:change=move |e| {
                                let v = event_target_checked(&e);
                                on.set(v);
                                busy.set(true);
                                leptos::task::spawn_local(async move {
                                    if !save(
                                            "PUT",
                                            "/api/settings",
                                            json!({"fleet": {"enabled": v}}),
                                        )
                                        .await
                                    {
                                        on.set(!v);
                                    }
                                    busy.set(false);
                                });
                            }
                        />
                    </div>
                    <div class="settings-row">
                        <label class="settings-label" for="settings-bot-limit">
                            <b>"同时运行的 bot 上限"</b>
                            <span>"超过时新的邮件先排队，等有 bot 停下再派。取 1 到 16。"</span>
                        </label>
                        <input
                            id="settings-bot-limit"
                            class="settings-input settings-number"
                            type="number"
                            min="1"
                            max=BOT_LIMIT_MAX
                            disabled=move || busy.get()
                            prop:value=move || limit.get().to_string()
                            on:change=move |e| {
                                let old = limit.get_untracked();
                                match event_target_value(&e).trim().parse::<u64>() {
                                    Ok(n) if (1..=BOT_LIMIT_MAX).contains(&n) => {
                                        if n == old {
                                            return;
                                        }
                                        limit.set(n);
                                        busy.set(true);
                                        leptos::task::spawn_local(async move {
                                            if !save(
                                                    "PUT",
                                                    "/api/settings",
                                                    json!({"bots": {"max_concurrent": n}}),
                                                )
                                                .await
                                            {
                                                limit.set(old);
                                            }
                                            busy.set(false);
                                        });
                                    }
                                    _ => {
                                        event_target::<web_sys::HtmlInputElement>(&e)
                                            .set_value(&old.to_string());
                                        toast(format!("上限要在 1 到 {BOT_LIMIT_MAX} 之间"));
                                    }
                                }
                            }
                        />
                    </div>
                </section>
            }.into_any()
        }
    }
}

const BOT_LIMIT_MAX: u64 = 16;
