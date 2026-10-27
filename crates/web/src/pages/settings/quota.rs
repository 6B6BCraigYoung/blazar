use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api::{self, Accounts};
use crate::components::prefs::{PrefGroup, PrefRow};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::pages::runtimes::accounts::quota;

use super::save;

#[component]
pub(super) fn QuotaSettings() -> impl IntoView {
    let prefs = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let accounts = LocalResource::new(|| api::get::<Accounts>("/api/accounts"));
    let busy = RwSignal::new(false);
    move || match (prefs.get(), accounts.get()) {
        (None, _) | (_, None) => view! { <LoadingState text="正在加载额度…" /> }.into_any(),
        (Some(Err(e)), _) | (_, Some(Err(e))) => view! {
            <InlineError
                message=format!("无法加载额度：{e}")
                retry=Callback::new(move |_| {
                    prefs.refetch();
                    accounts.refetch();
                })
            />
        }
        .into_any(),
        (Some(Ok(p)), Some(Ok(list))) => {
            let pct = |t: f64| (t * 100.0).round() as u64;
            let limit = RwSignal::new(pct(p["quota"]["threshold"].as_f64().unwrap_or(0.85)));
            let rows = list
                .accounts
                .into_iter()
                .filter(|a| a.provider == "claude" && !a.disabled)
                .map(|a| {
                    let label = match a
                        .email
                        .as_deref()
                        .filter(|e| !e.is_empty() && *e != a.label)
                    {
                        Some(email) => format!("{} · {email}", a.label),
                        None => a.label.clone(),
                    };
                    view! { <PrefRow label=label>{quota(&a)}</PrefRow> }
                })
                .collect_view();
            view! {
                <PrefGroup title="额度分摊">
                    {rows}
                    <PrefRow
                        label="不再派新会话的用量阈值"
                        detail="Claude 选「自动分摊」时，五小时或七天窗口超过这个比例的账号不接新会话；全都超了就挑最低的照常跑。"
                        for_id="settings-quota-threshold"
                    >
                        <input
                            id="settings-quota-threshold"
                            class="settings-input"
                            type="number"
                            min="5"
                            max="100"
                            disabled=move || busy.get()
                            prop:value=move || limit.get().to_string()
                            on:change=move |e| {
                                let old = limit.get_untracked();
                                match event_target_value(&e).trim().parse::<u64>() {
                                    Ok(n) if (5..=100).contains(&n) => {
                                        if n == old {
                                            return;
                                        }
                                        limit.set(n);
                                        busy.set(true);
                                        leptos::task::spawn_local(async move {
                                            let t = n as f64 / 100.0;
                                            if !save("PUT", "/api/settings", json!({"quota": {"threshold": t}}))
                                                .await
                                            {
                                                limit.set(old);
                                            }
                                            busy.set(false);
                                        });
                                    }
                                    _ => {
                                        event_target::<web_sys::HtmlInputElement>(&e)
                                            .set_value(&old.to_string());
                                        toast("阈值要在 5 到 100 之间");
                                    }
                                }
                            }
                        />
                        <span class="muted">"%"</span>
                    </PrefRow>
                </PrefGroup>
            }
                .into_any()
        }
    }
}
