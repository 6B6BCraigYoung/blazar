use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow, Switch};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;

use super::save;

const ATTEMPTS_MAX: u64 = 100;

#[component]
pub(super) fn AutoResumeSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let busy = RwSignal::new(false);
    move || match data.get() {
        None => view! { <LoadingState text="正在加载设置…" /> }.into_any(),
        Some(Err(e)) => view! {
            <InlineError
                lead="无法加载设置"
                error=e.clone()
                retry=Callback::new(move |_| data.refetch())
            />
        }
        .into_any(),
        Some(Ok(p)) => {
            let on = RwSignal::new(p["auto_resume"]["enabled"].as_bool().unwrap_or(true));
            let attempts = RwSignal::new(p["auto_resume"]["max_attempts"].as_u64().unwrap_or(20));
            view! {
                <PrefGroup title="运行">
                    <PrefRow
                        label="断网自动续跑"
                        detail="断网、证书被拦或服务繁忙导致停下时，网络恢复后自动「继续」。"
                        for_id="settings-auto-resume"
                    >
                        <Switch
                            id="settings-auto-resume"
                            checked=on
                            disabled=busy
                            on_change=move |v: bool| {
                                on.set(v);
                                busy.set(true);
                                leptos::task::spawn_local(async move {
                                    if !save(
                                            "PUT",
                                            "/api/settings",
                                            json!({"auto_resume": {"enabled": v}}),
                                        )
                                        .await
                                    {
                                        on.set(!v);
                                    }
                                    busy.set(false);
                                });
                            }
                        />
                    </PrefRow>
                    <PrefAdvanced title="断网自动续跑">
                        <PrefRow
                            label="最多重试次数"
                            detail="用完还没恢复就停下，在中控台发一张卡片。取 1 到 100。"
                            for_id="settings-auto-resume-attempts"
                        >
                            <input
                                id="settings-auto-resume-attempts"
                                class="settings-input"
                                type="number"
                                min="1"
                                max=ATTEMPTS_MAX
                                disabled=move || busy.get()
                                prop:value=move || attempts.get().to_string()
                                on:change=move |e| {
                                    let old = attempts.get_untracked();
                                    match event_target_value(&e).trim().parse::<u64>() {
                                        Ok(n) if (1..=ATTEMPTS_MAX).contains(&n) => {
                                            if n == old {
                                                return;
                                            }
                                            attempts.set(n);
                                            busy.set(true);
                                            leptos::task::spawn_local(async move {
                                                if !save(
                                                        "PUT",
                                                        "/api/settings",
                                                        json!({"auto_resume": {"max_attempts": n}}),
                                                    )
                                                    .await
                                                {
                                                    attempts.set(old);
                                                }
                                                busy.set(false);
                                            });
                                        }
                                        _ => {
                                            event_target::<web_sys::HtmlInputElement>(&e)
                                                .set_value(&old.to_string());
                                            toast(format!("次数要在 1 到 {ATTEMPTS_MAX} 之间"));
                                        }
                                    }
                                }
                            />
                        </PrefRow>
                    </PrefAdvanced>
                </PrefGroup>
            }
            .into_any()
        }
    }
}
