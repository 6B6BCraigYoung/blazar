use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;
use serde_json::{Value, json};

use crate::api::{self, Account, Accounts, Runtime, Runtimes};
use crate::components::page_head::PageHead;
use crate::components::status::{EmptyState, InlineError, LoadingState, Tone};
use crate::components::term_dialog::{self, TermLogin};
use crate::components::toast::toast;
use crate::realtime::use_bus;

use super::agents;

mod account_form;
mod accounts;
mod detail;
mod global_sync;
mod overview;

use account_form::AccountForm;
use accounts::AccountRow;
use detail::RuntimeDetail;
use global_sync::GlobalSync;
use overview::RuntimeCard;

pub const WITH_ACCOUNTS: [(&str, &str); 2] = [("claude", "Claude Code"), ("codex", "Codex")];

pub fn account_status(a: &Account) -> (&'static str, Tone) {
    if a.disabled {
        ("已停用", Tone::Other)
    } else if a.status == "logged_out" && a.checked_at.is_some() {
        ("登录已失效", Tone::Attention)
    } else if a.status == "logged_out" {
        ("未登录", Tone::Attention)
    } else if a.status == "ok" && !a.stale {
        ("已登录", Tone::Other)
    } else {
        ("未检查", Tone::Other)
    }
}

pub fn account_status_hint(a: &Account) -> String {
    let checked = a
        .checked_at
        .as_deref()
        .map(|t| format!("上次检查：{}", crate::fmt::ago(t)));
    match (a.status.as_str(), a.stale, checked) {
        ("logged_out", _, Some(c)) => format!("凭据已经失效，重新登录后才会进入轮换。{c}"),
        ("ok", true, Some(c)) => format!("{c}，超过 1 小时的结果不再当真，用之前会重新检查"),
        ("ok", false, Some(c)) if a.kind == "token" => format!("{c}。token 失效后会提示重新配置"),
        (_, _, Some(c)) => c,
        _ => "还没有检查过登录状态".to_owned(),
    }
}

pub fn runtime_status(rt: &Runtime, active: Option<&Account>) -> (&'static str, Tone) {
    if !rt.installed {
        return ("未安装", Tone::Attention);
    }
    if WITH_ACCOUNTS.iter().any(|x| x.0 == rt.id) {
        return match active {
            Some(a) => account_status(a),
            None => ("未选择账号", Tone::Attention),
        };
    }
    match rt.authed {
        Some(true) => ("已登录", Tone::Other),
        Some(false) => ("未登录", Tone::Attention),
        None => ("待检查", Tone::Other),
    }
}

pub fn active_account(a: &Accounts, provider: &str) -> Option<Account> {
    let on = a.active(provider)?;
    a.accounts.iter().find(|x| x.id == on).cloned()
}

#[component]
pub fn RuntimesPage() -> impl IntoView {
    let bus = use_bus();
    let rescan = RwSignal::new(0u32);
    let runtimes = LocalResource::new(move || {
        rescan.track();
        api::get::<Runtimes>("/api/runtimes")
    });
    let accounts = LocalResource::new(move || {
        bus.accounts.track();
        rescan.track();
        api::get::<Accounts>("/api/accounts")
    });
    spawn_local(async move {
        let _ = api::send::<Value>("POST", "/api/accounts/check", &json!({})).await;
    });

    view! {
        <div class="page runtimes-page">
            <PageHead title="智能体" nav=|| crate::components::section::agents("/runtimes") />
            {move || match runtimes.get() {
                None => view! { <LoadingState text="正在查找运行时…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=format!("无法加载运行时：{e}")
                            retry=Callback::new(move |_| rescan.update(|n| *n += 1))
                        />
                    }
                        .into_any()
                }
                Some(Ok(r)) => {
                    let (inst, missing): (Vec<Runtime>, Vec<Runtime>) = r
                        .runtimes
                        .into_iter()
                        .partition(|x| x.installed);
                    let brains = inst.iter().any(|x| x.remote_hands && x.authed != Some(false));
                    let empty = inst.is_empty();
                    view! {
                        {empty
                            .then(|| {
                                view! {
                                    <EmptyState
                                        title="添加第一个运行时"
                                        detail="安装 Claude Code 或 Codex 后，重新扫描即可添加账号。"
                                    />
                                }
                            })}
                        {inst
                            .into_iter()
                            .map(|rt| view! { <RuntimeCard rt accounts /> })
                            .collect_view()}
                        {(!brains && !empty)
                            .then(|| {
                                view! {
                                    <div class="card notice">
                                        "添加或登录一个 Claude Code / Codex 账号，即可开始对话。"
                                    </div>
                                }
                            })}
                        {(!missing.is_empty())
                            .then(|| {
                                view! {
                                    <details class="runtime-details">
                                        <summary>"其他支持的运行时"</summary>
                                        <p class="muted small">
                                            {format!(
                                                "尚未安装：{}",
                                                missing
                                                    .iter()
                                                    .map(|x| x.label.as_str())
                                                    .collect::<Vec<_>>()
                                                    .join("、"),
                                            )}
                                        </p>
                                    </details>
                                }
                            })}
                        <details class="runtime-details">
                            <summary>"本机信息"</summary>
                            <p class="muted small">
                                {format!(
                                    "{} · Blazar v{} · {} {}",
                                    r.machine.hostname,
                                    r.machine.blazar_version,
                                    r.machine.os,
                                    r.machine.arch,
                                )}
                            </p>
                        </details>
                    }
                        .into_any()
                }
            }}
            <GlobalSync accounts />
        </div>
    }
}

#[derive(Clone, PartialEq)]
pub enum Form {
    Add,
    Token(Account),
    Plan(Account),
}

#[component]
pub fn AccountsSection(
    provider: String,
    accounts: LocalResource<Result<Accounts, api::ApiError>>,
    #[prop(optional)] form: Option<RwSignal<Option<Form>>>,
    #[prop(default = true)] head: bool,
) -> impl IntoView {
    let form = form.unwrap_or_else(|| RwSignal::new(None));
    let menu = RwSignal::new(None::<String>);
    let p2 = provider.clone();
    view! {
        {head
            .then(|| {
                view! {
                    <div class="acc-row head">
                        <span></span>
                        <span>"账号"</span>
                        <span>"状态"</span>
                        <span>"额度"</span>
                        <span>"最近使用"</span>
                        <span></span>
                    </div>
                }
            })}
        {move || {
            let a = match accounts.get() {
                None => return view! { <LoadingState text="正在加载账号…" /> }.into_any(),
                Some(Err(error)) => {
                    return view! {
                        <InlineError
                            message=format!("无法加载账号：{error}")
                            retry=Callback::new(move |_| accounts.refetch())
                        />
                    }
                        .into_any();
                }
                Some(Ok(a)) => a,
            };
            let on = a.active(&p2);
            let list = a.accounts.into_iter().filter(|x| x.provider == p2).collect::<Vec<_>>();
            if list.is_empty() {
                return view! {
                    <EmptyState
                        title="还没有账号"
                        detail="添加账号后，可在这里切换使用。"
                    />
                }
                    .into_any();
            }
            list.into_iter()
                .map(|x| {
                    let active = on.as_deref() == Some(x.id.as_str());
                    view! { <AccountRow a=x active menu form /> }
                })
                .collect_view()
                .into_any()
        }}
        {head
            .then(|| {
                view! {
                    <div class="pref-bar">
                        <button
                            type="button"
                            class="pref-bar-btn"
                            aria-label="添加账号"
                            title="添加账号"
                            aria-haspopup="dialog"
                            on:click=move |_| form.set(Some(Form::Add))
                        >
                            "+"
                        </button>
                    </div>
                }
            })}
        {move || {
            form.get()
                .map(|f| {
                    view! {
                        <AccountForm f provider=provider.clone() on_close=move || form.set(None) />
                    }
                })
        }}
    }
}

async fn patch(a: &Account, body: Value) -> Result<Value, api::ApiError> {
    api::send::<Value>("PUT", &format!("/api/accounts/{}", api::enc(&a.id)), &body).await
}

pub fn login(a: &Account) {
    let id = a.id.clone();
    let cmd = if a.provider == "claude" {
        "claude auth login"
    } else {
        "codex login"
    };
    term_dialog::open(TermLogin {
        title: format!("登录 {}", a.label),
        hint: format!(
            "正在运行 {cmd}{}。浏览器没自动打开的话，复制终端里的链接去登录。",
            if a.config_dir.is_some() {
                "（独立配置目录）"
            } else {
                "（默认账号）"
            }
        ),
        path: format!("/api/accounts/{}/login/ws", api::enc(&a.id)),
        verify_node: None,
        after: Callback::new(move |_| {
            let id = id.clone();
            spawn_local(async move {
                match api::send::<Value>(
                    "POST",
                    &format!("/api/accounts/{}/check", api::enc(&id)),
                    &json!({}),
                )
                .await
                {
                    Ok(r) => {
                        let label = r["label"].as_str().unwrap_or("");
                        toast(if r["status"] == "ok" {
                            format!(
                                "「{label}」已登录{}",
                                r["email"]
                                    .as_str()
                                    .map(|e| format!("：{e}"))
                                    .unwrap_or_default()
                            )
                        } else {
                            format!("「{label}」还没有登录成功")
                        });
                    }
                    Err(e) => toast(e.to_string()),
                }
            });
        }),
    });
}

pub fn node_login(node: String, after: Callback<bool>) {
    let shown = crate::fmt::machine(&node);
    term_dialog::open(TermLogin {
        title: format!("在 {shown} 上登录 Codex"),
        hint: format!(
            "在 {shown} 上运行 codex login --device-auth（没装 Codex 会先装进 ~/.blazar）。在本地浏览器打开下面的链接、输入验证码即可，凭据只保存在 {shown} 上。"
        ),
        path: format!("/api/nodes/{}/login/codex/ws", api::enc(&node)),
        verify_node: Some(node.clone()),
        after,
    });
}

#[component]
pub fn RuntimeDetailPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();
    move || {
        let id = id();
        view! { <RuntimeDetail id /> }
    }
}
