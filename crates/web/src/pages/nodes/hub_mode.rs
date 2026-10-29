use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::dialog::{Choice, ask};
use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow, Sheet, Switch};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::{flash, toast_error};
use crate::desktop;

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().into()
}

async fn save(body: Value) -> bool {
    match api::send::<Value>("PUT", "/api/settings", &body).await {
        Ok(_) => {
            flash("已保存");
            true
        }
        Err(e) => {
            toast_error("没保存上", &e);
            false
        }
    }
}

/// 机器表下面一小组：跨机器协作的开关，和桌面版的 Blazar 跑在哪台机器上。
#[component]
pub(super) fn MultiMachine() -> impl IntoView {
    view! {
        <PrefGroup title="多机协作">
            <CrossMachine />
            <RunsOn />
        </PrefGroup>
    }
}

/// 允许智能体跨机器建工作区、派活（原先在设置 → 对话与运行）。
#[component]
fn CrossMachine() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let busy = RwSignal::new(false);
    move || match data.get() {
        None => view! { <LoadingState text="正在加载…" /> }.into_any(),
        Some(Err(e)) => view! {
            <InlineError
                lead="无法加载设置"
                error=e
                retry=Callback::new(move |_| data.refetch())
            />
        }
        .into_any(),
        Some(Ok(p)) => {
            let on = RwSignal::new(p["fleet"]["enabled"].as_bool().unwrap_or(false));
            view! {
                <PrefRow
                    label="允许智能体跨机器建工作区、派活"
                    detail="对话能列出机器、在别的机器上建工作区、给那里的智能体派活；权限随对话发放，对话结束就收回。下一轮对话开始时生效。"
                    for_id="nodes-fleet"
                >
                    <Switch
                        id="nodes-fleet"
                        checked=on
                        disabled=busy
                        on_change=move |v: bool| {
                            on.set(v);
                            busy.set(true);
                            leptos::task::spawn_local(async move {
                                if !save(json!({"fleet": {"enabled": v}})).await {
                                    on.set(!v);
                                }
                                busy.set(false);
                            });
                        }
                    />
                </PrefRow>
            }
                .into_any()
        }
    }
}

/// 桌面版连这台电脑上的 Blazar 服务，还是另一台常开的机器（原先在设置 → 数据与关于）。
/// 平时只占一行，「更改…」才展开。
#[component]
fn RunsOn() -> impl IntoView {
    if !desktop::available() {
        return ().into_any();
    }
    let data = LocalResource::new(|| async { desktop::invoke("hub_mode", &json!({})).await });
    let open = RwSignal::new(false);
    let line = move || match data.get() {
        None => "正在读取…".to_owned(),
        Some(Err(e)) => crate::vocab::lead("读不到运行位置", crate::vocab::explain(&e, None)).0,
        Some(Ok(v)) if text(&v, "mode") == "remote" => {
            let state = if v["connected"].as_bool().unwrap_or(false) {
                "已连上"
            } else {
                "没连上"
            };
            format!("{}（{state}）", text(&v, "label"))
        }
        Some(Ok(_)) => "这台电脑".to_owned(),
    };
    view! {
        {move || {
            view! {
                <PrefRow label="Blazar 运行在" detail=line()>
                    <button
                        type="button"
                        class="btn"
                        aria-haspopup="dialog"
                        disabled=move || data.get().is_none()
                        on:click=move |_| open.set(true)
                    >
                        "更改…"
                    </button>
                </PrefRow>
            }
        }}
        <Show when=move || open.get()>
            <Sheet
                title="Blazar 在哪台机器上运行"
                on_close=Callback::new(move |()| open.set(false))
                foot=move || {
                    view! {
                        <button
                            type="button"
                            class="btn primary"
                            data-modal-initial-focus=""
                            on:click=move |_| open.set(false)
                        >
                            "完成"
                        </button>
                    }
                }
            >
                {move || match data.get() {
                    None => view! { <LoadingState text="正在读取…" /> }.into_any(),
                    Some(Err(e)) => {
                        view! {
                            <InlineError
                                lead="读不到运行位置"
                                message=e
                                retry=Callback::new(move |_| data.refetch())
                            />
                        }
                            .into_any()
                    }
                    Some(Ok(v)) if text(&v, "mode") == "remote" => {
                        view! { <RemoteNotice state=v /> }.into_any()
                    }
                    Some(Ok(v)) => view! { <ModeForm state=v /> }.into_any(),
                }}
            </Sheet>
        </Show>
    }
    .into_any()
}

async fn switch_to(args: Value, title: &str, body: String) -> Result<(), String> {
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
fn RemoteNotice(state: Value) -> impl IntoView {
    let label = text(&state, "label");
    let busy = RwSignal::new(false);
    let status = RwSignal::new((String::new(), false));
    let connected = state["connected"].as_bool().unwrap_or(false);
    view! {
        <PrefRow
            label="运行在"
            detail=format!(
                "{label}。工作区、对话和账号都在那台机器上，合上这台电脑也照常跑。",
            )
        >
            <span>{if connected { "已连上" } else { "没连上" }}</span>
        </PrefRow>
        <PrefRow
            label="切回这台电脑"
            detail="Blazar 会重启，改用这台电脑。那台机器上的 Blazar 照常运行。"
        >
            <button
                type="button"
                class="btn"
                disabled=move || busy.get()
                on:click=move |_| {
                    if busy.get_untracked() {
                        return;
                    }
                    busy.set(true);
                    leptos::task::spawn_local(async move {
                        let result = switch_to(
                                json!({"mode": "embedded"}),
                                "切回这台电脑",
                                "Blazar 会重启，改用这台电脑。那台机器上的 Blazar 照常运行。"
                                    .into(),
                            )
                            .await;
                        if let Err(e) = result {
                            status.set((crate::vocab::error(&e, None), true));
                        }
                        busy.set(false);
                    });
                }
            >
                "切回这台电脑…"
            </button>
        </PrefRow>
        <Show when=move || !status.get().0.is_empty()>
            <p class="pref-status" class:bad=move || status.get().1>
                {move || status.get().0}
            </p>
        </Show>
    }
}

#[component]
fn ModeForm(state: Value) -> impl IntoView {
    let saved = &state["saved"];
    let remote = RwSignal::new(false);
    let ssh = RwSignal::new(text(saved, "ssh"));
    let port = RwSignal::new(
        saved["port"]
            .as_u64()
            .map_or_else(|| "7777".to_owned(), |p| p.to_string()),
    );
    let busy = RwSignal::new(false);
    let status = RwSignal::new(("现在运行在这台电脑上。".to_owned(), false));
    let target = move || -> Result<(String, u16), String> {
        let host = ssh.get_untracked().trim().to_owned();
        if host.is_empty() {
            return Err("先填那台机器的名字".into());
        }
        let port = port
            .get_untracked()
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| "「高级…」里的端口要在 1 到 65535 之间".to_owned())?;
        Ok((host, port))
    };
    view! {
        <PrefRow
            label="运行在"
            detail="放到一台常开的机器上，合上这台电脑后 bot 和对话继续跑。"
        >
            <div class="seg" role="group" aria-label="运行在">
                <button
                    type="button"
                    data-on=move || (!remote.get()).to_string()
                    aria-pressed=move || (!remote.get()).to_string()
                    disabled=move || busy.get()
                    on:click=move |_| remote.set(false)
                >
                    "这台电脑"
                </button>
                <button
                    type="button"
                    data-on=move || remote.get().to_string()
                    aria-pressed=move || remote.get().to_string()
                    disabled=move || busy.get()
                    on:click=move |_| remote.set(true)
                >
                    "另一台机器"
                </button>
            </div>
        </PrefRow>
        <Show when=move || remote.get()>
            <PrefRow
                label="机器"
                detail="填 ~/.ssh/config 里的名字，要能免密登录。"
                for_id="hub-ssh"
            >
                <input
                    id="hub-ssh"
                    class="settings-input"
                    placeholder="如 office-mac"
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || ssh.get()
                    disabled=move || busy.get()
                    on:input=move |e| ssh.set(event_target_value(&e))
                />
            </PrefRow>
            <PrefRow label="切换" detail="切换后 Blazar 会重启并连到那台机器。">
                <button
                    type="button"
                    class="btn"
                    disabled=move || busy.get()
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
                                    status
                                        .set((v.as_str().unwrap_or("能连上").to_owned(), false))
                                }
                                Err(e) => status.set((crate::vocab::error(&e, None), true)),
                            }
                            busy.set(false);
                        });
                    }
                >
                    "测试连接"
                </button>
                <button
                    type="button"
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
                            let result = switch_to(
                                    json!({"mode": "remote", "ssh": host, "port": port}),
                                    "改到另一台机器上运行",
                                    format!(
                                        "Blazar 会重启并连到 {host}。这台电脑上的数据保留，随时可以切回来。",
                                    ),
                                )
                                .await;
                            if let Err(e) = result {
                                status.set((crate::vocab::error(&e, None), true));
                            }
                            busy.set(false);
                        });
                    }
                >
                    "切换并重启…"
                </button>
            </PrefRow>
        </Show>
        <Show when=move || !status.get().0.is_empty()>
            <p class="pref-status" class:bad=move || status.get().1>
                {move || status.get().0}
            </p>
        </Show>
        <PrefAdvanced title="高级">
            <p>
                "那台机器上要先装好常驻的 Blazar 服务（仓库里的 scripts/hub-service.sh）。这台电脑经 SSH 连过去，所以要能免密登录。"
            </p>
            <PrefRow
                label="端口"
                detail="那台机器上 Blazar 服务用的端口，默认 7777，一般不用改。"
                for_id="hub-port"
            >
                <input
                    id="hub-port"
                    class="settings-input"
                    type="number"
                    min="1"
                    max="65535"
                    prop:value=move || port.get()
                    disabled=move || busy.get()
                    on:input=move |e| port.set(event_target_value(&e))
                />
            </PrefRow>
        </PrefAdvanced>
    }
}
