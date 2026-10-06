use leptos::prelude::*;

use crate::api::{self, Accounts, Runtime};
use crate::rt_logo;

use super::{AccountsSection, WITH_ACCOUNTS};

#[component]
pub(super) fn RuntimeCard(
    rt: Runtime,
    accounts: LocalResource<Result<Accounts, api::ApiError>>,
) -> impl IntoView {
    let id = rt.id.clone();
    let multi = WITH_ACCOUNTS.iter().any(|x| x.0 == id);
    let acc = move || accounts.get().and_then(Result::ok);
    let pid = id.clone();
    let active = move || {
        let a = acc()?;
        let on = active_id(&a, &pid)?;
        a.accounts.into_iter().find(|x| x.id == on)
    };
    let authed = rt.authed;
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
                {move || if multi {
                    match active() {
                        Some(a) => { let usable = a.usable(); view! { <span class=if usable { "state ok" } else { "state bad" } title=a.label>{if usable { "可用" } else { "需登录" }}</span> }.into_any() },
                        None => view! { <span class="state muted">"未选择"</span> }.into_any(),
                    }
                } else {
                    match authed {
                        Some(true) => view! { <span class="state ok">"已登录"</span> }.into_any(),
                        Some(false) => view! { <span class="state bad">"未登录"</span> }.into_any(),
                        None => view! { <span class="state muted" title="暂未获取登录状态，可重新扫描">"待检查"</span> }.into_any(),
                    }
                }}
                <span class="grow"></span>
                <a class="btn ghost" href=format!("/runtimes/{id}")>"设置"</a>
            </div>
            {multi.then(|| view! { <AccountsSection provider=id.clone() accounts/> })}
        </section>
    }
}

fn active_id(a: &Accounts, p: &str) -> Option<String> {
    a.active(p)
}
