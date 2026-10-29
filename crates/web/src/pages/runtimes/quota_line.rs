use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::PrefRow;
use crate::components::toast::{flash, toast, toast_error};

/// 「自动分摊」的额度线：超过就不再派新对话，同时提醒一次（原先在设置 → 对话与运行）。
#[component]
pub(super) fn QuotaLine() -> impl IntoView {
    let prefs = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let busy = RwSignal::new(false);
    move || {
        let p = prefs.get()?.ok()?;
        let pct = |t: f64| (t * 100.0).round() as u64;
        let limit = RwSignal::new(pct(p["quota"]["threshold"].as_f64().unwrap_or(0.85)));
        Some(view! {
            <PrefRow
                label="自动分摊的额度线"
                detail="选「自动分摊」时，五小时或七天窗口用到这个比例的账号不再接新对话，并提醒你一次；全都超了就挑用得最少的照常跑。"
                for_id="runtime-quota-threshold"
            >
                <input
                    id="runtime-quota-threshold"
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
                                    match api::send::<
                                        Value,
                                    >("PUT", "/api/settings", &json!({"quota": {"threshold": t}}))
                                        .await
                                    {
                                        Ok(_) => flash("已保存"),
                                        Err(e) => {
                                            toast_error("没保存上", &e);
                                            limit.set(old);
                                        }
                                    }
                                    busy.set(false);
                                });
                            }
                            _ => {
                                event_target::<web_sys::HtmlInputElement>(&e)
                                    .set_value(&old.to_string());
                                toast("要在 5 到 100 之间");
                            }
                        }
                    }
                />
                <span class="pref-unit">"%"</span>
            </PrefRow>
        })
    }
}
