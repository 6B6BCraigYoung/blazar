use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, Account, Accounts, Runtime};
use crate::components::status::Status;
use crate::components::toast::toast_error;
use crate::fmt;
use crate::rt_logo;

use super::accounts::{plan_label, quota};
use super::{AccountsSection, WITH_ACCOUNTS, active_account, runtime_status};

#[component]
fn CliQuotaRow(a: Account) -> impl IntoView {
    let busy = RwSignal::new(false);
    let id = a.id.clone();
    let refresh = move |_| {
        if busy.get_untracked() {
            return;
        }
        let id = id.clone();
        busy.set(true);
        spawn_local(async move {
            if let Err(e) = api::refresh_quota(&id).await {
                toast_error("没查到额度", &e);
            }
            let _ = busy.try_set(false);
        });
    };
    let sub = [
        a.email.clone().unwrap_or_default(),
        a.plan
            .as_deref()
            .map(|p| plan_label(p).to_owned())
            .unwrap_or_default(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    view! {
        <div class="acc-row">
            <span class="acc-check"></span>
            <span class="name">
                <b>"本机登录"</b>
                {(!sub.is_empty()).then(|| view! { <div class="acc-sub">{sub}</div> })}
            </span>
            <span class="acc-state"></span>
            <div class="quota">{quota(&a)}</div>
            <span class="last">
                {a.last_used_at.as_deref().map_or_else(|| "—".to_owned(), fmt::ago)}
            </span>
            <span class="act">
                <button class="btn small" disabled=move || busy.get() on:click=refresh>
                    {move || if busy.get() { "查询中…" } else { "刷新额度" }}
                </button>
            </span>
        </div>
    }
}

#[component]
pub(super) fn RuntimeCard(
    rt: Runtime,
    accounts: LocalResource<Result<Accounts, api::ApiError>>,
) -> impl IntoView {
    let id = rt.id.clone();
    let multi = WITH_ACCOUNTS.iter().any(|x| x.0 == id);
    let pid = id.clone();
    let active = move || {
        accounts
            .get()
            .and_then(Result::ok)
            .and_then(|a| active_account(&a, &pid))
    };
    let cid = id.clone();
    let cli = move || {
        accounts
            .get()
            .and_then(Result::ok)
            .and_then(|a| a.cli.into_iter().find(|x| x.provider == cid && !x.disabled))
    };
    let rt_s = StoredValue::new(rt.clone());
    let title = [
        rt.version.clone().map(|v| format!("v{v}")),
        rt.path.clone(),
        Some(if rt.remote_hands {
            "可用于任意工作区".to_owned()
        } else {
            "只能用于本机工作区".to_owned()
        }),
        (rt.cost_7d > 0.0).then(|| format!("近 7 天 ${:.2}", rt.cost_7d)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    view! {
        <section class="card" aria-label=rt.label.clone()>
            <div class="card-head">
                <span inner_html=rt_logo::mark(&id)></span>
                <b title=title>{rt.label.clone()}</b>
                {move || {
                    let a = active();
                    let (text, tone) = rt_s.with_value(|rt| runtime_status(rt, a.as_ref()));
                    view! { <Status tone label=text title=a.map(|a| a.label) /> }
                }}
                <span class="grow"></span>
                <a class="btn ghost" href=format!("/runtimes/{id}")>
                    "设置"
                </a>
            </div>
            {multi.then(|| view! { <AccountsSection provider=id.clone() accounts /> })}
            {move || cli().map(|a| view! { <CliQuotaRow a /> })}
        </section>
    }
}
