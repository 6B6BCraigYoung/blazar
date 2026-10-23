use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::{Value, json};

use super::settings::{NOTIFY_KINDS, flag, rows, save, text};
use crate::components::page_head::PageHead;
use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow, Switch};
use crate::components::{
    dialog::{Choice, ask},
    status::{Checking, EmptyState, InlineError, LoadingState, Status, Tone},
    toast::toast,
};
use crate::{api, app_state::use_app};

mod job_poll;
mod sims;
use job_poll::JobPoll;
pub use sims::SIMS;

const APPS: &[(&str, &str, &str)] = &[
    (
        "lark",
        "飞书 / Lark",
        "让智能体处理飞书事务，接收工作提醒。",
    ),
    ("github", "GitHub", "创建 PR、查看检查结果，发现技能。"),
    (
        "obsidian",
        "Obsidian",
        "把笔记库作为工作区，读写 Markdown 笔记。",
    ),
];

fn ready(id: &str, v: &Value) -> bool {
    flag(v, "installed")
        && match id {
            "github" => flag(v, "authed"),
            "lark" => flag(&v["auth"]["user"], "available") || flag(&v["auth"]["bot"], "available"),
            _ => rows(&v["vaults"]).iter().any(|v| flag(v, "exists")),
        }
}

fn state(id: &str, v: &Value) -> String {
    if !flag(v, "installed") {
        return "未安装".into();
    }
    let ready = ready(id, v);
    match id {
        "github" if ready => "已连接",
        "github" => "未登录",
        "lark" if ready => "已连接",
        "lark" if flag(v, "configured") => "未登录",
        "lark" => "待配置",
        _ if ready => "已就绪",
        _ => "待打开",
    }
    .into()
}

fn state_tone(id: &str, value: &Value) -> &'static str {
    if ready(id, value) { "ok" } else { "neutral" }
}

fn next_step(id: &str, v: &Value) -> &'static str {
    if !flag(v, "installed") {
        return "安装";
    }
    match id {
        "github" if !flag(v, "authed") => "登录",
        "lark" if !flag(v, "configured") => "创建应用",
        "lark" if !ready(id, v) => "登录",
        "obsidian"
            if rows(&v["vaults"])
                .iter()
                .any(|x| flag(x, "exists") && text(x, "workspace_id").is_empty()) =>
        {
            "创建工作区"
        }
        _ => "设置",
    }
}

#[component]
fn AppStatus(#[prop(into)] label: String, tone: &'static str) -> impl IntoView {
    let tone = match tone {
        "busy" => Tone::Run,
        "error" => Tone::Failed,
        _ => Tone::Other,
    };
    view! { <Status tone label /> }
}

#[component]
fn AppLogo(id: &'static str) -> impl IntoView {
    match id {
        "lark" => {
            view! { <img class="apps-logo" src="/vendor/brands/lark.png" alt="" /> }.into_any()
        }
        "obsidian" => {
            view! { <img class="apps-logo" src="/vendor/brands/obsidian.png" alt="" /> }.into_any()
        }
        "github" => {
            view! { <span class="apps-logo apps-github-logo" aria-hidden="true"></span> }.into_any()
        }
        "isaacsim" => {
            view! { <img class="apps-logo" src="/vendor/brands/isaacsim.png" alt="" /> }.into_any()
        }
        "mujoco" => {
            view! { <img class="apps-logo" src="/vendor/brands/mujoco.svg" alt="" /> }.into_any()
        }
        _ => ().into_any(),
    }
}

#[component]
pub fn AppsPage() -> impl IntoView {
    view! {
        <div class="page apps-page">
            <PageHead title="接入的软件" lead="连接常用软件，让智能体协助处理工作。" />
            <section class="apps-section">
                <h2>"办公"</h2>
                <div class="apps-grid">
                    {APPS
                        .iter()
                        .map(|&(id, name, desc)| view! { <AppCard id name desc /> })
                        .collect_view()}
                </div>
            </section>
            <section class="apps-section">
                <h2>"仿真"</h2>
                <div class="apps-grid">
                    {SIMS
                        .iter()
                        .map(|&(id, name, desc)| view! { <sims::SimCard id name desc /> })
                        .collect_view()}
                </div>
            </section>
        </div>
    }
}

#[component]
fn AppCard(id: &'static str, name: &'static str, desc: &'static str) -> impl IntoView {
    let rev = RwSignal::new(0u32);
    let data = LocalResource::new(move || {
        rev.get();
        async move { api::get::<Value>(&format!("/api/office/{id}")).await }
    });
    let step = move || {
        data.get()
            .and_then(Result::ok)
            .map_or("打开", |v| next_step(id, &v))
    };
    view! {
        <article class="card apps-card">
            <div class="apps-card-head">
                <AppLogo id />
                <h2>{name}</h2>
            </div>
            <p class="muted">{desc}</p>
            <div class="apps-card-status" aria-live="polite">
                {move || match data.get() {
                    None => view! { <AppStatus label="读取中…" tone="neutral" /> }.into_any(),
                    Some(Err(error)) => {
                        view! {
                            <span title=error.to_string()>
                                <AppStatus label="无法读取" tone="error" />
                            </span>
                            <button
                                type="button"
                                class="linkbtn"
                                on:click=move |_| rev.update(|n| *n += 1)
                            >
                                "重试"
                            </button>
                        }
                            .into_any()
                    }
                    Some(Ok(value)) => {
                        view! { <AppStatus label=state(id, &value) tone=state_tone(id, &value) /> }
                            .into_any()
                    }
                }}
            </div>
            <a
                class="btn apps-card-link"
                href=format!("/apps/{id}")
                aria-label=move || format!("{name}：{}", step())
            >
                {step}
                <span aria-hidden="true">"→"</span>
            </a>
        </article>
    }
}

#[component]
pub fn AppDetailPage() -> impl IntoView {
    let params = use_params_map();
    view! {
        <div class="page apps-page detail-page">
            <For
                each=move || vec![params.with(|p| p.get("id").unwrap_or_default())]
                key=|id| id.clone()
                children=move |id| {
                    if let Some(&(app, name, _)) = APPS.iter().find(|&&(app, _, _)| app == id) {
                        view! { <AppDetail id=app name /> }.into_any()
                    } else if let Some(&(app, name, _)) = SIMS
                        .iter()
                        .find(|&&(app, _, _)| app == id)
                    {
                        view! { <sims::SimApp id=app name /> }.into_any()
                    } else {
                        view! {
                            <PageHead title="接入的软件" />
                            <EmptyState title="找不到这个软件" />
                            <a class="btn" href="/apps">
                                "返回接入的软件"
                            </a>
                        }
                            .into_any()
                    }
                }
            />
        </div>
    }
}

#[component]
fn AppDetail(id: &'static str, name: &'static str) -> impl IntoView {
    let rev = RwSignal::new(0u32);
    let checking = RwSignal::new(false);
    crate::refocus::on_refocus(move || rev.update(|n| *n += 1));
    let data = LocalResource::new(move || {
        rev.get();
        checking.set(true);
        async move {
            let r = api::get::<Value>(&format!("/api/office/{id}")).await;
            checking.try_set(false);
            r
        }
    });
    let last = RwSignal::new(None::<Value>);
    Effect::new(move |_| {
        if let Some(Ok(v)) = data.get() {
            last.set(Some(v));
        }
    });
    let loading = Signal::derive(move || data.get().is_none());
    let checking = Signal::derive(move || checking.get());
    view! {
        {move || match (last.get(), data.get()) {
            (Some(v), _) => {
                if id == "obsidian" {
                    view! { <Obsidian initial=v rev checking /> }.into_any()
                } else {
                    view! { <ConnectApp id name initial=v rev loading checking /> }.into_any()
                }
            }
            (None, Some(Err(e))) => {
                view! {
                    <PageHead parent=("接入的软件", "/apps") icon=move || view! { <AppLogo id /> } title=name />
                    <div class="detail-body">
                        <InlineError
                            message=e.to_string()
                            retry=Callback::new(move |_| rev.update(|n| *n += 1))
                        />
                    </div>
                }
                    .into_any()
            }
            _ => {
                view! {
                    <PageHead parent=("接入的软件", "/apps") icon=move || view! { <AppLogo id /> } title=name />
                    <div class="detail-body">
                        <LoadingState text="读取连接…" />
                    </div>
                }
                    .into_any()
            }
        }}
    }
}

const DOMAINS: &[(&str, &str)] = &[
    ("im", "消息"),
    ("docs", "文档"),
    ("drive", "云盘"),
    ("base", "多维表格"),
    ("sheets", "表格"),
    ("slides", "幻灯片"),
    ("calendar", "日历"),
    ("mail", "邮箱"),
    ("task", "任务"),
    ("vc", "会议"),
    ("minutes", "妙记"),
    ("wiki", "知识库"),
    ("contact", "通讯录"),
    ("approval", "审批"),
    ("okr", "OKR"),
    ("attendance", "考勤"),
];

#[component]
fn ConnectApp(
    id: &'static str,
    name: &'static str,
    initial: Value,
    rev: RwSignal<u32>,
    loading: Signal<bool>,
    checking: Signal<bool>,
) -> impl IntoView {
    let tick = RwSignal::new(0u32);
    let pending = RwSignal::new(false);
    let fetching = RwSignal::new(false);
    let poll = RwSignal::new(JobPoll::default());
    let last_job = RwSignal::new(None::<Value>);
    let job = LocalResource::new(move || {
        tick.get();
        fetching.set(true);
        async move {
            let result = api::get::<Value>(&format!("/api/office/{id}/job")).await;
            let _ = fetching.try_set(false);
            result
        }
    });
    let interval = StoredValue::new_local(gloo_timers::callback::Interval::new(1100, move || {
        if pending.try_get_untracked() == Some(false)
            && fetching.try_get_untracked() == Some(false)
            && poll.try_with_untracked(|p| p.should_poll()) == Some(true)
        {
            tick.try_update(|n| *n += 1);
        }
    }));
    on_cleanup(move || interval.dispose());
    Effect::new(move |_| match job.get() {
        Some(Ok(value)) => {
            let mut finished = false;
            poll.update(|poll| finished = poll.observe(Ok(text(&value, "status") == "running")));
            last_job.set((!value.is_null()).then_some(value));
            if finished {
                rev.update(|n| *n += 1);
            }
        }
        Some(Err(_)) => poll.update(|poll| {
            poll.observe(Err(()));
        }),
        None => {}
    });
    let busy = move || pending.get() || fetching.get() || poll.get().busy() || loading.get();
    let start = Callback::new(move |body: Value| {
        if busy() {
            return;
        }
        pending.set(true);
        leptos::task::spawn_local(async move {
            let confirm = if body["step"] == "logout" {
                ask(
                    "退出登录",
                    "退出本机应用登录后，相关功能需要重新登录才能继续使用。",
                    vec![Choice::plain("取消"), Choice::danger("退出登录")],
                )
                .await
                    == Some(1)
            } else if flag(&body, "force") {
                ask(
                    "重新创建飞书应用",
                    "这会替换本机应用配置，需要重新登录；原应用仍保留在开放平台。",
                    vec![Choice::plain("取消"), Choice::danger("重新创建")],
                )
                .await
                    == Some(1)
            } else {
                true
            };
            if confirm {
                match api::send::<Value>("POST", &format!("/api/office/{id}/connect"), &body).await
                {
                    Ok(value) => {
                        poll.update(JobPoll::started);
                        last_job.set(Some(value));
                        job.set(None);
                        tick.update(|n| *n += 1);
                    }
                    Err(e) => {
                        toast(e.to_string());
                        poll.update(JobPoll::start_failed);
                        job.set(None);
                        tick.update(|n| *n += 1);
                    }
                }
            }
            pending.set(false);
        });
    });
    let brand = RwSignal::new(
        if initial["auth"]["brand"] == "lark" {
            "lark"
        } else {
            "feishu"
        }
        .to_owned(),
    );
    let scope = RwSignal::new("recommend".to_owned());
    let domains = RwSignal::new(Vec::<String>::new());
    let installed = flag(&initial, "installed");
    let can_install = flag(&initial, "can_install");
    let authed = if id == "lark" {
        flag(&initial["auth"]["user"], "available") || flag(&initial["auth"]["bot"], "available")
    } else {
        flag(&initial, "authed")
    };
    let configured = flag(&initial, "configured") || authed;
    let login = move || {
        if id == "lark" && scope.get_untracked() == "domains" && domains.get_untracked().is_empty()
        {
            toast("至少选择一项权限");
            return;
        }
        start.run(
            json!({"step":"login","scope":scope.get_untracked(),"domains":domains.get_untracked()}),
        );
    };
    let status_label = state(id, &initial);
    let status_tone = state_tone(id, &initial);
    let version = text(&initial, "version");
    let tool_text = if installed {
        format!("已安装 {version}")
    } else {
        "未安装".to_owned()
    };
    let tool = if id == "lark" { "lark-cli" } else { "gh" };
    let install_url = if id == "lark" {
        "https://nodejs.org"
    } else {
        "https://cli.github.com"
    };
    let brand_label = if initial["auth"]["brand"] == "lark" {
        "Lark"
    } else {
        "飞书"
    };
    let identities = if id == "lark" {
        [("user", "用户"), ("bot", "机器人")]
            .iter()
            .filter(|(k, _)| flag(&initial["auth"][*k], "available"))
            .map(|(_, l)| *l)
            .collect::<Vec<_>>()
            .join("、")
    } else {
        String::new()
    };
    let account_text = if !installed {
        "先安装连接工具".to_owned()
    } else if authed && identities.is_empty() {
        "已登录".to_owned()
    } else if authed {
        format!("已登录（{identities}）")
    } else if id == "lark" && !configured {
        "先创建应用".to_owned()
    } else {
        "未登录".to_owned()
    };
    let account_detail = if authed || !installed {
        ""
    } else if id == "lark" && !configured {
        "创建应用后再登录。"
    } else {
        "点上方「登录」，在浏览器完成授权。"
    };
    let settings_initial = initial.clone();
    let commands = (
        text(&initial, "install"),
        initial["init"].as_str().map(str::to_owned),
        initial["login"]
            .as_str()
            .unwrap_or(if id == "lark" {
                "lark-cli auth login --recommend"
            } else {
                "gh auth login"
            })
            .to_owned(),
    );
    let primary = if !installed && can_install {
        view! {
            <button
                type="button"
                class="btn primary"
                disabled=busy
                on:click=move |_| start.run(json!({"step":"install"}))
            >
                "安装"
            </button>
        }
        .into_any()
    } else if !installed {
        view! {
            <a class="btn primary" href=install_url target="_blank" rel="noopener noreferrer">
                "查看安装说明 ↗"
            </a>
        }
        .into_any()
    } else if id == "lark" && !configured {
        view! {
            <button
                type="button"
                class="btn primary"
                disabled=busy
                on:click=move |_| {
                    start.run(json!({"step":"init","brand":brand.get_untracked(),"force":false}))
                }
            >
                "创建应用"
            </button>
        }
        .into_any()
    } else if !authed {
        view! {
            <button type="button" class="btn primary" disabled=busy on:click=move |_| login()>
                "登录"
            </button>
        }
        .into_any()
    } else {
        ().into_any()
    };
    view! {
        <PageHead
            parent=("接入的软件", "/apps")
            icon=move || view! { <AppLogo id /> }
            title=name
            status=move || {
                view! {
                    <AppStatus label=status_label.clone() tone=status_tone />
                    <Checking busy=checking />
                }
            }
        >
            {primary}
        </PageHead>
        <div class="detail-body">
            <PrefGroup title="连接">
                <PrefRow label="连接工具" detail=tool>
                    <span class="pref-text">{tool_text}</span>
                </PrefRow>
                {(id == "lark" && installed)
                    .then(|| {
                        view! {
                            <PrefRow
                                label="应用"
                                detail=if configured {
                                    "重新创建后需要再次登录。"
                                } else {
                                    "选择飞书或 Lark，然后点上方「创建应用」。"
                                }
                            >
                                {if configured {
                                    view! {
                                        <span class="pref-text">{brand_label}</span>
                                        <button
                                            type="button"
                                            class="btn"
                                            disabled=busy
                                            on:click=move |_| {
                                                start
                                                    .run(
                                                        json!(
                                                            {"step":"init","brand":brand.get_untracked(),"force":true}
                                                        ),
                                                    )
                                            }
                                        >
                                            "重新创建…"
                                        </button>
                                    }
                                        .into_any()
                                } else {
                                    view! {
                                        <select
                                            class="settings-input"
                                            aria-label="飞书品牌"
                                            prop:value=move || brand.get()
                                            disabled=busy
                                            on:change=move |e| brand.set(event_target_value(&e))
                                        >
                                            <option
                                                value="feishu"
                                                selected=move || brand.get() == "feishu"
                                            >
                                                "飞书"
                                            </option>
                                            <option value="lark" selected=move || brand.get() == "lark">
                                                "Lark"
                                            </option>
                                        </select>
                                    }
                                        .into_any()
                                }}
                            </PrefRow>
                        }
                    })}
                <PrefRow label="账号" detail=account_detail>
                    {(id == "lark" && installed && configured)
                        .then(|| {
                            view! {
                                <select
                                    class="settings-input"
                                    aria-label="授权范围"
                                    prop:value=move || scope.get()
                                    disabled=busy
                                    on:change=move |e| scope.set(event_target_value(&e))
                                >
                                    <option
                                        value="recommend"
                                        selected=move || scope.get() == "recommend"
                                    >
                                        "推荐权限"
                                    </option>
                                    <option value="all" selected=move || scope.get() == "all">
                                        "全部权限"
                                    </option>
                                    <option
                                        value="domains"
                                        selected=move || scope.get() == "domains"
                                    >
                                        "自选权限"
                                    </option>
                                </select>
                            }
                        })}
                    <span class="pref-text">{account_text}</span>
                    {authed
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="btn"
                                    disabled=busy
                                    on:click=move |_| login()
                                >
                                    "重新授权"
                                </button>
                                <button
                                    type="button"
                                    class="btn danger"
                                    disabled=busy
                                    on:click=move |_| start.run(json!({"step":"logout"}))
                                >
                                    "退出登录"
                                </button>
                            }
                        })}
                </PrefRow>
                <Show when=move || id == "lark" && installed && configured && scope.get() == "domains">
                    <PrefRow label="自选权限">
                        <div class="settings-checks">
                            {DOMAINS
                                .iter()
                                .map(move |&(domain, label)| {
                                    view! {
                                        <label>
                                            <input
                                                type="checkbox"
                                                disabled=busy
                                                prop:checked=move || {
                                                    domains.get().iter().any(|d| d == domain)
                                                }
                                                on:change=move |e| {
                                                    let checked = event_target_checked(&e);
                                                    domains
                                                        .update(|d| {
                                                            d.retain(|s| s != domain);
                                                            if checked {
                                                                d.push(domain.into());
                                                            }
                                                        });
                                                }
                                            />
                                            {label}
                                        </label>
                                    }
                                })
                                .collect_view()}
                        </div>
                    </PrefRow>
                </Show>
                <PrefAdvanced title="连接详情">
                    {
                        let version = version.clone();
                        view! {
                            <PrefRow label="版本">
                                <code>{if version.is_empty() { "—".to_owned() } else { version }}</code>
                            </PrefRow>
                        }
                    }
                    {
                        let install = commands.0.clone();
                        view! {
                            <PrefRow label="安装">
                                <code>{install}</code>
                            </PrefRow>
                        }
                    }
                    {commands
                        .1
                        .clone()
                        .map(|init| {
                            view! {
                                <PrefRow label="配置">
                                    <code>{init}</code>
                                </PrefRow>
                            }
                        })}
                    {
                        let login = commands.2.clone();
                        view! {
                            <PrefRow label="登录">
                                <code>{login}</code>
                            </PrefRow>
                        }
                    }
                    <p>"安装与登录在本机执行，令牌由软件自己的命令行管理。"</p>
                    {(id == "lark")
                        .then(|| {
                            view! { <p>"绑定已有应用需要 App ID 和 Secret，请在终端配置。"</p> }
                        })}
                </PrefAdvanced>
            </PrefGroup>
            {move || {
                job.get()
                    .and_then(Result::err)
                    .map(|error| {
                        view! {
                            <InlineError
                                message="连接状态读取失败，正在重试。"
                                detail=error.to_string()
                                retry=Callback::new(move |_| tick.update(|n| *n += 1))
                            />
                        }
                    })
            }}
            {move || last_job.get().map(|data| view! { <ConnectJob id data tick /> })}
            {move || {
                (last_job.get().is_none() && fetching.get())
                    .then(|| {
                        view! {
                            <LoadingState text="读取连接任务…" class="apps-inline-state" />
                        }
                    })
            }}
            {(id == "lark" && installed)
                .then(|| view! { <LarkSettings initial=settings_initial.clone() /> })}
            {(id == "github")
                .then(|| {
                    view! {
                        <PrefGroup
                            title="使用 GitHub"
                            note="在工作区的 Git 面板创建 PR、查看状态和检查结果。远端工作区使用所在机器上的 gh 登录态，这里管理本机连接。"
                        >
                            <PrefRow label="PR 默认设置" detail="草稿 PR、分支前缀和 AI 起草。">
                                <a class="btn" href="/settings?s=git">
                                    "打开设置"
                                </a>
                            </PrefRow>
                            <PrefRow label="技能" detail="从 GitHub 发现并导入技能。">
                                <a class="btn" href="/skills?tab=market">
                                    "发现技能"
                                </a>
                            </PrefRow>
                        </PrefGroup>
                    }
                })}
        </div>
    }
}

#[component]
fn ConnectJob(id: &'static str, data: Value, tick: RwSignal<u32>) -> impl IntoView {
    let cancel = RwSignal::new(false);
    let running = text(&data, "status") == "running";
    let (status, tone) = match text(&data, "status").as_str() {
        "running" => ("运行中", "busy"),
        "done" => ("已完成", "ok"),
        "failed" => ("失败", "error"),
        "cancelled" => ("已取消", "neutral"),
        _ => ("待确认", "neutral"),
    };
    let step = match text(&data, "step").as_str() {
        "install" => "安装连接工具",
        "init" => "创建应用",
        "login" => "登录授权",
        "logout" => "退出登录",
        _ => "连接任务",
    };
    let url = data["url"]
        .as_str()
        .filter(|s| s.starts_with("https://") || s.starts_with("http://"))
        .map(str::to_owned);
    let message = data["message"]
        .as_str()
        .filter(|message| !message.trim().is_empty())
        .map(str::to_owned);
    let lines = rows(&data["lines"])
        .into_iter()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect::<Vec<_>>()
        .join("\n");
    let summary = format!("{} · {}", text(&data, "step"), text(&data, "status"));
    let code = data["code"].as_str().map(str::to_owned);
    let qr = data["qr"]
        .as_str()
        .filter(|s| s.starts_with("data:image/"))
        .map(str::to_owned);
    let untrusted = running && !flag(&data, "url_trusted") && data["url"].is_string();
    view! {
        <PrefGroup title=step>
            <div aria-live="polite">
                <PrefRow label="状态">
                    <AppStatus label=status tone />
                    {running
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="btn"
                                    disabled=move || cancel.get()
                                    on:click=move |_| {
                                        cancel.set(true);
                                        leptos::task::spawn_local(async move {
                                            match api::send::<
                                                Value,
                                            >(
                                                    "POST",
                                                    &format!("/api/office/{id}/job/cancel"),
                                                    &json!({}),
                                                )
                                                .await
                                            {
                                                Ok(_) => tick.update(|n| *n += 1),
                                                Err(e) => toast(e.to_string()),
                                            }
                                            cancel.set(false);
                                        });
                                    }
                                >
                                    "取消"
                                </button>
                            }
                        })}
                </PrefRow>
                {message
                    .map(|message| {
                        view! {
                            <PrefRow label="提示">
                                <span class="pref-text" class:bad=tone == "error">
                                    {message}
                                </span>
                            </PrefRow>
                        }
                    })}
                {code
                    .map(|code| {
                        view! {
                            <PrefRow label="一次性验证码">
                                <strong>{code}</strong>
                            </PrefRow>
                        }
                    })}
                {url
                    .map(|url| {
                        view! {
                            <PrefRow label="授权页面" detail=url.clone()>
                                <a
                                    class="btn primary"
                                    href=url
                                    target="_blank"
                                    rel="noopener noreferrer"
                                >
                                    "打开授权页面 ↗"
                                </a>
                            </PrefRow>
                        }
                    })}
                {qr
                    .map(|qr| {
                        view! {
                            <PrefRow label="二维码" detail="用飞书扫码登录。">
                                <img class="app-qr" src=qr alt="授权二维码" />
                            </PrefRow>
                        }
                    })}
                {untrusted
                    .then(|| {
                        view! {
                            <p class="pref-status bad">
                                "此链接域名不在应用官方域名列表中，请核对后打开。"
                            </p>
                        }
                    })}
            </div>
            <PrefAdvanced title="操作详情">
                {
                    let summary = summary.clone();
                    view! { <p>{summary}</p> }
                }
                {
                    let lines = lines.clone();
                    (!lines.is_empty()).then(|| view! { <pre class="settings-pre">{lines}</pre> })
                }
            </PrefAdvanced>
        </PrefGroup>
    }
}

#[component]
fn LarkSettings(initial: Value) -> impl IntoView {
    let access = RwSignal::new(text(&initial["settings"], "agent_access"));
    let busy = RwSignal::new(false);
    let enabled = RwSignal::new(flag(&initial["settings"]["notify"], "enabled"));
    let target = RwSignal::new(text(&initial["settings"]["notify"], "target"));
    let identity = RwSignal::new(text(&initial["settings"]["notify"], "as"));
    let kinds = RwSignal::new(
        rows(&initial["settings"]["notify"]["kinds"])
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect::<Vec<_>>(),
    );
    let output = RwSignal::new(String::new());
    let skills = format!(
        "本机 {} 个，已导入 {} 个。",
        initial["skills_local"], initial["skills_imported"],
    );
    let patch = move || json!({"notify":{"enabled":enabled.get_untracked(),"target":target.get_untracked().trim(),"as":identity.get_untracked(),"kinds":kinds.get_untracked()}});
    let persist = move |undo: Box<dyn FnOnce() + 'static>| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            if !save("PUT", "/api/office/lark/settings", patch()).await {
                undo();
            }
            busy.set(false);
        });
    };
    let test = Callback::new(move |dry: bool| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        output.set("处理中…".into());
        leptos::task::spawn_local(async move {
            if save("PUT", "/api/office/lark/settings", patch()).await {
                match api::send::<Value>(
                    "POST",
                    "/api/office/lark/test-notify",
                    &json!({"dry_run":dry}),
                )
                .await
                {
                    Ok(r) => output.set(if flag(&r, "ok") {
                        if dry {
                            format!("预览（没有发送）：\n{}", text(&r, "detail"))
                        } else {
                            "已发出测试消息。".into()
                        }
                    } else {
                        format!("发送失败：\n{}", text(&r, "detail"))
                    }),
                    Err(e) => output.set(e.to_string()),
                }
            } else {
                output.set("设置保存失败，测试未执行，请检查提示后重试。".into());
            }
            busy.set(false);
        });
    });
    let import = move || {
        busy.set(true);
        leptos::task::spawn_local(async move {
            if ask(
                "导入并覆盖飞书技能",
                "技能库中同名的飞书技能文件将被替换，本地修改无法恢复。",
                vec![Choice::plain("取消"), Choice::danger("导入并覆盖")],
            )
            .await
                != Some(1)
            {
                busy.set(false);
                return;
            }
            let result = async {
                let found = api::get::<Value>("/api/skills/import").await?;
                let sources = rows(&found)
                    .into_iter()
                    .filter(|s| text(s, "name").starts_with("lark-"))
                    .map(|s| s["source"].clone())
                    .collect::<Vec<_>>();
                api::send::<Value>(
                    "POST",
                    "/api/skills/import",
                    &json!({"sources":sources,"conflict":"overwrite"}),
                )
                .await
            }
            .await;
            match result {
                Ok(r) => {
                    output.set(serde_json::to_string_pretty(&r).unwrap_or_default());
                    toast("飞书技能导入完成，可在智能体能力页配置");
                }
                Err(e) => toast(e.to_string()),
            }
            busy.set(false);
        });
    };
    view! {
        <PrefGroup
            title="智能体权限"
            note="飞书操作在本机执行。高风险写操作以及登录、配置命令始终被限制。"
        >
            <PrefRow label="权限" for_id="lark-access">
                <select
                    id="lark-access"
                    class="settings-input"
                    data-ui-guard=""
                    prop:value=move || access.get()
                    disabled=move || busy.get()
                    on:change=move |e| {
                        let next = event_target_value(&e);
                        let previous = access.get_untracked();
                        access.set(next.clone());
                        busy.set(true);
                        leptos::task::spawn_local(async move {
                            let allowed = next != "write"
                                || ask(
                                        "允许智能体读写飞书",
                                        "智能体将能发送消息、创建文档和修改日程。需批准模式仍逐次确认；绕过权限模式下直接执行。",
                                        vec![Choice::plain("取消"), Choice::plain("允许读写")],
                                    )
                                    .await == Some(1);
                            if allowed
                                && save(
                                        "PUT",
                                        "/api/office/lark/settings",
                                        json!({"agent_access":next}),
                                    )
                                    .await
                            {
                                access.set(next);
                            } else {
                                access.set(previous);
                            }
                            busy.set(false);
                        });
                    }
                >
                    <option value="off" selected=move || access.get() == "off">
                        "关闭"
                    </option>
                    <option value="read" selected=move || access.get() == "read">
                        "只读"
                    </option>
                    <option value="write" selected=move || access.get() == "write">
                        "读写"
                    </option>
                </select>
            </PrefRow>
            <PrefRow label="飞书技能" detail=skills>
                <button
                    type="button"
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| import()
                >
                    "导入或更新技能"
                </button>
            </PrefRow>
            <LarkCalls />
        </PrefGroup>
        <PrefGroup
            title="转发工作提醒"
            note="提醒内容包含标题、正文摘要，可能带工作区名和智能体回复。改动随手保存。"
        >
            <PrefRow label="开启转发" for_id="lark-notify">
                <Switch
                    id="lark-notify"
                    checked=enabled
                    disabled=busy
                    on_change=move |v: bool| {
                        enabled.set(v);
                        persist(Box::new(move || enabled.set(!v)));
                    }
                />
            </PrefRow>
            <PrefRow
                label="接收群或用户"
                detail="群聊填写群 ID（oc_ 开头），个人填写用户 ID（ou_ 开头）。"
                for_id="lark-target"
            >
                <input
                    id="lark-target"
                    class="settings-input"
                    placeholder="oc_… 或 ou_…"
                    autocomplete="off"
                    spellcheck="false"
                    disabled=move || busy.get()
                    prop:value=move || target.get()
                    on:change=move |e| {
                        let next = event_target_value(&e).trim().to_owned();
                        let old = target.get_untracked();
                        if next == old {
                            return;
                        }
                        target.set(next);
                        persist(Box::new(move || target.set(old)));
                    }
                />
            </PrefRow>
            <PrefRow label="发送身份" for_id="lark-as">
                <select
                    id="lark-as"
                    class="settings-input"
                    disabled=move || busy.get()
                    prop:value=move || identity.get()
                    on:change=move |e| {
                        let next = event_target_value(&e);
                        let old = identity.get_untracked();
                        identity.set(next);
                        persist(Box::new(move || identity.set(old)));
                    }
                >
                    <option value="" selected=move || identity.get() == "">
                        "默认身份"
                    </option>
                    <option value="bot" selected=move || identity.get() == "bot">
                        "机器人"
                    </option>
                    <option value="user" selected=move || identity.get() == "user">
                        "我自己"
                    </option>
                </select>
            </PrefRow>
            {NOTIFY_KINDS
                .iter()
                .map(move |&(kind, label)| {
                    let id = format!("lark-kind-{kind}");
                    view! {
                        <PrefRow label=label for_id=id.clone()>
                            <Switch
                                id=id
                                checked=Signal::derive(move || kinds.get().iter().any(|s| s == kind))
                                disabled=busy
                                on_change=move |checked: bool| {
                                    let old = kinds.get_untracked();
                                    kinds
                                        .update(|ks| {
                                            ks.retain(|s| s != kind);
                                            if checked {
                                                ks.push(kind.into());
                                            }
                                        });
                                    persist(Box::new(move || kinds.set(old)));
                                }
                            />
                        </PrefRow>
                    }
                })
                .collect_view()}
            <PrefRow label="测试" detail="按当前设置预览一条提醒，或真的发一条到接收方。">
                <button
                    type="button"
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| test.run(true)
                >
                    "预览提醒"
                </button>
                <button
                    type="button"
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| test.run(false)
                >
                    "发送测试消息"
                </button>
            </PrefRow>
            <Show when=move || !output.get().is_empty()>
                <pre class="settings-pre pref-log" aria-live="polite">
                    {move || output.get()}
                </pre>
            </Show>
        </PrefGroup>
    }
}

#[component]
fn LarkCalls() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/office/lark/calls"));
    view! {
        <PrefAdvanced title="智能体调用记录">
            <p>"只记录命令，不记录消息或文档正文等参数。"</p>
            {move || match data.get() {
                None => {
                    view! { <LoadingState text="读取调用记录…" class="apps-inline-state" /> }
                        .into_any()
                }
                Some(Err(e)) => view! { <InlineError message=e.to_string() /> }.into_any(),
                Some(Ok(v)) => {
                    let list = rows(&v);
                    if list.is_empty() {
                        view! { <EmptyState title="还没有调用记录" class="apps-inline-state" /> }
                            .into_any()
                    } else {
                        list.into_iter()
                            .map(|c| {
                                view! {
                                    <PrefRow
                                        label=format!("lark-cli {}", text(&c, "command"))
                                        detail=format!(
                                            "{} · {} · {}",
                                            text(&c, "verdict"),
                                            text(&c, "risk"),
                                            text(&c, "created_at"),
                                        )
                                    />
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }
            }}
        </PrefAdvanced>
    }
}

#[component]
fn Obsidian(initial: Value, rev: RwSignal<u32>, checking: Signal<bool>) -> impl IntoView {
    let navigate = StoredValue::new_local(use_navigate());
    let app = use_app();
    let busy = RwSignal::new(false);
    let output = RwSignal::new(None::<Value>);
    let vaults = rows(&initial["vaults"]);
    let live = vaults
        .iter()
        .filter(|v| flag(v, "exists"))
        .cloned()
        .collect::<Vec<_>>();
    let first = live.first().map(|v| text(v, "path")).unwrap_or_default();
    let vault = RwSignal::new(
        initial["prefs"]["vault"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or(first),
    );
    let folder = RwSignal::new(text(&initial["prefs"], "folder"));
    let installed = flag(&initial, "installed");
    let has_live = !live.is_empty();
    let status_label = state("obsidian", &initial);
    let status_tone = state_tone("obsidian", &initial);
    let persist =
        move || json!({"vault":vault.get_untracked(),"folder":folder.get_untracked().trim()});
    let put = move |undo: Box<dyn FnOnce() + 'static>| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            if !save("PUT", "/api/office/obsidian/settings", persist()).await {
                undo();
            }
            busy.set(false);
        });
    };
    let create = move |path: String| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            match api::send::<Value>(
                "POST",
                "/api/workspaces",
                &json!({"node":"local","path":path,"name":null,"project":null}),
            )
            .await
            {
                Ok(r) => {
                    app.load_state();
                    navigate
                        .with_value(|go| go(&format!("/w/{}", text(&r, "id")), Default::default()));
                    rev.update(|n| *n += 1);
                }
                Err(e) => toast(e.to_string()),
            }
            busy.set(false);
        });
    };
    let write_test = move || {
        busy.set(true);
        leptos::task::spawn_local(async move {
            if ask(
                "写入测试笔记",
                "将在当前笔记库写入「Blazar 测试笔记」，同名笔记会被覆盖。",
                vec![Choice::plain("取消"), Choice::danger("写入并覆盖")],
            )
            .await
                != Some(1)
            {
                busy.set(false);
                return;
            }
            if save("PUT", "/api/office/obsidian/settings", persist()).await {
                match api::send::<Value>(
                    "POST",
                    "/api/office/obsidian/note",
                    &json!({"title":"Blazar 测试笔记","content":"# Blazar 测试笔记\n\n这篇测试笔记可以删除。\n","overwrite":true}),
                )
                .await
                {
                    Ok(v) => output.set(Some(v)),
                    Err(e) => toast(e.to_string()),
                }
            }
            busy.set(false);
        });
    };
    view! {
        <PageHead
            parent=("接入的软件", "/apps")
            icon=|| view! { <AppLogo id="obsidian" /> }
            title="Obsidian"
            status=move || {
                view! {
                    <AppStatus label=status_label.clone() tone=status_tone />
                    <Checking busy=checking />
                }
            }
        >
            {(!installed)
                .then(|| {
                    view! {
                        <a
                            class="btn primary"
                            href="https://obsidian.md"
                            target="_blank"
                            rel="noopener noreferrer"
                        >
                            "安装 Obsidian ↗"
                        </a>
                    }
                })}
        </PageHead>
        <div class="detail-body">
            <PrefGroup title="笔记库" note="列表来自 Obsidian 记录的本机 Markdown 文件夹，无需插件或登录。">
                {if !installed {
                    view! {
                        <PrefRow label="Obsidian" detail="装好并打开一个库后，回到这里会自动重新检测。">
                            <span class="pref-text">"未安装"</span>
                        </PrefRow>
                    }
                        .into_any()
                } else if vaults.is_empty() {
                    view! {
                        <PrefRow
                            label="还没有笔记库"
                            detail="先在 Obsidian 新建或打开一个库，回到这里会自动重新检测。"
                        />
                    }
                        .into_any()
                } else {
                    vaults
                        .into_iter()
                        .map(move |v| {
                            let path = text(&v, "path");
                            let ws = text(&v, "workspace_id");
                            let exists = flag(&v, "exists");
                            let detail = if exists {
                                format!("{} 篇笔记 · {path}", v["notes"])
                            } else {
                                format!("找不到文件夹 · {path}")
                            };
                            view! {
                                <PrefRow label=text(&v, "name") detail=detail>
                                    {if !exists {
                                        view! { <span class="pref-text">"找不到"</span> }.into_any()
                                    } else if !ws.is_empty() {
                                        view! {
                                            <a class="btn" href=format!("/w/{}", api::enc(&ws))>
                                                "打开工作区"
                                            </a>
                                        }
                                            .into_any()
                                    } else {
                                        view! {
                                            <button
                                                type="button"
                                                class="btn"
                                                disabled=move || busy.get()
                                                on:click=move |_| create(path.clone())
                                            >
                                                "创建工作区"
                                            </button>
                                        }
                                            .into_any()
                                    }}
                                </PrefRow>
                            }
                        })
                        .collect_view()
                        .into_any()
                }}
            </PrefGroup>
            {has_live
                .then(|| {
                    view! {
                        <PrefGroup
                            title="存到 Obsidian"
                            note="任务笔记的默认保存位置。文件夹填写库内相对路径，改动随手保存。"
                        >
                            <PrefRow label="默认笔记库" for_id="obsidian-vault">
                                <select
                                    id="obsidian-vault"
                                    class="settings-input"
                                    disabled=move || busy.get()
                                    prop:value=move || vault.get()
                                    on:change=move |e| {
                                        let next = event_target_value(&e);
                                        let old = vault.get_untracked();
                                        vault.set(next);
                                        put(Box::new(move || vault.set(old)));
                                    }
                                >
                                    {live
                                        .into_iter()
                                        .map(|v| {
                                            let path = text(&v, "path");
                                            view! {
                                                <option
                                                    value=text(&v, "path")
                                                    selected=move || vault.get() == path
                                                >
                                                    {text(&v, "name")}
                                                </option>
                                            }
                                        })
                                        .collect_view()}
                                </select>
                            </PrefRow>
                            <PrefRow label="文件夹" for_id="obsidian-folder">
                                <input
                                    id="obsidian-folder"
                                    class="settings-input"
                                    placeholder="Blazar"
                                    autocomplete="off"
                                    spellcheck="false"
                                    disabled=move || busy.get()
                                    prop:value=move || folder.get()
                                    on:change=move |e| {
                                        let next = event_target_value(&e).trim().to_owned();
                                        let old = folder.get_untracked();
                                        if next == old {
                                            return;
                                        }
                                        folder.set(next);
                                        put(Box::new(move || folder.set(old)));
                                    }
                                />
                            </PrefRow>
                            <PrefRow
                                label="测试"
                                detail="在当前笔记库写入「Blazar 测试笔记」，同名笔记会被覆盖。"
                            >
                                <button
                                    type="button"
                                    class="btn"
                                    disabled=move || busy.get()
                                    on:click=move |_| write_test()
                                >
                                    "写入测试笔记"
                                </button>
                            </PrefRow>
                            {move || {
                                output
                                    .get()
                                    .map(|v| {
                                        view! {
                                            <p class="pref-status" aria-live="polite">
                                                "测试笔记已保存到 "
                                                <code>{text(&v, "path")}</code>
                                                "。"
                                                <a href=text(&v, "open")>"在 Obsidian 打开"</a>
                                            </p>
                                        }
                                    })
                            }}
                        </PrefGroup>
                    }
                })}
        </div>
    }
}
