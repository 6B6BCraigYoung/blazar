use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::{alerts, api, app_state::use_app};

use super::{flag, rows, save, text};

pub(crate) const NOTIFY_KINDS: &[(&str, &str)] = &[
    ("run_done", "运行完成"),
    ("run_failed", "运行失败"),
    ("approval", "等你批准"),
    ("question", "需要回答"),
    ("autopilot_paused", "自动化暂停"),
    ("rate_limit", "额度告警"),
    ("job", "后台任务"),
    ("alert", "提醒规则"),
];

#[component]
pub(super) fn NotifySettings() -> impl IntoView {
    let app = use_app();
    let sound = RwSignal::new(alerts::sound_prefs());
    let system = RwSignal::new(alerts::notify_on());
    let busy = RwSignal::new(false);
    let server = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let update_sound = move |p: alerts::SoundPrefs| {
        alerts::set_sound_prefs(&p);
        sound.set(p);
        app.alerts_rev.update(|n| *n += 1);
        toast("已保存");
    };
    view! {
        <section class="card settings-card">
            <h3>"本机提醒"</h3>
            {(!alerts::notify_supported())
                .then(|| {
                    view! {
                        <p class="muted">
                            "当前环境不支持系统通知，可使用提示音。"
                        </p>
                    }
                })}
            <div class="settings-row">
                <label for="settings-system-notify">"系统通知"</label>
                <input
                    id="settings-system-notify"
                    type="checkbox"
                    disabled=move || busy.get() || !alerts::notify_supported()
                    prop:checked=move || system.get()
                    on:change=move |e| {
                        let enabled = event_target_checked(&e);
                        system.set(enabled);
                        busy.set(true);
                        leptos::task::spawn_local(async move {
                            let on = !enabled || alerts::notify_granted()
                                || alerts::request_permission().await == "granted";
                            alerts::set_notify(enabled && on);
                            system.set(enabled && on);
                            app.alerts_rev.update(|n| *n += 1);
                            if enabled && !on {
                                toast("浏览器没有授予通知权限");
                            }
                            busy.set(false);
                        });
                    }
                />
            </div>
            <div class="settings-row">
                <label for="settings-sound">"提示音"</label>
                <input
                    id="settings-sound"
                    type="checkbox"
                    prop:checked=move || sound.get().on
                    on:change=move |e| {
                        let mut p = sound.get_untracked();
                        p.on = event_target_checked(&e);
                        update_sound(p);
                    }
                />
            </div>
            <div class="settings-row">
                <label for="settings-tone">"音色"</label>
                <select
                    id="settings-tone"
                    class="settings-input"
                    prop:value=move || sound.get().tone
                    on:change=move |e| {
                        let mut p = sound.get_untracked();
                        p.tone = event_target_value(&e);
                        update_sound(p);
                        alerts::play("done", true);
                    }
                >
                    <option value="soft" selected=move || sound.get().tone == "soft">
                        "柔和"
                    </option>
                    <option value="bright" selected=move || sound.get().tone == "bright">
                        "明亮"
                    </option>
                    <option value="wood" selected=move || sound.get().tone == "wood">
                        "木质"
                    </option>
                </select>
            </div>
            <div class="settings-row">
                <label for="settings-volume">"音量"</label>
                <input
                    id="settings-volume"
                    type="range"
                    min="0.05"
                    max="1"
                    step="0.05"
                    prop:value=move || sound.get().volume
                    on:change=move |e| {
                        let mut p = sound.get_untracked();
                        p.volume = event_target_value(&e).parse().unwrap_or(0.5);
                        update_sound(p);
                    }
                />
            </div>
            <div class="settings-actions">
                <button class="btn" on:click=|_| alerts::play("done", true)>
                    "试听完成提示"
                </button>
                <button class="btn" on:click=|_| alerts::play("attention", true)>
                    "试听待处理提示"
                </button>
            </div>
        </section>
        {move || match server.get() {
            None => view! { <LoadingState text="正在加载提醒设置…" /> }.into_any(),
            Some(Err(e)) => {
                view! {
                    <InlineError
                        message=format!("无法加载提醒设置：{e}")
                        retry=Callback::new(move |_| server.refetch())
                    />
                }
                    .into_any()
            }
            Some(Ok(p)) => view! { <InboxSettings initial=p /> }.into_any(),
        }}
        <AlertRuleSettings />
        <section class="card settings-card">
            <h3>"转发到飞书"</h3>
            <a href="/apps/lark">"配置飞书提醒转发 →"</a>
        </section>
    }
}

#[component]
fn InboxSettings(initial: Value) -> impl IntoView {
    let muted = RwSignal::new(
        rows(&initial["inbox"]["muted"])
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect::<Vec<_>>(),
    );
    let busy = RwSignal::new(false);
    view! {
        <section class="card settings-card">
            <h3>"提醒哪些事项"</h3>
            <p class="muted">
                "关闭的类型仍记录到收件箱，不计未读、不提示或转发。"
            </p>
            {NOTIFY_KINDS
                .iter()
                .map(move |&(key, label)| {
                    view! {
                        <div class="settings-row">
                            <label for=format!("notify-kind-{key}")>{label}</label>
                            <input
                                id=format!("notify-kind-{key}")
                                type="checkbox"
                                disabled=move || busy.get()
                                prop:checked=move || !muted.get().iter().any(|s| s == key)
                                on:change=move |e| {
                                    let enabled = event_target_checked(&e);
                                    let old = muted.get_untracked();
                                    let mut next = old.clone();
                                    next.retain(|x| x != key);
                                    if !enabled {
                                        next.push(key.into());
                                    }
                                    muted.set(next.clone());
                                    busy.set(true);
                                    leptos::task::spawn_local(async move {
                                        if save(
                                                "PUT",
                                                "/api/settings",
                                                json!({"inbox":{"muted":next}}),
                                            )
                                            .await
                                        {
                                            muted.set(next);
                                        } else {
                                            muted.set(old);
                                        }
                                        busy.set(false);
                                    });
                                }
                            />
                        </div>
                    }
                })
                .collect_view()}
        </section>
    }
}

#[component]
fn AlertRuleSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/alert-rules"));
    view! {
        <section class="card settings-card">
            <h3>"提醒规则"</h3>
            <p class="muted">
                "hub 每 30 秒检查一次，事情发生时也会立刻检查。同一件事持续期间只提醒一次，消失后在收件箱里标为已解决。"
            </p>
            {move || match data.get() {
                None => view! { <LoadingState text="正在加载提醒规则…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=format!("无法加载提醒规则：{e}")
                            retry=Callback::new(move |_| data.refetch())
                        />
                    }
                        .into_any()
                }
                Some(Ok(v)) => {
                    rows(&v["rules"])
                        .into_iter()
                        .map(|r| view! { <AlertRuleRow rule=r /> })
                        .collect_view()
                        .into_any()
                }
            }}
        </section>
    }
}

#[component]
fn AlertRuleRow(rule: Value) -> impl IntoView {
    let key = text(&rule, "key");
    let unit = text(&rule, "unit");
    let (min, max) = (
        rule["min"].as_i64().unwrap_or(0),
        rule["max"].as_i64().unwrap_or(0),
    );
    let on = RwSignal::new(flag(&rule, "enabled"));
    let threshold = RwSignal::new(rule["threshold"].as_i64());
    let busy = RwSignal::new(false);
    let put = move |key: String, body: Value, undo: Box<dyn FnOnce() + 'static>| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            if !save("PUT", "/api/settings", json!({ "rules": { key: body } })).await {
                undo();
            }
            busy.set(false);
        });
    };
    let switch_key = key.clone();
    let number_key = key.clone();
    view! {
        <div class="settings-row alert-rule">
            <div class="settings-label">
                <b>{text(&rule, "label")}</b>
                <span>{text(&rule, "about")}</span>
            </div>
            <div class="settings-actions alert-rule-controls">
                <span class="alert-rule-threshold">
                    {threshold
                        .get_untracked()
                        .map(|_| {
                            let key = number_key.clone();
                            view! {
                                <input
                                    id=format!("alert-rule-{key}-threshold")
                                    class="settings-input"
                                    type="number"
                                    min=min
                                    max=max
                                    aria-label=format!("「{}」的阈值（{unit}）", text(&rule, "label"))
                                    disabled=move || busy.get()
                                    prop:value=move || {
                                        threshold.get().map(|n| n.to_string()).unwrap_or_default()
                                    }
                                    on:change=move |e| {
                                        let old = threshold.get_untracked();
                                        match event_target_value(&e).trim().parse::<i64>() {
                                            Ok(n) if (min..=max).contains(&n) => {
                                                if old == Some(n) {
                                                    return;
                                                }
                                                threshold.set(Some(n));
                                                put(
                                                    key.clone(),
                                                    json!({ "threshold": n }),
                                                    Box::new(move || threshold.set(old)),
                                                );
                                            }
                                            _ => {
                                                event_target::<web_sys::HtmlInputElement>(&e)
                                                    .set_value(
                                                        &old.map(|n| n.to_string()).unwrap_or_default(),
                                                    );
                                                toast(format!("阈值要在 {min} 到 {max} 之间"));
                                            }
                                        }
                                    }
                                />
                                <span class="alert-rule-unit">{unit.clone()}</span>
                            }
                        })}
                </span>
                <input
                    id=format!("alert-rule-{switch_key}")
                    type="checkbox"
                    aria-label=format!("启用「{}」", text(&rule, "label"))
                    disabled=move || busy.get()
                    prop:checked=move || on.get()
                    on:change=move |e| {
                        let v = event_target_checked(&e);
                        on.set(v);
                        put(switch_key.clone(), json!({ "enabled": v }), Box::new(move || on.set(!v)));
                    }
                />
            </div>
        </div>
    }
}
