use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::{PrefGroup, PrefRow, Sheet, Switch};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::{flash, toast};

const BOT_LIMIT_MAX: u64 = 16;

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().into()
}

async fn save(body: Value) -> bool {
    match api::send::<Value>("PUT", "/api/settings", &body).await {
        Ok(_) => {
            flash("已保存");
            true
        }
        Err(e) => {
            toast(format!("没保存上：{e}"));
            false
        }
    }
}

/// bot 页「高级…」：同时运行几个 bot、什么时候提醒（原先在设置的「对话与运行」和「提醒」里）。
#[component]
pub(super) fn BotAdvanced(on_close: Callback<()>) -> impl IntoView {
    view! {
        <Sheet
            title="bot 高级设置"
            class="dlg sheet wide"
            on_close
            foot=move || {
                view! {
                    <button
                        type="button"
                        class="btn primary"
                        data-modal-initial-focus=""
                        on:click=move |_| on_close.run(())
                    >
                        "完成"
                    </button>
                }
            }
        >
            <Concurrency />
            <AlertRules />
        </Sheet>
    }
}

#[component]
fn Concurrency() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let busy = RwSignal::new(false);
    move || match data.get() {
        None => view! { <LoadingState text="正在加载…" /> }.into_any(),
        Some(Err(e)) => view! {
            <InlineError
                message=format!("无法加载设置：{e}")
                retry=Callback::new(move |_| data.refetch())
            />
        }
        .into_any(),
        Some(Ok(p)) => {
            let limit = RwSignal::new(p["bots"]["max_concurrent"].as_u64().unwrap_or(6));
            view! {
                <PrefGroup title="同时运行">
                    <PrefRow
                        label="最多同时运行几个 bot"
                        detail="超过时新的邮件先排队，等有 bot 停下再派。取 1 到 16。"
                        for_id="bots-limit"
                    >
                        <input
                            id="bots-limit"
                            class="settings-input"
                            type="number"
                            min="1"
                            max=BOT_LIMIT_MAX
                            disabled=move || busy.get()
                            prop:value=move || limit.get().to_string()
                            on:change=move |e| {
                                let old = limit.get_untracked();
                                match event_target_value(&e).trim().parse::<u64>() {
                                    Ok(n) if (1..=BOT_LIMIT_MAX).contains(&n) => {
                                        if n == old {
                                            return;
                                        }
                                        limit.set(n);
                                        busy.set(true);
                                        leptos::task::spawn_local(async move {
                                            if !save(json!({"bots": {"max_concurrent": n}})).await {
                                                limit.set(old);
                                            }
                                            busy.set(false);
                                        });
                                    }
                                    _ => {
                                        event_target::<web_sys::HtmlInputElement>(&e)
                                            .set_value(&old.to_string());
                                        toast(format!("要在 1 到 {BOT_LIMIT_MAX} 之间"));
                                    }
                                }
                            }
                        />
                    </PrefRow>
                </PrefGroup>
            }
            .into_any()
        }
    }
}

#[component]
fn AlertRules() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/alert-rules"));
    view! {
        <PrefGroup title="什么时候提醒" aside="默认都开着，一般不用改">
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
                    v["rules"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|r| view! { <AlertRuleRow rule=r /> })
                        .collect_view()
                        .into_any()
                }
            }}
        </PrefGroup>
        <p class="muted">
            "情况一出现就会检查，之后每半分钟再看一次。同一件事只提醒一次，情况消失后在中控台里标为已解决。要关掉某一类提醒，去「设置 → 提醒」。"
        </p>
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
    let on = RwSignal::new(rule["enabled"].as_bool().unwrap_or(false));
    let threshold = RwSignal::new(rule["threshold"].as_i64());
    let busy = RwSignal::new(false);
    let put = move |key: String, body: Value, undo: Box<dyn FnOnce() + 'static>| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            if !save(json!({ "rules": { key: body } })).await {
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
                            aria-label=format!("「{number_label}」的时长（{unit}）")
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
                                        toast(format!("要在 {min} 到 {max} 之间"));
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
