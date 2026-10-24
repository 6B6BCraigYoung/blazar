use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::chat_model::Todo;
use crate::storage;

use super::super::log::todo_list;
use super::super::state::Chat;

pub(super) const TRAY_MIN_LOG: f64 = 360.0;
pub(super) const TRAY_LIST: f64 = 66.0;

#[component]
pub(super) fn Tray(chat: Chat, log_h: RwSignal<f64>) -> impl IntoView {
    let manual = RwSignal::new(None::<bool>);
    Effect::new(move |_| {
        chat.running.track();
        manual.set(None);
    });
    let todos = Memo::new(move |_| {
        let list: Vec<Todo> = chat
            .transcript
            .with(|t| t.todos.clone())
            .unwrap_or_default();
        let done = list.iter().filter(|t| t.status == "completed").count();
        (!list.is_empty() && (done < list.len() || chat.running.get())).then_some((list, done))
    });
    let changes = Memo::new(move |_| {
        chat.diff.files.with(|f| {
            (
                f.len(),
                f.iter().map(|x| x.added).sum::<u32>(),
                f.iter().map(|x| x.removed).sum::<u32>(),
            )
        })
    });
    let shown = Memo::new(move |prev: Option<&(bool, bool)>| {
        let was_open = prev.is_some_and(|p| p.1);
        let room = log_h.get() + if was_open { TRAY_LIST } else { 0.0 };
        let fits = room >= TRAY_MIN_LOG + TRAY_LIST;
        let want =
            todos.with(Option::is_some) && manual.get().unwrap_or_else(|| chat.running.get());
        (fits, fits && want)
    });
    let fits = Memo::new(move |_| shown.get().0);
    let open = Memo::new(move |_| shown.get().1);
    move || {
        let (n, added, removed) = changes.get();
        let todos = todos.get();
        if todos.is_none() && n == 0 {
            return None;
        }
        let list = todos.as_ref().map(|(l, _)| l.clone()).unwrap_or_default();
        Some(view! {
            <div class="cb-tray" data-open=move || open.get().to_string()>
                <div class="cb-todos" hidden=move || !open.get()>
                    {todo_list(&list)}
                </div>
                <div class="cb-tray-row">
                    {todos
                        .map(|(list, done)| {
                            let cur = list
                                .iter()
                                .find(|t| t.status == "in_progress")
                                .map(|t| t.content.clone());
                            let all = done == list.len();
                            view! {
                                <button
                                    type="button"
                                    class="cb-todo"
                                    aria-expanded=move || open.get().to_string()
                                    disabled=move || !fits.get()
                                    title="待办"
                                    on:click=move |_| {
                                        manual.set(Some(!open.get_untracked()));
                                    }
                                >
                                    <span class="ck" aria-hidden="true">
                                        {if all { "☒" } else { "☐" }}
                                    </span>
                                    <b>{format!("待办 {done}/{}", list.len())}</b>
                                    {cur
                                        .map(|c| {
                                            view! {
                                                <span class="sep">"·"</span>
                                                <span class="cur">{c}</span>
                                            }
                                        })}
                                    <span class="chev" aria-hidden="true"></span>
                                </button>
                            }
                        })}
                    <span class="grow"></span>
                    {(n > 0)
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="cb-review"
                                    title="在检查器里审阅更改"
                                    on:click=move |_| chat.show_diff.run(())
                                >
                                    <span class="n">{format!("{n} 个文件")}</span>
                                    <span class="ok">{format!("+{added}")}</span>
                                    <span class="bad">{format!("−{removed}")}</span>
                                    <span class="sep">"·"</span>
                                    <span class="go">"审阅"</span>
                                </button>
                            }
                        })}
                </div>
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
        let short = crate::fmt::window_label_en(&hot.name);
        let reset = hot
            .resets_at
            .map(|r| crate::fmt::resets_en(Some(&r.to_rfc3339())))
            .filter(|r| !r.is_empty())
            .map(|r| format!(" · {r}"))
            .unwrap_or_default();
        Some(view! {
            <div class="cb-band rate">
                <span>
                    {format!("{short} limit {}% used{reset}", (hot.utilization * 100.0).round())}
                </span>
                <span class="grow"></span>
                <button
                    class="x"
                    aria-label="Dismiss"
                    on:click=move |_| {
                        storage::save_raw("blazar.rate.off", &key);
                        off.set(key.clone());
                    }
                >
                    "×"
                </button>
            </div>
        })
    }
}
