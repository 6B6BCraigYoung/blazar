use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::Switch;
use crate::components::status::{InlineError, LoadingState, Status, Tone};
use crate::components::toast::{flash, toast, toast_error};
use crate::rt_logo;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct Choice {
    id: String,
    label: String,
    runtime: String,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    metered: bool,
    #[serde(default)]
    installed: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct Jev {
    #[serde(default)]
    key: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct Routing {
    #[serde(default)]
    enabled: bool,
    channels: Vec<String>,
    first: String,
    options: Vec<Choice>,
    jev: Jev,
}

const LEVELS: [&str; 5] = ["很简单", "简单", "中等", "较难", "很难"];

fn kind_zh(kind: &str) -> &'static str {
    match kind {
        "question" => "问答",
        "fix" => "修 bug",
        "feature" => "新功能",
        "refactor" => "重构",
        "docs" => "文档",
        "research" => "调研",
        "ops" => "跑命令",
        _ => "其他",
    }
}

fn tested(v: &Value) -> String {
    if v["ok"] != true {
        return v["error"].as_str().unwrap_or("测试失败").to_owned();
    }
    let d = &v["verdict"];
    let level = d["difficulty"]
        .as_f64()
        .map_or(2, |x| x.round().clamp(0.0, 4.0) as usize);
    let mut parts = vec![
        "通了".to_owned(),
        format!("{:.1} 秒", d["ms"].as_f64().unwrap_or(0.0) / 1000.0),
        format!(
            "示例任务判为「{} · {}」",
            LEVELS[level],
            kind_zh(d["kind"].as_str().unwrap_or_default())
        ),
    ];
    if let Some(c) = d["cost_usd"].as_f64() {
        parts.push(format!("本次约 ${c:.5}"));
    }
    parts.join(" · ")
}

fn save(channels: Vec<String>, first: String, rev: RwSignal<u32>) {
    spawn_local(async move {
        match api::send::<Value>(
            "PUT",
            "/api/routing",
            &json!({ "channels": channels, "first": first }),
        )
        .await
        {
            Ok(_) => flash("已保存"),
            Err(e) => toast_error("没保存上", &e),
        }
        let _ = rev.try_update(|n| *n += 1);
    });
}

fn rows(r: Routing, rev: RwSignal<u32>) -> impl IntoView {
    let r = StoredValue::new(r);
    let options = r.with_value(|r| r.options.clone());
    options
        .into_iter()
        .map(|o| {
            let on = r.with_value(|r| r.channels.contains(&o.id));
            let first = r.with_value(|r| r.first == o.id);
            let missing = o.installed == Some(false);
            let note = if missing {
                "未安装"
            } else if o.metered {
                "按量计费"
            } else {
                ""
            };
            let glyph = if o.account.is_some() {
                rt_logo::glyph("endpoint")
            } else {
                rt_logo::glyph(&o.runtime)
            };
            let id = o.id.clone();
            let toggle = move |e: leptos::ev::Event| {
                let want = event_target_checked(&e);
                let (mut picked, first) = r.with_value(|r| (r.channels.clone(), r.first.clone()));
                picked.retain(|c| c != &id);
                if want {
                    picked.push(id.clone());
                }
                let order: Vec<String> = r.with_value(|r| {
                    r.options
                        .iter()
                        .map(|o| o.id.clone())
                        .filter(|x| picked.contains(x))
                        .collect()
                });
                if order.is_empty() {
                    event_target::<web_sys::HtmlInputElement>(&e).set_checked(true);
                    toast("至少要留一个渠道");
                    return;
                }
                let first = if order.contains(&first) {
                    first
                } else {
                    order[0].clone()
                };
                save(order, first, rev);
            };
            let id2 = o.id.clone();
            let make_first = move |_| {
                let picked = r.with_value(|r| {
                    r.options
                        .iter()
                        .map(|o| o.id.clone())
                        .filter(|x| r.channels.contains(x))
                        .collect::<Vec<_>>()
                });
                save(picked, id2.clone(), rev);
            };
            let action = if first {
                view! { <span class="route-first">"首选"</span> }.into_any()
            } else if on {
                view! {
                    <button type="button" class="btn ghost small" on:click=make_first>
                        "设为首选"
                    </button>
                }
                .into_any()
            } else {
                ().into_any()
            };
            view! {
                <div class="route-row" data-off=(!on).then_some("")>
                    <input
                        type="checkbox"
                        class="switch"
                        aria-label=format!("Auto 可以用 {}", o.label)
                        prop:checked=on
                        disabled=missing && !on
                        on:change=toggle
                    />
                    <span class="rt-ic" inner_html=glyph></span>
                    <b>{o.label.clone()}</b>
                    {(!note.is_empty()).then(|| view! { <span class="route-note">{note}</span> })}
                    <span class="grow"></span>
                    {action}
                </div>
            }
        })
        .collect_view()
}

#[component]
pub(super) fn RoutingCard() -> impl IntoView {
    let rev = RwSignal::new(0u32);
    let data = LocalResource::new(move || {
        rev.track();
        api::get::<Routing>("/api/routing")
    });
    let key = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let result = RwSignal::new(None::<String>);
    let has_key = move || data.get().and_then(Result::ok).is_some_and(|r| r.jev.key);
    let save_key = move |_| {
        let k = key.get_untracked();
        if k.trim().is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>("PUT", "/api/routing/key", &json!({ "key": k })).await {
                Ok(_) => {
                    let _ = key.try_set(String::new());
                    let _ = result.try_set(None);
                    flash("key 已保存");
                }
                Err(e) => toast_error("key 没存上", &e),
            }
            let _ = busy.try_set(false);
            let _ = rev.try_update(|n| *n += 1);
        });
    };
    let remove_key = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>("DELETE", "/api/routing/key", &json!({})).await {
                Ok(_) => {
                    let _ = result.try_set(None);
                    flash("key 已移除");
                }
                Err(e) => toast_error("key 没移除", &e),
            }
            let _ = busy.try_set(false);
            let _ = rev.try_update(|n| *n += 1);
        });
    };
    let test = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        result.set(Some("正在请求 Jev…".into()));
        spawn_local(async move {
            let line = match api::send::<Value>("POST", "/api/routing/test", &json!({})).await {
                Ok(v) => tested(&v),
                Err(e) => format!("测试失败：{e}"),
            };
            let _ = result.try_set(Some(line));
            let _ = busy.try_set(false);
        });
    };

    let toggle_all = move |on: bool| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>("PUT", "/api/routing", &json!({ "enabled": on })).await {
                Ok(_) => flash(if on {
                    "智能路由已打开"
                } else {
                    "智能路由已关闭"
                }),
                Err(e) => toast_error("没保存上", &e),
            }
            let _ = busy.try_set(false);
            let _ = rev.try_update(|n| *n += 1);
        });
    };
    let off = move || data.get().and_then(Result::ok).is_some_and(|r| !r.enabled);

    view! {
        <section class="card routing-card" aria-label="智能路由">
            <div class="card-head">
                <span inner_html=rt_logo::mark("auto")></span>
                <b>"智能路由"</b>
                {move || {
                    data.get()
                        .and_then(Result::ok)
                        .map(|r| {
                            let label = if !r.enabled {
                                "已关闭"
                            } else if r.jev.key {
                                "Jev 挑型号"
                            } else {
                                "只按额度挑"
                            };
                            view! { <Status tone=Tone::Other label=label.to_owned() /> }
                        })
                }}
                <span class="grow"></span>
                {move || {
                    data.get()
                        .and_then(Result::ok)
                        .map(|r| {
                            view! {
                                <Switch
                                    label="智能路由"
                                    checked=r.enabled
                                    disabled=busy
                                    on_change=Callback::new(toggle_all)
                                />
                            }
                        })
                }}
            </div>
            <p class="route-lead">
                "打开后，新对话默认用 Auto：Jev 判断每条消息该用哪个型号，账号按剩余额度挑。对话中途只在同一个软件里换，要 Jev 很有把握或者账号到了额度线才换。在对话里手动选型号或用 /model 指定，就按你选的来，直到切回 Auto。"
            </p>
            <div class="route-body" data-off=move || off().then_some("")>
                {move || match data.get() {
                    None => view! { <LoadingState text="正在加载智能路由…" /> }.into_any(),
                    Some(Err(error)) => {
                        view! {
                            <InlineError
                                lead="无法加载智能路由"
                                error=error
                                retry=Callback::new(move |_| rev.update(|n| *n += 1))
                            />
                        }
                            .into_any()
                    }
                    Some(Ok(r)) => rows(r, rev).into_any(),
                }}
                <div class="route-jev">
                    <div class="route-jev-text">
                        <b>"难度判断（Jev）"</b>
                        <span>
                            "填上 TypeSafe 的 key 后，每条 Auto 消息会先发给 Jev，由它在能用的型号里挑一个：新对话发第一条消息的前 6000 字，之后再带上对话开头，约 1 秒、每次约 $0.0001。不填就只按额度挑软件，型号用你的默认。"
                        </span>
                    </div>
                    <div class="route-jev-act">
                        {move || {
                            if has_key() {
                                view! {
                                    <button
                                        type="button"
                                        class="btn small"
                                        disabled=move || busy.get()
                                        on:click=test
                                    >
                                        "测试"
                                    </button>
                                    <button
                                        type="button"
                                        class="btn ghost small"
                                        disabled=move || busy.get()
                                        on:click=remove_key
                                    >
                                        "移除 key"
                                    </button>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <input
                                        type="password"
                                        class="settings-input"
                                        placeholder="TypeSafe API key"
                                        autocomplete="off"
                                        aria-label="TypeSafe API key"
                                        prop:value=move || key.get()
                                        on:input=move |e| key.set(event_target_value(&e))
                                    />
                                    <button
                                        type="button"
                                        class="btn small"
                                        disabled=move || {
                                            busy.get() || key.with(|k| k.trim().is_empty())
                                        }
                                        on:click=save_key
                                    >
                                        "保存"
                                    </button>
                                }
                                    .into_any()
                            }
                        }}
                    </div>
                    <p class="route-test" aria-live="polite">
                        {move || {
                            result
                                .get()
                                .unwrap_or_else(|| {
                                    if has_key() {
                                        "key 已保存。点「测试」用一条示例任务试一次。"
                                            .to_owned()
                                    } else {
                                        "还没填 key，现在只按额度挑。".to_owned()
                                    }
                                })
                        }}
                    </p>
                </div>
            </div>
        </section>
    }
}
