use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow, Switch};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::{flash, toast};
use crate::{alerts, api, app_state::use_app};

use super::{flag, rows, save, text};

pub(crate) const NOTIFY_KINDS: &[(&str, &str)] = &[
    ("run_done", "运行完成"),
    ("run_failed", "运行失败"),
    ("approval", "等你批准"),
    ("question", "在问你"),
    ("denied", "被拦下"),
    ("autopilot_paused", "自动化已暂停"),
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
    let supported = alerts::notify_supported();
    let sound_on = Signal::derive(move || sound.get().on);
    let update_sound = move |p: alerts::SoundPrefs| {
        alerts::set_sound_prefs(&p);
        sound.set(p);
        app.alerts_rev.update(|n| *n += 1);
        flash("已保存");
    };
    view! {
        <PrefGroup title="本机提醒">
            <PrefRow
                label="系统通知"
                detail=if supported { "" } else { "当前环境不支持系统通知。" }
                for_id="settings-system-notify"
            >
                <Switch
                    id="settings-system-notify"
                    checked=system
                    disabled=Signal::derive(move || busy.get() || !supported)
                    on_change=move |enabled: bool| {
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
                            } else {
                                flash("已保存");
                            }
                            busy.set(false);
                        });
                    }
                />
            </PrefRow>
            <PrefRow label="提示音" for_id="settings-sound">
                <Switch
                    id="settings-sound"
                    checked=sound_on
                    on_change=move |on: bool| {
                        let mut p = sound.get_untracked();
                        p.on = on;
                        update_sound(p);
                    }
                />
            </PrefRow>
            <Show when=move || sound_on.get()>
                <PrefRow label="音色" for_id="settings-tone">
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
                </PrefRow>
                <PrefRow label="音量" for_id="settings-volume">
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
                </PrefRow>
                <PrefRow label="试听">
                    <button type="button" class="btn small" on:click=|_| alerts::play("done", true)>
                        "完成提示"
                    </button>
                    <button
                        type="button"
                        class="btn small"
                        on:click=|_| alerts::play("attention", true)
                    >
                        "待处理提示"
                    </button>
                </PrefRow>
            </Show>
        </PrefGroup>
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
        <PrefGroup title="飞书">
            <PrefRow label="转发到飞书" detail="把提醒同时发到飞书机器人。">
                <a class="btn" href="/apps/lark">
                    "配置…"
                </a>
            </PrefRow>
        </PrefGroup>
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
        <PrefGroup title="提醒哪些事项">
            {move || {
                let label = match muted.with(Vec::len) {
                    0 => "所有事项都提醒".to_owned(),
                    n => format!("关掉了 {n} 类"),
                };
                view! {
                    <PrefRow
                        label=label
                        detail="关掉的类型仍记录到中控台，只是不计未读、不提示也不转发。"
                    />
                }
            }} <PrefAdvanced title="提醒哪些事项" button="选择…">
                {NOTIFY_KINDS
                    .iter()
                    .map(move |&(key, label)| {
                        view! {
                            <PrefRow label=label for_id=format!("notify-kind-{key}")>
                                <Switch
                                    id=format!("notify-kind-{key}")
                                    checked=Signal::derive(move || {
                                        !muted.get().iter().any(|s| s == key)
                                    })
                                    disabled=busy
                                    on_change=move |enabled: bool| {
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
                            </PrefRow>
                        }
                    })
                    .collect_view()}
            </PrefAdvanced>
        </PrefGroup>
    }
}

#[component]
fn AlertRuleSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/alert-rules"));
    view! {
        <PrefGroup title="提醒规则">
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
            }} <PrefAdvanced title="提醒规则">
                <p>
                    "hub 每 30 秒检查一次，事情发生时也会立刻检查。同一件事持续期间只提醒一次，消失后在中控台里标为已解决。"
                </p>
            </PrefAdvanced>
        </PrefGroup>
    }
}

#[component]
fn AlertRuleRow(rule: Value) -> impl IntoView {
    let key = text(&rule, "key");
    let unit = text(&rule, "unit");
    let label = text(&rule, "label");
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
    let number_label = label.clone();
    view! {
        <PrefRow label=label.clone() detail=text(&rule, "about") for_id=format!("alert-rule-{key}")>
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
                            aria-label=format!("「{number_label}」的阈值（{unit}）")
                            disabled=move || busy.get() || !on.get()
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
                                            .set_value(&old.map(|n| n.to_string()).unwrap_or_default());
                                        toast(format!("阈值要在 {min} 到 {max} 之间"));
                                    }
                                }
                            }
                        />
                        <span class="pref-unit">{unit.clone()}</span>
                    }
                })}
            <Switch
                id=format!("alert-rule-{switch_key}")
                label=format!("启用「{label}」")
                checked=on
                disabled=busy
                on_change=move |v: bool| {
                    on.set(v);
                    put(switch_key.clone(), json!({ "enabled": v }), Box::new(move || on.set(!v)));
                }
            />
        </PrefRow>
    }
}
