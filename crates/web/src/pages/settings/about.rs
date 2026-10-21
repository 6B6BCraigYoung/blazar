use leptos::prelude::*;
use serde_json::Value;

use crate::api;
use crate::components::status::{InlineError, LoadingState};
use crate::components::{
    dialog::{Choice, ask},
    toast::toast,
};

use super::{apply_ui_preferences, text};

#[component]
pub(super) fn AboutSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/about"));
    let resetting = RwSignal::new(false);
    view! {
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
            Some(Ok(a)) => {
                view! {
                    <section class="card settings-card">
                        <h3>"Blazar"</h3>
                        <div class="settings-row">
                            <span>"版本"</span>
                            <code>{text(&a, "version")}</code>
                        </div>
                        <details class="settings-details">
                            <summary>"连接信息"</summary>
                            <div class="settings-row">
                                <span>"本地服务地址"</span>
                                <code>
                                    {a["hub_url"]
                                        .as_str()
                                        .filter(|s| !s.is_empty())
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| {
                                            window().location().origin().unwrap_or_default()
                                        })}
                                </code>
                            </div>
                        </details>
                        <p class="muted">
                            "文档在仓库 README.md。Blazar 代码使用 MIT 许可证。"
                        </p>
                    </section>
                    <section class="card settings-card">
                        <h3>"第三方许可证"</h3>
                        <p>
                            {format!(
                                "组网引擎：{} {} · {}",
                                text(&a["engine"], "name"),
                                text(&a["engine"], "version"),
                                text(&a["engine"], "license"),
                            )}
                        </p>
                        <a
                            href=text(&a["engine"], "source")
                            target="_blank"
                            rel="noopener noreferrer"
                        >
                            "对应版本源码 ↗"
                        </a>
                        <p class="muted">
                            "Monaco Editor、xterm.js：MIT；IBM Plex Sans、JetBrains Mono：SIL OFL 1.1。完整声明见 THIRD-PARTY-NOTICES.md。"
                        </p>
                    </section>
                    <details class="card settings-card settings-details">
                        <summary>"本地数据详情"</summary>
                        <p>
                            {format!(
                                "本机 SQLite：{:.1} MB",
                                a["db_bytes"].as_f64().unwrap_or_default() / 1048576.0,
                            )}
                        </p>
                        {a["counts"]
                            .as_object()
                            .map(|counts| {
                                counts
                                    .iter()
                                    .map(|(k, v)| {
                                        view! {
                                            <div class="settings-row">
                                                <span>{k.clone()}</span>
                                                <span>{v.to_string()}</span>
                                            </div>
                                        }
                                    })
                                    .collect_view()
                            })}
                    </details>
                }
                    .into_any()
            }
        }}
        <section class="card settings-card">
            <h3>"这台设备上的偏好"</h3>
            <p class="muted">
                "恢复主题、布局、快捷键、提醒和视图默认值。工作区、对话和任务数据保留。"
            </p>
            <button
                class="btn danger"
                disabled=move || resetting.get()
                on:click=move |_| {
                    if resetting.get_untracked() {
                        return;
                    }
                    resetting.set(true);
                    leptos::task::spawn_local(async move {
                        if ask(
                                "恢复默认偏好",
                                "恢复这台设备上的界面偏好？",
                                vec![Choice::plain("取消"), Choice::danger("恢复默认")],
                            )
                            .await != Some(1)
                        {
                            resetting.set(false);
                            return;
                        }
                        if let Ok(Some(store)) = window().local_storage() {
                            let keys = (0..store.length().unwrap_or(0))
                                .filter_map(|i| store.key(i).ok().flatten())
                                .filter(|k| {
                                    [
                                        "blazar.ui",
                                        "blazar.shortcuts",
                                        "blazar.sound",
                                        "blazar.notify",
                                        "blazar.md.mode",
                                        "blazar.diffprefs",
                                        "blazar.preview",
                                        "blazar.an.days",
                                        "blazar.v2.ws.panel",
                                        "blazar.v2.ws.layout",
                                        "blazar.todosOpen",
                                        "blazar.editor",
                                    ]
                                        .contains(&k.as_str())
                                })
                                .collect::<Vec<_>>();
                            for k in keys {
                                let _ = store.remove_item(&k);
                            }
                        }
                        apply_ui_preferences();
                        toast("已恢复默认，刷新页面后布局和快捷键全部生效");
                        resetting.set(false);
                    });
                }
            >
                "恢复默认"
            </button>
        </section>
    }
}
