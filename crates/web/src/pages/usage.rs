use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::empty::{self, Action, Blank};
use crate::components::icon;
use crate::components::page_head::PageHead;
use crate::components::rows::open_row;
use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::pages::runtimes::accounts;
use crate::{api, storage, vocab};

mod slots;
use slots::{Slot, slots};

fn n(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or_default()
}
fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}
fn list(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}
fn short_day(day: &str) -> String {
    let mut parts = day.split('-').skip(1).map(|p| p.trim_start_matches('0'));
    match (parts.next(), parts.next()) {
        (Some(m), Some(d)) => format!("{m}-{d}"),
        _ => day.to_owned(),
    }
}

#[component]
pub fn UsagePage() -> impl IntoView {
    let days = RwSignal::new(
        storage::load_raw("blazar.an.days")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| [7, 30, 90].contains(n))
            .unwrap_or(30),
    );
    let usage = LocalResource::new(move || async { api::get::<Value>("/api/usage").await });
    let analytics = LocalResource::new(move || {
        let d = days.get();
        let tz = -js_sys::Date::new_0().get_timezone_offset();
        async move { api::get::<Value>(&format!("/api/analytics?days={d}&tz={tz}")).await }
    });
    let never = move || {
        usage.with(|u| matches!(u, Some(Ok(u)) if n(&u["total"], "calls") == 0.0))
            && analytics.with(|a| matches!(a, Some(Ok(a)) if n(&a["totals"], "runs") == 0.0))
    };
    let widen = Callback::new(move |()| {
        storage::save_raw("blazar.an.days", "90");
        days.set(90);
    });
    let summary = move || {
        analytics.with(|a| match a {
            Some(Ok(a)) if n(&a["totals"], "runs") > 0.0 => {
                Some(summary_line(&a["totals"], days.get()))
            }
            _ => None,
        })
    };
    view! {
        <div class="page usage-page">
            <PageHead title="用量" nav=|| crate::components::section::agents("/usage") />
            <QuotaBlock />
            <Show
                when=move || !never()
                fallback=|| {
                    view! {
                        <Blank
                            icon=icon::CHART
                            title=empty::never("用量")
                            detail="智能体运行后，这里会统计次数、耗时、Token 和费用。"
                        />
                    }
                }
            >
                <div
                    class="settings-actions usage-period"
                    role="group"
                    aria-label="统计时间范围"
                >
                    <h2>{move || format!("近 {} 天", days.get())}</h2>
                    <div class="seg">
                        {[7, 30, 90]
                            .into_iter()
                            .map(move |d| {
                                view! {
                                    <button
                                        type="button"
                                        aria-pressed=move || days.get() == d
                                        data-on=move || (days.get() == d).to_string()
                                        on:click=move |_| {
                                            storage::save_raw("blazar.an.days", &d.to_string());
                                            days.set(d);
                                        }
                                    >
                                        {format!("{d} 天")}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                    {move || summary().map(|line| view! { <p class="usage-summary">{line}</p> })}
                </div>
                {move || match analytics.get() {
                    None => view! { <LoadingState text="正在加载运行分析…" /> }.into_any(),
                    Some(Err(e)) => {
                        view! {
                            <InlineError
                                lead="无法加载运行分析"
                                error=e.clone()
                                retry=Callback::new(move |_| analytics.refetch())
                            />
                        }
                            .into_any()
                    }
                    Some(Ok(a)) => view! { <Analytics data=a days=days.get() widen /> }.into_any(),
                }}
                <details class="usage-details">
                    <summary>"累计 Token 与费用"</summary>
                    {move || match usage.get() {
                        None => {
                            view! { <LoadingState text="正在加载累计用量…" /> }.into_any()
                        }
                        Some(Err(e)) => {
                            view! {
                                <InlineError
                                    lead="无法加载累计用量"
                                    error=e.clone()
                                    retry=Callback::new(move |_| usage.refetch())
                                />
                            }
                                .into_any()
                        }
                        Some(Ok(u)) => {
                            let t = &u["total"];
                            if n(t, "calls") == 0.0 {
                                return view! {
                                    <EmptyState
                                        title="暂无累计用量"
                                        detail="运行智能体后，可在这里查看 Token 与费用。"
                                    />
                                }
                                    .into_any();
                            }
                            let mut workspaces = list(&u, "by_workspace");
                            workspaces
                                .sort_by(|a, b| {
                                    n(&b["usage"], "cost_usd")
                                        .total_cmp(&n(&a["usage"], "cost_usd"))
                                });
                            view! {
                                <p class="usage-summary">
                                    {format!(
                                        "{:.0} 次调用 · ${:.2} · 输入 {:.0} · 输出 {:.0} · 缓存读取 {:.0} Token",
                                        n(t, "calls"),
                                        n(t, "cost_usd"),
                                        n(t, "input"),
                                        n(t, "output"),
                                        n(t, "cache_read"),
                                    )}
                                </p>
                                <UsageTable
                                    title="按 Claude Code / Codex"
                                    data=u["by_agent"].clone()
                                />
                                <UsageTable title="按机器" data=u["by_node"].clone() />
                                {(!workspaces.is_empty())
                                    .then(|| {
                                        view! {
                                            <section class="card settings-card">
                                                <h3>"按工作区"</h3>
                                                <div class="settings-table-wrap">
                                                    <table class="settings-table">
                                                        <thead>
                                                            <tr>
                                                                <th>"工作区"</th>
                                                                <th class="num">"调用"</th>
                                                                <th class="num">"输出"</th>
                                                                <th class="num">"费用"</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {workspaces
                                                                .into_iter()
                                                                .take(30)
                                                                .map(|w| {
                                                                    view! {
                                                                        <tr>
                                                                            <td>{s(&w, "workspace")}</td>
                                                                            <td class="num">
                                                                                {format!("{:.0}", n(&w["usage"], "calls"))}
                                                                            </td>
                                                                            <td class="num">
                                                                                {format!("{:.0}", n(&w["usage"], "output"))}
                                                                            </td>
                                                                            <td class="num">
                                                                                {format!("${:.4}", n(&w["usage"], "cost_usd"))}
                                                                            </td>
                                                                        </tr>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </tbody>
                                                    </table>
                                                </div>
                                            </section>
                                        }
                                    })}
                            }
                                .into_any()
                        }
                    }}
                </details>
            </Show>
        </div>
    }
}

/// 统计范围的一行灰字：「40 次 · 76% 成功 · 9 次失败 · $27.97 · 日均 1.3 次」。
fn summary_line(t: &Value, days: u32) -> String {
    let runs = n(t, "runs");
    let failed = n(t, "failed");
    let mut parts = vec![format!("{runs:.0} 次")];
    if let Some(rate) = t["success_rate"].as_f64() {
        parts.push(format!("{:.0}% 成功", rate * 100.0));
    }
    if failed > 0.0 {
        parts.push(format!("{failed:.0} 次失败"));
    }
    parts.push(format!("${:.2}", n(t, "cost")));
    parts.push(format!("日均 {:.1} 次", runs / f64::from(days.max(1))));
    parts.join(" · ")
}

/// 页顶的各账号额度条，和「账号与额度」页同一个组件；没有账号查到过额度就不出这一块。
#[component]
fn QuotaBlock() -> impl IntoView {
    let res = LocalResource::new(|| api::get::<api::Accounts>("/api/accounts"));
    move || {
        let mut list = res
            .get()?
            .ok()?
            .accounts
            .into_iter()
            .filter(|a| !a.disabled && !a.windows.is_empty())
            .collect::<Vec<_>>();
        if list.is_empty() {
            return None;
        }
        list.sort_by(|a, b| a.provider.cmp(&b.provider));
        Some(view! {
            <section class="card settings-card usage-quota">
                <div class="usage-chart-head">
                    <h3>"额度"</h3>
                    <span class="grow"></span>
                    <a class="muted small" href="/runtimes">
                        "管理账号"
                    </a>
                </div>
                {list
                    .into_iter()
                    .map(|a| {
                        let provider = accounts::provider_label(&a.provider).to_owned();
                        let meta = [
                            (provider != a.label).then_some(provider),
                            a.plan.as_deref().map(|p| accounts::plan_label(p).to_owned()),
                        ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · ");
                        view! {
                            <div class="usage-quota-row">
                                <span class="name">
                                    {a.label.clone()}
                                    {(!meta.is_empty())
                                        .then(|| view! { <span class="muted small">{meta}</span> })}
                                </span>
                                {accounts::quota(&a)}
                            </div>
                        }
                    })
                    .collect_view()}
            </section>
        })
    }
}

#[component]
fn UsageTable(title: &'static str, data: Value) -> impl IntoView {
    let mut rows = data
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    rows.sort_by(|a, b| n(&b.1, "cost_usd").total_cmp(&n(&a.1, "cost_usd")));
    (!rows.is_empty()).then(|| view! {
        <section class="card settings-card">
            <h3>{title}</h3>
            <div class="settings-table-wrap">
                <table class="settings-table">
                    <thead>
                        <tr>
                            <th>"名称"</th>
                            <th class="num">"调用"</th>
                            <th class="num">"输入"</th>
                            <th class="num">"输出"</th>
                            <th class="num">"缓存读"</th>
                            <th class="num">"费用"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {rows
                            .into_iter()
                            .map(|(k, v)| {
                                view! {
                                    <tr>
                                        <td>{vocab::term(&k).to_owned()}</td>
                                        {["calls", "input", "output", "cache_read"]
                                            .into_iter()
                                            .map(|k| {
                                                view! { <td class="num">{format!("{:.0}", n(&v, k))}</td> }
                                            })
                                            .collect_view()}
                                        <td class="num">{format!("${:.4}", n(&v, "cost_usd"))}</td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
            </div>
        </section>
    })
}

#[component]
fn Analytics(data: Value, days: u32, widen: Callback<()>) -> impl IntoView {
    let t = &data["totals"];
    if n(t, "runs") == 0.0 {
        return if days < 90 {
            view! {
                <Blank
                    icon=icon::FILTER
                    title=empty::unmatched("运行")
                    detail=format!("近 {days} 天没有运行，放宽时间范围看看。")
                    action=Action::clear(move || widen.run(()))
                    inset=true
                />
            }
            .into_any()
        } else {
            view! {
                <Blank
                    icon=icon::CHART
                    title=empty::never("运行")
                    detail="近 90 天没有运行。开始一次智能体对话后，这里会有统计。"
                    inset=true
                />
            }
            .into_any()
        };
    }
    let daily = list(&data, "daily");
    let now = js_sys::Date::new_0();
    let series = (0..days)
        .rev()
        .map(|i| {
            let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
                now.get_time() - i as f64 * 86_400_000.0,
            ));
            let key = format!(
                "{:04}-{:02}-{:02}",
                d.get_full_year(),
                d.get_month() + 1,
                d.get_date()
            );
            daily
                .iter()
                .find(|d| s(d, "day") == key)
                .cloned()
                .unwrap_or_else(|| json!({"day":key}))
        })
        .collect::<Vec<_>>();
    let max_runs = series.iter().map(|d| n(d, "runs")).fold(1.0_f64, f64::max);
    let slots = slots(&series.iter().map(|d| n(d, "runs")).collect::<Vec<_>>());
    let label = |slot: Slot| match slot {
        Slot::Day(i) => {
            let d = &series[i];
            let failed = n(d, "failed");
            let failed = if failed > 0.0 {
                format!(" · {failed:.0} 次失败")
            } else {
                String::new()
            };
            format!(
                "{} · {:.0} 次{failed} · ${:.2}",
                s(d, "day"),
                n(d, "runs"),
                n(d, "cost")
            )
        }
        Slot::Idle(a, b) => format!(
            "{} 至 {} 无运行",
            short_day(&s(&series[a], "day")),
            short_day(&s(&series[b], "day"))
        ),
    };
    let labels = slots.iter().map(|slot| label(*slot)).collect::<Vec<_>>();
    let first = match slots.first() {
        Some(Slot::Day(i) | Slot::Idle(i, _)) => s(&series[*i], "day"),
        None => String::new(),
    };
    let last = series.last().map(|d| s(d, "day")).unwrap_or_default();
    let count = slots.len();
    let hover = RwSignal::new(None::<usize>);
    let keyed = RwSignal::new(None::<usize>);
    let shown = move || {
        hover
            .get()
            .or(keyed.get())
            .unwrap_or(count.saturating_sub(1))
            .min(count.saturating_sub(1))
    };
    let by_slot = StoredValue::new(labels.clone());
    let readout = move || by_slot.with_value(|all| all.get(shown()).cloned().unwrap_or_default());
    let step = move |delta: i64| {
        let end = count.saturating_sub(1) as i64;
        let next = (shown() as i64 + delta).clamp(0, end) as usize;
        keyed.set(Some(next));
        hover.set(None);
    };
    let bars = slots
        .into_iter()
        .zip(labels)
        .map(|(slot, title)| match slot {
            Slot::Day(i) => (
                Some((n(&series[i], "runs"), n(&series[i], "failed"))),
                title,
            ),
            Slot::Idle(..) => (None, title),
        })
        .collect::<Vec<_>>();
    let daily_table = series;
    let mut heat = [[0.0_f64; 24]; 7];
    for h in list(&data, "heat") {
        if let (Some(w), Some(hour), Some(count)) = (h[0].as_u64(), h[1].as_u64(), h[2].as_f64())
            && w < 7
            && hour < 24
        {
            heat[w as usize][hour as usize] = count;
        }
    }
    let hmax = heat.iter().flatten().copied().fold(1.0_f64, f64::max);
    let heat_table = heat
        .into_iter()
        .enumerate()
        .flat_map(|(day, row)| {
            row.into_iter()
                .enumerate()
                .filter(|(_, count)| *count > 0.0)
                .map(move |(hour, count)| (day, hour, count))
        })
        .collect::<Vec<_>>();
    let board = list(&data, "board");
    let reasons = crate::vocab::failure_reasons(
        &list(&data, "reasons")
            .iter()
            .map(|r| (n(r, "n"), s(r, "why")))
            .collect::<Vec<_>>(),
    );
    let has_reasons = !reasons.is_empty();
    view! {
        <section class="card settings-card usage-trend">
            <div class="usage-chart-head">
                <h3>"运行趋势"</h3>
                <span class="grow"></span>
                <div class="usage-readout" role="status">
                    {readout}
                </div>
            </div>
            <div
                class="usage-chart"
                role="group"
                tabindex="0"
                aria-label="每天的运行次数与费用，用左右方向键选日期"
                on:pointerleave=move |_| hover.set(None)
                on:keydown=move |e: leptos::ev::KeyboardEvent| {
                    let delta = match e.key().as_str() {
                        "ArrowLeft" => -1,
                        "ArrowRight" => 1,
                        "Home" => -(count as i64),
                        "End" => count as i64,
                        _ => return,
                    };
                    e.prevent_default();
                    step(delta);
                }
            >
                <div class="usage-days">
                    {bars
                        .into_iter()
                        .enumerate()
                        .map(|(i, (day, title))| {
                            let sel = move || (shown() == i).to_string();
                            let enter = move |_: leptos::ev::PointerEvent| hover.set(Some(i));
                            match day {
                                Some((runs, failed)) => {
                                    view! {
                                        <div
                                            class="usage-day"
                                            title=title
                                            data-sel=sel
                                            on:pointerenter=enter
                                        >
                                            <div class="usage-stack">
                                                <i
                                                    class="run"
                                                    style:height=format!("{}%", runs / max_runs * 100.0)
                                                ></i>
                                                {(failed > 0.0).then(|| view! { <i class="bad"></i> })}
                                            </div>
                                        </div>
                                    }
                                        .into_any()
                                }
                                None => {
                                    view! {
                                        <div
                                            class="usage-idle"
                                            title=title
                                            data-sel=sel
                                            on:pointerenter=enter
                                        ></div>
                                    }
                                        .into_any()
                                }
                            }
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="settings-actions muted usage-axis">
                <span>{first}</span>
                <span class="grow"></span>
                <span class="usage-legend" aria-hidden="true">
                    <span>
                        <i class="run"></i>
                        "运行"
                    </span>
                    <span>
                        <i class="bad"></i>
                        "失败"
                    </span>
                </span>
                <span class="grow"></span>
                <span>{last}</span>
            </div>
            <details class="usage-details">
                <summary>"查看每日明细"</summary>
                <div class="settings-table-wrap">
                    <table class="settings-table">
                        <thead>
                            <tr>
                                <th>"日期"</th>
                                <th class="num">"运行"</th>
                                <th class="num">"成功"</th>
                                <th class="num">"失败"</th>
                                <th class="num">"中断"</th>
                                <th class="num">"费用"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {daily_table
                                .into_iter()
                                .map(|d| {
                                    view! {
                                        <tr>
                                            <td>{s(&d, "day")}</td>
                                            {["runs", "done", "failed", "interrupted"]
                                                .into_iter()
                                                .map(|key| {
                                                    view! {
                                                        <td class="num">{format!("{:.0}", n(&d, key))}</td>
                                                    }
                                                })
                                                .collect_view()}
                                            <td class="num">{format!("${:.4}", n(&d, "cost"))}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            </details>
        </section>
        <section class="card settings-card">
            <h3>"运行分布"</h3>
            {if board.is_empty() {
                view! { <p class="muted">"这段时间没有运行"</p> }.into_any()
            } else {
                view! {
                    <div class="settings-table-wrap">
                        <table class="settings-table">
                            <thead>
                                <tr>
                                    <th>"智能体"</th>
                                    <th class="num">"运行"</th>
                                    <th class="num">"成功率"</th>
                                    <th class="num">"平均耗时"</th>
                                    <th class="num">"费用"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {board
                                    .into_iter()
                                    .map(|b| {
                                        let fin = n(&b, "done") + n(&b, "failed");
                                        let runtime = vocab::term(&s(&b, "runtime")).to_owned();
                                        view! {
                                            <tr
                                                class="link-row"
                                                on:click=move |e: leptos::ev::MouseEvent| open_row(
                                                    &e,
                                                    "a.cell-link",
                                                )
                                            >
                                                <td>
                                                    {match b["agent_id"].as_str() {
                                                        Some(id) => {
                                                            view! {
                                                                <a
                                                                    class="cell-link"
                                                                    href=format!("/agent/{}", api::enc(id))
                                                                >
                                                                    {s(&b, "who")}
                                                                </a>
                                                                <span class="muted">{format!(" {runtime}")}</span>
                                                            }
                                                                .into_any()
                                                        }
                                                        None => runtime.into_any(),
                                                    }}
                                                </td>
                                                <td class="num">{format!("{:.0}", n(&b, "runs"))}</td>
                                                <td class="num">
                                                    {if fin > 0.0 {
                                                        format!("{:.0}%", n(&b, "done") / fin * 100.0)
                                                    } else {
                                                        "—".into()
                                                    }}
                                                </td>
                                                <td class="num">
                                                    {b["avg_secs"]
                                                        .as_f64()
                                                        .map(|v| format!("{v:.0} 秒"))
                                                        .unwrap_or_else(|| "—".into())}
                                                </td>
                                                <td class="num">{format!("${:.2}", n(&b, "cost"))}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                    .into_any()
            }}
        </section>
        <details class="card settings-card usage-details" open=has_reasons>
            <summary>"失败原因"</summary>
            {if reasons.is_empty() {
                view! { <p class="muted">"这段时间没有失败的运行"</p> }.into_any()
            } else {
                reasons
                    .into_iter()
                    .map(|(count, why, originals)| {
                        view! {
                            <div class="settings-row">
                                <b>{format!("{count:.0} 次")}</b>
                                <span>
                                    {why}
                                    {(!originals.is_empty())
                                        .then(|| {
                                            view! {
                                                <details class="ui-detail">
                                                    <summary>"详情"</summary>
                                                    <pre>{originals.join("\n")}</pre>
                                                </details>
                                            }
                                        })}
                                </span>
                            </div>
                        }
                    })
                    .collect_view()
                    .into_any()
            }}
        </details>
        <details class="card settings-card usage-details">
            <summary>"活跃时段"</summary>
            <p class="muted">"星期 × 小时（本地时间）"</p>
            <div
                class="usage-heat"
                role="img"
                aria-label="按星期和小时统计的运行次数，数值见下方时段明细"
            >
                {heat
                    .into_iter()
                    .enumerate()
                    .map(|(w, row)| {
                        view! {
                            <span>
                                {format!(
                                    "周{}",
                                    ["日", "一", "二", "三", "四", "五", "六"][w],
                                )}
                            </span>
                            {row
                                .into_iter()
                                .enumerate()
                                .map(|(h, count)| {
                                    view! {
                                        <i
                                            title=format!(
                                                "周{} {h:02}:00 · {count:.0} 次",
                                                ["日", "一", "二", "三", "四", "五", "六"][w],
                                            )
                                            style:opacity=(0.12 + count / hmax * 0.88).to_string()
                                        ></i>
                                    }
                                })
                                .collect_view()}
                        }
                    })
                    .collect_view()}
                <span></span>
                {(0..24)
                    .map(|h| {
                        view! {
                            <small>{if h % 6 == 0 { h.to_string() } else { String::new() }}</small>
                        }
                    })
                    .collect_view()}
            </div>
            <details class="usage-details">
                <summary>"查看时段明细"</summary>
                <p class="muted">"未列出的时段为 0 次运行。"</p>
                <div class="settings-table-wrap">
                    <table class="settings-table">
                        <thead>
                            <tr>
                                <th>"星期"</th>
                                <th>"时间"</th>
                                <th class="num">"运行"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {heat_table
                                .into_iter()
                                .map(|(day, hour, count)| {
                                    view! {
                                        <tr>
                                            <td>
                                                {format!(
                                                    "周{}",
                                                    ["日", "一", "二", "三", "四", "五", "六"][day],
                                                )}
                                            </td>
                                            <td>{format!("{hour:02}:00")}</td>
                                            <td class="num">{format!("{count:.0}")}</td>
                                        </tr>
                                    }
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            </details>
        </details>
    }.into_any()
}
