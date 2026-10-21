use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::dialog::{Choice, ask};
use crate::components::status::{InlineError, LoadingState};
use crate::desktop;

use super::{flag, text};

#[component]
pub(super) fn HubSettings() -> impl IntoView {
    if !desktop::available() {
        return ().into_any();
    }
    let data = LocalResource::new(|| async { desktop::invoke("hub_mode", &json!({})).await });
    view! {
        <section class="card settings-card">
            <h3>"这台电脑连哪个 hub"</h3>
            {move || match data.get() {
                None => view! { <LoadingState text="正在读取 hub 设置…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=format!("读不了 hub 设置：{e}")
                            retry=Callback::new(move |_| data.refetch())
                        />
                    }
                        .into_any()
                }
                Some(Ok(v)) if text(&v, "mode") == "remote" => {
                    view! { <RemoteHubNotice state=v /> }.into_any()
                }
                Some(Ok(v)) => view! { <HubModeForm state=v /> }.into_any(),
            }}
        </section>
    }
    .into_any()
}

async fn switch_hub(args: Value, title: &str, body: String) -> Result<(), String> {
    if ask(
        title,
        &body,
        vec![Choice::plain("取消"), Choice::plain("切换并重启")],
    )
    .await
        != Some(1)
    {
        return Ok(());
    }
    desktop::invoke("set_hub_mode", &args).await.map(drop)
}

#[component]
fn RemoteHubNotice(state: Value) -> impl IntoView {
    let label = text(&state, "label");
    let busy = RwSignal::new(false);
    let status = RwSignal::new((String::new(), false));
    let connected = flag(&state, "connected");
    view! {
        <p>{format!("现在连的是 {label} 上的 hub。工作区、对话和账号都在那台机器上。")}</p>
        <div class="settings-row">
            <span>"状态"</span>
            <span>{if connected { "已连上" } else { "没连上" }}</span>
        </div>
        <div class="settings-actions">
            <button
                class="btn"
                disabled=move || busy.get()
                on:click=move |_| {
                    if busy.get_untracked() {
                        return;
                    }
                    busy.set(true);
                    leptos::task::spawn_local(async move {
                        let result = switch_hub(
                                json!({"mode": "embedded"}),
                                "切回本机 hub",
                                "Blazar 会重启，改用这台电脑上的 hub。远程 hub 照常运行。".into(),
                            )
                            .await;
                        if let Err(e) = result {
                            status.set((e, true));
                        }
                        busy.set(false);
                    });
                }
            >
                "切回本机 hub"
            </button>
        </div>
        <p class="settings-status" class:bad=move || status.get().1>
            {move || status.get().0}
        </p>
    }
}

#[component]
fn HubModeForm(state: Value) -> impl IntoView {
    let saved = &state["saved"];
    let remote = RwSignal::new(false);
    let ssh = RwSignal::new(text(saved, "ssh"));
    let port = RwSignal::new(
        saved["port"]
            .as_u64()
            .map_or_else(|| "7777".to_owned(), |p| p.to_string()),
    );
    let busy = RwSignal::new(false);
    let status = RwSignal::new(("正在用这台电脑上的 hub。".to_owned(), false));
    let target = move || -> Result<(String, u16), String> {
        let host = ssh.get_untracked().trim().to_owned();
        if host.is_empty() {
            return Err("先填 SSH 主机别名".into());
        }
        let port = port
            .get_untracked()
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| "端口要在 1 到 65535 之间".to_owned())?;
        Ok((host, port))
    };
    view! {
        <p class="muted">
            "本机：这台电脑自己运行 hub。远程机器：经 SSH 连到一台常驻机器上用 scripts/hub-service.sh 装好的 hub，bot 和对话在那台机器上继续跑。"
        </p>
        <div class="settings-row">
            <span>"连接到"</span>
            <div class="settings-checks" role="radiogroup">
                <label>
                    <input
                        type="radio"
                        name="hub-mode"
                        prop:checked=move || !remote.get()
                        disabled=move || busy.get()
                        on:change=move |_| remote.set(false)
                    />
                    "本机"
                </label>
                <label>
                    <input
                        type="radio"
                        name="hub-mode"
                        prop:checked=move || remote.get()
                        disabled=move || busy.get()
                        on:change=move |_| remote.set(true)
                    />
                    "远程机器"
                </label>
            </div>
        </div>
        <div class="settings-form">
            <label class="settings-field">
                "SSH 主机别名"
                <input
                    class="settings-input"
                    placeholder="hub-host"
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || ssh.get()
                    disabled=move || busy.get() || !remote.get()
                    on:input=move |e| ssh.set(event_target_value(&e))
                />
            </label>
            <label class="settings-field">
                "端口"
                <input
                    class="settings-input"
                    type="number"
                    min="1"
                    max="65535"
                    prop:value=move || port.get()
                    disabled=move || busy.get() || !remote.get()
                    on:input=move |e| port.set(event_target_value(&e))
                />
            </label>
            <div class="settings-actions">
                <button
                    class="btn"
                    disabled=move || busy.get() || !remote.get()
                    on:click=move |_| {
                        if busy.get_untracked() {
                            return;
                        }
                        let (host, port) = match target() {
                            Ok(t) => t,
                            Err(e) => {
                                status.set((e, true));
                                return;
                            }
                        };
                        busy.set(true);
                        status.set((format!("正在测试 {host}，最多 15 秒…"), false));
                        leptos::task::spawn_local(async move {
                            match desktop::invoke(
                                    "test_remote_hub",
                                    &json!({"ssh": host, "port": port}),
                                )
                                .await
                            {
                                Ok(v) => {
                                    status.set((v.as_str().unwrap_or("能连上").to_owned(), false))
                                }
                                Err(e) => status.set((e, true)),
                            }
                            busy.set(false);
                        });
                    }
                >
                    "测试连接"
                </button>
                <button
                    class="btn primary"
                    disabled=move || busy.get() || !remote.get()
                    on:click=move |_| {
                        if busy.get_untracked() {
                            return;
                        }
                        let (host, port) = match target() {
                            Ok(t) => t,
                            Err(e) => {
                                status.set((e, true));
                                return;
                            }
                        };
                        busy.set(true);
                        leptos::task::spawn_local(async move {
                            let result = switch_hub(
                                    json!({"mode": "remote", "ssh": host, "port": port}),
                                    "切换到远程 hub",
                                    format!(
                                        "Blazar 会重启并连到 {host} 上的 hub。这台电脑上的 hub 会停下，它的数据保留，随时可以切回来。",
                                    ),
                                )
                                .await;
                            if let Err(e) = result {
                                status.set((e, true));
                            }
                            busy.set(false);
                        });
                    }
                >
                    "切换并重启"
                </button>
            </div>
            <p class="settings-status" class:bad=move || status.get().1>
                {move || status.get().0}
            </p>
        </div>
    }
}
