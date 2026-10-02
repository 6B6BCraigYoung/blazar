//! 运行时与账号：每个运行时一张卡，下面挂它的账号，一行一个；在用的那行高亮，别的行点「使用」就换过去。

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, Account, Accounts, Runtime, Runtimes};
use crate::fmt;
use crate::realtime::use_bus;

// 支持多账号的运行时；别的运行时只显示装没装、登没登。
const WITH_ACCOUNTS: [&str; 2] = ["claude", "codex"];

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
        api::get::<Accounts>("/api/accounts")
    });
    let error = RwSignal::new(None::<String>);

    view! {
        <div class="page">
            <div class="page-head">
                <h1>"运行时"</h1>
                <span class="sub">
                    {move || runtimes.get().and_then(Result::ok)
                        .map(|r| format!("{} · 本机", r.machine.hostname))}
                </span>
                <span class="grow"></span>
                <button class="btn" on:click=move |_| rescan.update(|n| *n += 1)>"重新扫描"</button>
            </div>
            {move || error.get().map(|e| view! { <div class="err-line">{e}</div> })}
            {move || match runtimes.get() {
                None => view! { <div class="empty">"加载中…"</div> }.into_any(),
                Some(Err(e)) => view! { <div class="empty err-line">{e.to_string()}</div> }.into_any(),
                Some(Ok(r)) => r.runtimes.into_iter()
                    .filter(|rt| rt.installed)
                    .map(|rt| view! { <RuntimeCard rt accounts error/> })
                    .collect_view()
                    .into_any(),
            }}
        </div>
    }
}

#[component]
fn RuntimeCard(
    rt: Runtime,
    accounts: LocalResource<Result<Accounts, api::ApiError>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let id = rt.id.clone();
    let multi = WITH_ACCOUNTS.contains(&id.as_str());
    let acc = move || accounts.get().and_then(Result::ok);
    let pid = id.clone();
    let active_label = move || {
        let a = acc()?;
        let on = a.active(&pid)?;
        a.accounts.into_iter().find(|x| x.id == on).map(|x| x.label)
    };
    let mark = if id == "claude" {
        "A"
    } else if id == "codex" {
        "◎"
    } else {
        "·"
    };
    let authed = rt.authed;

    view! {
        <section class="card" aria-label=rt.label.clone()>
            <div class="card-head">
                <span class="rt-mark" data-rt=id.clone()>{mark}</span>
                <b>{rt.label.clone()}</b>
                <span class="muted" style="font-size:12px">{rt.version.clone()}</span>
                {move || if multi {
                    active_label().map(|l| view! { <span class="state ok">{format!("在用：{l}")}</span> }).into_any()
                } else {
                    match authed {
                        Some(true) => view! { <span class="state ok">"已登录"</span> }.into_any(),
                        Some(false) => view! { <span class="state bad">"未登录"</span> }.into_any(),
                        None => ().into_any(),
                    }
                }}
                <span class="grow"></span>
                <a class="btn ghost" href=format!("/#/runtimes/{id}")>"设置"</a>
            </div>
            {multi.then(|| {
                let id = id.clone();
                view! {
                    <div class="acc-row head">
                        <span>"账号"</span><span>"状态"</span><span>"额度"</span><span>"最近使用"</span>
                        <span style="text-align:right">
                            <a href=format!("/#/runtimes/{id}?view=accounts")>"＋ 添加账号"</a>
                        </span>
                    </div>
                    {move || {
                        let Some(a) = acc() else { return ().into_any() };
                        let on = a.active(&id);
                        a.accounts.into_iter()
                            .filter(|x| x.provider == id)
                            .map(|x| {
                                let active = on.as_deref() == Some(x.id.as_str());
                                view! { <AccountRow a=x active error/> }
                            })
                            .collect_view()
                            .into_any()
                    }}
                }
            })}
        </section>
    }
}

fn status(a: &Account) -> impl IntoView + use<> {
    let tok = a.kind == "token";
    let (cls, text) = if a.disabled {
        ("state muted", "已停用")
    } else if a.status == "ok" {
        ("state ok", if tok { "已配置 token" } else { "已登录" })
    } else if a.status == "logged_out" {
        ("state bad", if tok { "token 无效" } else { "未登录" })
    } else {
        ("state muted", "无法判断")
    };
    view! { <span class=cls>{text}</span> }
}

fn quota(a: &Account) -> AnyView {
    if a.windows.is_empty() {
        let hint = if a.kind == "token" || a.provider == "codex" {
            "还没查过 —— 「⋯」里点「刷新额度」"
        } else {
            "还没有数据 —— 用它跑一次会话就有了"
        };
        return view! { <span class="muted" style="font-size:12px">{hint}</span> }.into_any();
    }
    a.windows
        .iter()
        .map(|w| {
            let pct = (w.utilization * 100.0).round().clamp(0.0, 100.0);
            let lvl = if pct >= 90.0 {
                "bad"
            } else if pct >= 70.0 {
                "warn"
            } else {
                "ok"
            };
            view! {
                <div class="qbar" data-lvl=lvl>
                    <span class="l">{fmt::window_label(&w.name).to_owned()}</span>
                    <span class="t"><i style=format!("width:{pct}%")></i></span>
                    <span class="v">{format!("{pct}%")}</span>
                    <span class="r">{fmt::resets(w.resets_at.as_deref())}</span>
                </div>
            }
        })
        .collect_view()
        .into_any()
}

#[component]
fn AccountRow(a: Account, active: bool, error: RwSignal<Option<String>>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let menu = RwSignal::new(false);
    let usable = a.usable();
    let title = [
        a.email.clone().unwrap_or_default(),
        a.plan.clone().unwrap_or_default(),
        if a.kind == "token" {
            "长期 token".into()
        } else {
            "浏览器登录".into()
        },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");

    let a_use = a.clone();
    let on_use = move |_| {
        let a = a_use.clone();
        busy.set(true);
        spawn_local(async move {
            // 成功后 hub 会推 accounts_changed，整页自动刷新。
            if let Err(e) = api::use_account(&a).await {
                error.set(Some(format!("换不过去：{e}")));
            }
            busy.set(false);
        });
    };
    let id = a.id.clone();
    let on_quota = move |_| {
        let id = id.clone();
        menu.set(false);
        busy.set(true);
        spawn_local(async move {
            if let Err(e) = api::refresh_quota(&id).await {
                error.set(Some(format!("额度没查到：{e}")));
            }
            busy.set(false);
        });
    };

    view! {
        <div class="acc-row" class:on=active>
            <span class="name"><b title=title>{a.label.clone()}</b></span>
            {status(&a)}
            <div class="quota">{quota(&a)}</div>
            <span class="last">
                {a.last_used_at.as_deref().map_or_else(|| "—".to_owned(), fmt::ago)}
            </span>
            <span class="act">
                {if active {
                    view! { <span class="pill-on">"使用中"</span> }.into_any()
                } else {
                    view! {
                        <button class="btn" style="height:26px" on:click=on_use
                            disabled=move || busy.get() || !usable
                            title=(!usable).then_some("这个账号现在不可用")>"使用"</button>
                    }.into_any()
                }}
                <span class="more">
                    <button class="btn ghost" aria-label="更多" on:click=move |_| menu.update(|m| *m = !*m)>"⋯"</button>
                    <Show when=move || menu.get()>
                        <div class="menu" on:mouseleave=move |_| menu.set(false)>
                            <button on:click=on_quota.clone()>"刷新额度"</button>
                            <a href=format!("/#/runtimes/{}?view=accounts", a.provider)>"在旧界面管理"</a>
                        </div>
                    </Show>
                </span>
            </span>
        </div>
    }
}
