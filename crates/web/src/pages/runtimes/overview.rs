use leptos::prelude::*;

use crate::api::{self, Accounts, Runtime};
use crate::components::status::Status;
use crate::rt_logo;

use super::{AccountsSection, WITH_ACCOUNTS, active_account, runtime_status};

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
        </section>
    }
}
