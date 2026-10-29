use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::app_state::use_app;
use crate::fmt;

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
    let seg = move |k: &'static str,
                    tone: &'static str,
                    label: &'static str,
                    n: Signal<usize>,
                    tip: Signal<String>,
                    on: Callback<()>| {
        view! {
            <button
                type="button"
                class="pulse-seg"
                data-k=k
                data-zero=move || (n.get() == 0).to_string()
                title=move || tip.get()
                on:click=move |_| on.run(())
            >
                <i class="dot" data-tone=tone></i>
                <b>{move || n.get()}</b>
                {label}
            </button>
        }
    };
    let to_list = Callback::new(move |()| go("/workspaces"));
    let to_rail = Callback::new(move |()| rail_mode.run(false));
    view! {
        <p class="pulse" aria-label="状态">
            {seg(
                "running",
                "run",
                "在跑",
                Signal::derive(move || running.with(Vec::len)),
                Signal::derive(move || running.with(|l| names(l))),
                to_list,
            )}
            <span class="pulse-sep" aria-hidden="true">
                "·"
            </span>
            {seg(
                "waiting",
                "attention",
                "等你",
                Signal::derive(move || waiting.get().0),
                Signal::derive(move || waiting.get().1),
                to_rail,
            )}
            <span class="pulse-sep" aria-hidden="true">
                "·"
            </span>
            {seg(
                "failed",
                "failed",
                "失败",
                Signal::derive(move || waiting.get().2),
                Signal::derive(move || failed_sub.get()),
                to_rail,
            )}
        </p>
    }
}
