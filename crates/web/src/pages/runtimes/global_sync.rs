use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api::{self, Accounts};
use crate::components::dialog::{self, Choice};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::realtime::use_bus;

#[component]
pub(super) fn GlobalSync(
    accounts: LocalResource<Result<Accounts, api::ApiError>>,
) -> impl IntoView {
    let bus = use_bus();
    let state = LocalResource::new(move || {
        bus.accounts.track();
        api::get::<Value>("/api/accounts/global")
    });
    let busy = RwSignal::new(false);
    let on = move || {
        state
            .get()
            .and_then(Result::ok)
            .is_some_and(|v| v["enabled"].as_bool() == Some(true))
    };
    let label_of = move |id: Option<&str>| -> String {
        match id {
            None => "原来的默认登录".into(),
            Some(id) => accounts
                .get()
                .and_then(Result::ok)
                .and_then(|a| a.accounts.into_iter().find(|x| x.id == id).map(|x| x.label))
                .unwrap_or_else(|| id.to_owned()),
        }
    };
    let toggle = move |_| {
        if busy.get_untracked() || !matches!(state.get_untracked(), Some(Ok(_))) {
            return;
        }
        busy.set(true);
        let enable = !on();
        spawn_local(async move {
            if enable {
                let body = "打开后，在 Blazar 里点「使用」切换账号时，这台电脑上终端里的 claude / codex 和 Cursor 的 Claude 插件也会换成同一个账号。\n\n\
                    · 会改：钥匙串里的 Claude Code 登录、~/.claude.json 里的账号信息、~/.codex/auth.json。项目设置、MCP 配置和登录不受影响。\n\
                    · 原来的登录会先备份；关掉这个开关就恢复原样。\n\
                    · 正在运行的 claude / codex 要重开一次才会用新账号。";
                if dialog::ask(
                    "同步到终端和 Cursor",
                    body,
                    vec![Choice::plain("取消"), Choice::plain("打开")],
                )
                .await
                    != Some(1)
                {
                    let _ = busy.try_set(false);
                    return;
                }
            }
            match api::send::<Value>("PUT", "/api/accounts/global", &json!({ "enabled": enable }))
                .await
            {
                Ok(_) => toast(if enable {
                    "已打开：终端和 Cursor 现在跟着 Blazar 用同一个账号"
                } else {
                    "已关掉：终端和 Cursor 换回原来的登录"
                }),
                Err(e) => toast(format!("没切成：{e}")),
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <details class="card pad global-sync runtime-details">
            <summary>"终端与 Cursor"<span class=move || if on() { "state ok" } else { "state muted" }>{move || match state.get() { None => "加载中", Some(Err(_)) => "待重试", Some(Ok(_)) if on() => "已同步", Some(Ok(_)) => "未同步" }}</span></summary>
            <div class="card-title">
                <h3>"同步账号选择"</h3>
                <span class="grow"></span>
                <button class="btn" disabled=move || busy.get() || !matches!(state.get(), Some(Ok(_))) on:click=toggle>
                    {move || if busy.get() { "切换中…" } else if on() { "停止同步" } else { "开启同步" }}
                </button>
            </div>
            {move || match state.get() {
                None => view! { <LoadingState text="正在加载同步设置…"/> }.into_any(),
                Some(Err(error)) => view! { <InlineError message=format!("无法加载同步设置：{error}") retry=Callback::new(move |_| state.refetch())/> }.into_any(),
                Some(Ok(_)) => ().into_any(),
            }}
            <div class="muted small">
                {move || if on() {
                    let v = state.get().and_then(Result::ok).unwrap_or(Value::Null);
                    format!("终端里的 claude 和 Cursor 插件现在用「{}」，codex 用「{}」。在下面点「使用」会一起换。",
                        label_of(v["current"]["claude"].as_str()), label_of(v["current"]["codex"].as_str()))
                } else {
                    "现在只换 Blazar 自己的对话。打开后，终端里的 claude / codex 和 Cursor 插件也跟着这里选的账号走。".to_owned()
                }}
            </div>
        </details>
    }
}
