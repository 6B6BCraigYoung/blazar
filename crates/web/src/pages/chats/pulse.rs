use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api;
use crate::app_state::use_app;
use crate::components::icon;
use crate::fmt;
use crate::realtime::use_bus;

fn names(list: &[String]) -> String {
    match list {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} · {b}"),
        [a, b, rest @ ..] => format!("{a} · {b} · 还有 {}", rest.len()),
    }
}

#[component]
pub fn Pulse(rail_mode: Callback<bool>) -> impl IntoView {
    let app = use_app();
    let bus = use_bus();
    let navigate = StoredValue::new_local(use_navigate());
    let go = move |href: &str| {
        let href = href.to_owned();
        navigate.with_value(|n| n(&href, Default::default()));
    };
    let running = Memo::new(move |_| {
        let mut list: Vec<String> = app
            .top_workspaces()
            .iter()
            .filter(|w| matches!(w.activity.as_str(), "running" | "starting"))
            .map(|w| fmt::ws_label(&w.name, &w.node))
            .collect();
        if app
            .assistant()
            .is_some_and(|w| matches!(w.activity.as_str(), "running" | "starting"))
        {
            list.insert(0, "对话".into());
        }
        list
    });
    let waiting = Memo::new(move |_| {
        let items = app.inbox_items();
        let count = |k: &str| {
            items
                .iter()
                .filter(|it| it.needs_you() && it.kind == k)
                .count()
        };
        let (a, q, d, f) = (
            count("approval"),
            count("question"),
            count("denied"),
            count("run_failed"),
        );
        let parts: Vec<String> = [("批准", a), ("提问", q), ("被拦", d)]
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(l, n)| format!("{l} {n}"))
            .collect();
        (a + q + d, parts.join(" · "), f)
    });
    let failed_sub = Memo::new(move |_| {
        let items = app.inbox_items();
        let handled = items
            .iter()
            .filter(|it| it.kind == "run_failed" && !it.needs_you())
            .count();
        if handled > 0 {
            format!("已处理 {handled}")
        } else {
            String::new()
        }
    });
    let machines = Memo::new(move |_| {
        let nodes = app.nodes();
        let usable: Vec<_> = nodes.iter().filter(|n| n.is_usable()).collect();
        let on = usable.iter().filter(|n| n.status == "online").count();
        (on, usable.len())
    });
    let quota = LocalResource::new(move || {
        bus.accounts.track();
        bus.resyncs.track();
        async move {
            let a = api::get::<api::Accounts>("/api/accounts").await.ok()?;
            a.accounts
                .iter()
                .filter(|a| a.usable())
                .flat_map(|a| a.windows.iter())
                .max_by(|x, y| x.utilization.total_cmp(&y.utilization))
                .map(|w| {
                    (
                        (w.utilization * 100.0).round().clamp(0.0, 100.0) as u32,
                        fmt::window_label(&w.name),
                    )
                })
        }
    });
    view! {
        <section class="pulse" aria-label="状态">
            <button type="button" class="tile" data-k="running" on:click=move |_| go("/workspaces")>
                <span class="tl">
                    <i class="dot" data-tone="run"></i>
                    "在跑"
                </span>
                <span class="tn">{move || running.with(Vec::len)}</span>
                <span class="ts">{move || running.with(|l| names(l))}</span>
            </button>
            <button
                type="button"
                class="tile"
                data-k="waiting"
                data-on=move || (waiting.get().0 > 0).to_string()
                on:click=move |_| rail_mode.run(false)
            >
                <span class="tl">
                    <i class="dot" data-tone="attention"></i>
                    "等你"
                </span>
                <span class="tn">{move || waiting.get().0}</span>
                <span class="ts">{move || waiting.get().1}</span>
            </button>
            <button
                type="button"
                class="tile"
                data-k="failed"
                data-on=move || (waiting.get().2 > 0).to_string()
                on:click=move |_| rail_mode.run(false)
            >
                <span class="tl">
                    <i class="dot" data-tone="failed"></i>
                    "失败"
                </span>
                <span class="tn">{move || waiting.get().2}</span>
                <span class="ts">{move || failed_sub.get()}</span>
            </button>
            <button type="button" class="tile" data-k="nodes" on:click=move |_| go("/nodes")>
                <span class="tl">
                    <span class="ic" inner_html=icon::svg(icon::NODES)></span>
                    "机器"
                </span>
                <span class="tn">
                    {move || machines.get().0}
                    <small>{move || format!(" / {}", machines.get().1)}</small>
                </span>
                <span class="ts">
                    {move || {
                        let (on, total) = machines.get();
                        if total > on { format!("{} 台离线", total - on) } else { "都在线".to_owned() }
                    }}
                </span>
            </button>
            <button type="button" class="tile" data-k="quota" on:click=move |_| go("/runtimes")>
                <span class="tl">
                    {move || match quota.get().flatten() {
                        Some((_, label)) => format!("额度 · {label}"),
                        None => "额度".to_owned(),
                    }}
                </span>
                <span class="tn">
                    {move || match quota.get().flatten() {
                        Some((pct, _)) => format!("{pct}%"),
                        None => "—".to_owned(),
                    }}
                </span>
                <span class="bar">
                    <b style=move || {
                        format!("width:{}%", quota.get().flatten().map_or(0, |q| q.0))
                    }></b>
                </span>
            </button>
        </section>
    }
}
