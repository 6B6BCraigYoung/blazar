use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::storage;

use super::super::log::todo_list;
use super::super::state::Chat;

#[component]
pub(super) fn Todos(chat: Chat) -> impl IntoView {
    let open = RwSignal::new(storage::load_raw("blazar.todosOpen").as_deref() != Some("0"));
    move || {
        let todos = chat
            .transcript
            .with(|t| t.todos.clone())
            .unwrap_or_default();
        let done = todos.iter().filter(|t| t.status == "completed").count();
        if todos.is_empty() || (done == todos.len() && !chat.running.get()) {
            return None;
        }
        let cur = todos
            .iter()
            .find(|t| t.status == "in_progress")
            .map(|t| t.content.clone());
        Some(view! {
            <div class="cc-todobar">
                <button class="td-h" on:click=move |_| { open.update(|o| *o = !*o); storage::save_raw("blazar.todosOpen", if open.get_untracked() { "1" } else { "0" }); }>
                    {move || if open.get() { "▾ " } else { "▸ " }}<b>{format!("任务 {done}/{}", todos.len())}</b>
                    {cur.map(|c| view! { <span class="td-cur">{c}</span> })}
                </button>
                {move || open.get().then(|| todo_list(&todos))}
            </div>
        })
    }
}

#[derive(Serialize, Deserialize, Default)]
struct SavedRate {
    #[serde(default)]
    windows: Vec<blazar_core_types::RateLimitWindow>,
}

#[component]
pub(super) fn RateBanner(chat: Chat) -> impl IntoView {
    let off = RwSignal::new(storage::load_raw("blazar.rate.off").unwrap_or_default());
    Effect::new(move |_| {
        let w = chat.transcript.with(|t| t.rate.clone());
        if !w.is_empty() {
            storage::save(
                "blazar.rate",
                &serde_json::json!({ "at": js_sys::Date::now(), "windows": w }),
            );
        }
    });
    move || {
        let mut ws = chat.transcript.with(|t| t.rate.clone());
        if ws.is_empty() {
            ws = storage::load::<SavedRate>("blazar.rate")
                .unwrap_or_default()
                .windows;
        }
        let now = js_sys::Date::now();
        let hot = ws
            .into_iter()
            .filter(|w| {
                w.utilization >= 0.8
                    && w.resets_at
                        .is_none_or(|r| (r.timestamp_millis() as f64) > now)
            })
            .max_by(|a, b| a.utilization.total_cmp(&b.utilization))?;
        let key = format!(
            "{}:{}",
            hot.name,
            hot.resets_at.map(|r| r.to_rfc3339()).unwrap_or_default()
        );
        if off.get() == key {
            return None;
        }
        let short = match hot.name.as_str() {
            "five_hour" => "5 小时",
            "seven_day" => "每周",
            "seven_day_opus" => "每周 Opus",
            "seven_day_sonnet" => "每周 Sonnet",
            n => n,
        }
        .to_owned();
        let reset = hot
            .resets_at
            .map(|r| crate::fmt::resets(Some(&r.to_rfc3339())))
            .filter(|r| !r.is_empty())
            .map(|r| format!(" · {r}"))
            .unwrap_or_default();
        Some(view! {
            <div class="cb-band rate">
                <span>{format!("{short}额度已用 {}%{reset}", (hot.utilization * 100.0).round())}</span>
                <span class="grow"></span>
                <button class="x" aria-label="关闭提示" on:click=move |_| { storage::save_raw("blazar.rate.off", &key); off.set(key.clone()); }>"×"</button>
            </div>
        })
    }
}
