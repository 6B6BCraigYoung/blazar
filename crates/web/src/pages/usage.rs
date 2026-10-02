//! 用量和运行分析；图表与表格共用 hub 统计口径。
use leptos::prelude::*;
use serde_json::{Value, json};

use crate::{api, storage};

fn n(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or_default()
}
fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}
fn list(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}

#[component]
pub fn UsagePage() -> impl IntoView {
    let days = RwSignal::new(
        storage::load_raw("blazar.an.days")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| [7, 30, 90].contains(n))
            .unwrap_or(30),
    );
    let refresh = RwSignal::new(0u32);
    let usage = LocalResource::new(move || {
        refresh.get();
        async { api::get::<Value>("/api/usage").await }
    });
    let analytics = LocalResource::new(move || {
        refresh.get();
        let d = days.get();
        let tz = -js_sys::Date::new_0().get_timezone_offset();
        async move { api::get::<Value>(&format!("/api/analytics?days={d}&tz={tz}")).await }
    });
    view! {
        <div class="page usage-page">
            <div class="page-head"><h1>"用量"</h1><span class="grow"></span><button class="btn" on:click=move |_| refresh.update(|n| *n += 1)>"刷新"</button></div>
            <div class="settings-actions"><h2>"运行分析"</h2>{[7, 30, 90].into_iter().map(move |d| view! {
                <button class="btn" class:primary=move || days.get() == d on:click=move |_| { storage::save_raw("blazar.an.days", &d.to_string()); days.set(d); }>{format!("{d} 天")}</button>
            }).collect_view()}</div>
            {move || match analytics.get() {
                None => view! { <div class="empty">"正在加载运行分析…"</div> }.into_any(),
                Some(Err(e)) => view! { <div class="err-line" role="alert">{e.to_string()}</div> }.into_any(),
                Some(Ok(a)) => view! { <Analytics data=a days=days.get()/> }.into_any(),
            }}
            <h2>"Token 与费用（累计）"</h2>
            {move || match usage.get() {
                None => view! { <div class="empty">"正在加载用量…"</div> }.into_any(),
                Some(Err(e)) => view! { <div class="err-line" role="alert">{e.to_string()}</div> }.into_any(),
                Some(Ok(u)) => {
                    let t = &u["total"];
                    let mut workspaces = list(&u, "by_workspace");
                    workspaces.sort_by(|a, b| n(&b["usage"], "cost_usd").total_cmp(&n(&a["usage"], "cost_usd")));
                    view! {
                        <div class="usage-kpis">{[("总费用", format!("${:.2}", n(t,"cost_usd"))), ("调用次数", format!("{:.0}",n(t,"calls"))), ("输入 token",format!("{:.0}",n(t,"input"))), ("输出 token",format!("{:.0}",n(t,"output"))), ("缓存读取",format!("{:.0}",n(t,"cache_read")))].into_iter().map(|(label,value)| view! { <div class="card usage-kpi"><span class="muted">{label}</span><strong>{value}</strong></div> }).collect_view()}</div>
                        <UsageTable title="按 Agent" data=u["by_agent"].clone()/>
                        <UsageTable title="按机器" data=u["by_node"].clone()/>
                        {(!workspaces.is_empty()).then(|| view! { <section class="card settings-card"><h3>"按工作区"</h3><div class="settings-table-wrap"><table class="settings-table"><thead><tr><th>"工作区"</th><th>"调用"</th><th>"输出"</th><th>"费用"</th></tr></thead><tbody>{workspaces.into_iter().take(30).map(|w| view! { <tr><td>{s(&w,"workspace")}</td><td>{format!("{:.0}",n(&w["usage"],"calls"))}</td><td>{format!("{:.0}",n(&w["usage"],"output"))}</td><td>{format!("${:.4}",n(&w["usage"],"cost_usd"))}</td></tr> }).collect_view()}</tbody></table></div></section> })}
                        {(n(t,"calls") == 0.0).then(|| view! { <div class="empty">"还没有用量数据，运行一次智能体会话后会在这里显示。"</div> })}
                    }.into_any()
                }
            }}
        </div>
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
        <section class="card settings-card"><h3>{title}</h3><div class="settings-table-wrap"><table class="settings-table"><thead><tr><th>"名称"</th><th>"调用"</th><th>"输入"</th><th>"输出"</th><th>"缓存读"</th><th>"费用"</th></tr></thead><tbody>{rows.into_iter().map(|(k,v)| view! {
            <tr><td>{k}</td>{["calls","input","output","cache_read"].into_iter().map(|k| view! { <td>{format!("{:.0}",n(&v,k))}</td> }).collect_view()}<td>{format!("${:.4}",n(&v,"cost_usd"))}</td></tr>
        }).collect_view()}</tbody></table></div></section>
    })
}

#[component]
fn Analytics(data: Value, days: u32) -> impl IntoView {
    let t = &data["totals"];
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
    let max_cost = series
        .iter()
        .map(|d| n(d, "cost"))
        .fold(0.0001_f64, f64::max);
    let first = series.first().map(|d| s(d, "day")).unwrap_or_default();
    let last = series.last().map(|d| s(d, "day")).unwrap_or_default();
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
    let board = list(&data, "board");
    let reasons = list(&data, "reasons");
    view! {
        <div class="usage-kpis">{[("运行次数", format!("{:.0}",n(t,"runs"))), ("成功率",t["success_rate"].as_f64().map(|v| format!("{:.0}%",v*100.0)).unwrap_or_else(|| "—".into())), ("失败",format!("{:.0}",n(t,"failed"))), ("费用",format!("${:.2}",n(t,"cost"))), ("日均",format!("{:.1} 次",n(t,"runs") / days as f64))].into_iter().map(|(label,value)| view! { <div class="card usage-kpi"><span class="muted">{label}</span><strong>{value}</strong></div> }).collect_view()}</div>
        <section class="card settings-card"><h3>"每天的运行"</h3><p class="muted">"柱：成功 / 失败 / 中断；细线：费用。悬停可查看每天的数值。"</p>
            <div class="usage-chart" role="img" aria-label="每天的运行次数与费用">{series.into_iter().map(|d| view! {
                <div class="usage-day" title=format!("{}：{:.0} 次；成功 {:.0}，失败 {:.0}，中断 {:.0}；${:.4}",s(&d,"day"),n(&d,"runs"),n(&d,"done"),n(&d,"failed"),n(&d,"interrupted"),n(&d,"cost"))>
                    <div class="usage-stack">{[("done","ok"),("failed","bad"),("interrupted","interrupted")].into_iter().map(|(k,c)| view! { <i class=c style:height=format!("{}%",n(&d,k)/max_runs*100.0)></i> }).collect_view()}</div>
                    <i class="usage-cost" style:height=format!("{}%",n(&d,"cost")/max_cost*100.0)></i>
                </div>
            }).collect_view()}</div><div class="settings-actions muted"><span>{first}</span><span class="grow"></span><span>{last}</span></div>
        </section>
        <section class="card settings-card"><h3>"谁在干活"</h3>{if board.is_empty() { view! { <p class="muted">"这段时间没有运行"</p> }.into_any() } else { view! { <div class="settings-table-wrap"><table class="settings-table"><thead><tr><th>"智能体 / 运行时"</th><th>"运行"</th><th>"成功率"</th><th>"平均耗时"</th><th>"费用"</th></tr></thead><tbody>{board.into_iter().map(|b| { let fin = n(&b,"done")+n(&b,"failed"); view! {
            <tr><td>{if let Some(id) = b["agent_id"].as_str() { view! { <a href=format!("/agent/{}",api::enc(id))>{format!("{} {}",s(&b,"avatar"),s(&b,"who"))}</a> }.into_any() } else { s(&b,"who").into_any() }}<span class="muted">{format!(" {}",s(&b,"runtime"))}</span></td><td>{format!("{:.0}",n(&b,"runs"))}</td><td>{if fin > 0.0 { format!("{:.0}%",n(&b,"done")/fin*100.0) } else { "—".into() }}</td><td>{b["avg_secs"].as_f64().map(|v| format!("{v:.0} 秒")).unwrap_or_else(|| "—".into())}</td><td>{format!("${:.2}",n(&b,"cost"))}</td></tr>
        } }).collect_view()}</tbody></table></div> }.into_any() }}</section>
        <section class="card settings-card"><h3>"为什么失败"</h3>{if reasons.is_empty() { view! { <p class="muted">"这段时间没有失败的运行"</p> }.into_any() } else { reasons.into_iter().map(|r| view! { <div class="settings-row"><b>{format!("{:.0} 次",n(&r,"n"))}</b><span>{s(&r,"why")}</span></div> }).collect_view().into_any() }}</section>
        <section class="card settings-card"><h3>"什么时候在跑"</h3><p class="muted">"星期 × 小时（本地时间）"</p><div class="usage-heat">{heat.into_iter().enumerate().map(|(w,row)| view! {
            <span>{format!("周{}",["日","一","二","三","四","五","六"][w])}</span>{row.into_iter().enumerate().map(|(h,count)| view! { <i title=format!("周{} {h:02}:00 · {count:.0} 次",["日","一","二","三","四","五","六"][w]) style:opacity=(0.12 + count/hmax*0.88).to_string()></i> }).collect_view()}
        }).collect_view()}<span></span>{(0..24).map(|h| view! { <small>{if h%6 == 0 { h.to_string() } else { String::new() }}</small> }).collect_view()}</div></section>
    }
}
