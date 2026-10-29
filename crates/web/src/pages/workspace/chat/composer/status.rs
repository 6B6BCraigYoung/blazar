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
    // Before any conversation has run here, the changes were already in the folder; say so
    // rather than presenting them like the agent's work.
    let existing = Memo::new(move |_| chat.threads.with(Vec::is_empty));
    // The 更改 view already shows the same summary; while it's open the tray doesn't repeat it.
    let lay = use_context::<crate::pages::workspace::layout::LayoutState>();
    let diff_open = Memo::new(move |_| {
        lay.is_some_and(|l| {
            l.lay
                .with(|l| l.inspect && l.tab == crate::pages::workspace::layout::Tab::Changes)
        })
    });
    move || {
        let (n, added, removed) = changes.get();
        let n = if diff_open.get() { 0 } else { n };
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
                                    title="Todos"
                                    on:click=move |_| {
                                        manual.set(Some(!open.get_untracked()));
                                    }
                                >
                                    <span class="ck" aria-hidden="true">
                                        {if all { "☒" } else { "☐" }}
                                    </span>
                                    <b>{format!("Todos {done}/{}", list.len())}</b>
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
                        })} <span class="grow"></span>
                    {(n > 0)
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="cb-review"
                                    title="Review changes in the inspector"
                                    on:click=move |_| chat.show_diff.run(())
                                >
                                    <span class="n">
                                        {format!(
                                            "{n} file{}{}",
                                            if n == 1 { "" } else { "s" },
                                            if existing.get() { " already changed" } else { "" },
                                        )}
                                    </span>
                                    {(!existing.get())
                                        .then(|| {
                                            view! {
                                                <span class="ok">{format!("+{added}")}</span>
                                                <span class="bad">{format!("−{removed}")}</span>
                                            }
                                        })}
                                    <span class="sep">"·"</span>
                                    <span class="go">"Review"</span>
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

/// 本机一个智能体都没装时替换输入框：一句说明加「Install」，装好后重新读一遍可用的智能体。
#[component]
pub(super) fn NoAgent(chat: Chat) -> impl IntoView {
    let installing = RwSignal::new(None::<&'static str>);
    let failed = RwSignal::new(None::<(&'static str, Option<String>)>);
    let install = move |id: &'static str| {
        if installing.get_untracked().is_some() {
            return;
        }
        installing.set(Some(id));
        failed.set(None);
        chat.spawn(async move {
            let r = crate::pages::runtimes::install_here(id).await;
            let _ = installing.try_set(None);
            match r {
                Ok(_) => chat.load_catalogs(),
                Err(e) => {
                    let _ = failed.try_set(Some((id, e.detail.or(Some(e.message)))));
                }
            }
        });
    };
    let name = |id: &str| {
        if id == "codex" {
            "Codex"
        } else {
            "Claude Code"
        }
    };
    view! {
        <div class="cc-noagent" role="status">
            <div class="cf-head">
                {move || match (installing.get(), failed.get()) {
                    (Some(id), _) => {
                        format!("Installing {}… this can take a few minutes.", name(id))
                    }
                    (None, Some((id, _))) => {
                        format!("Couldn't install {}. Check the network and try again.", name(id))
                    }
                    (None, None) => {
                        "Claude Code and Codex aren't installed on this machine yet.".to_owned()
                    }
                }}
            </div>
            {move || {
                failed
                    .get()
                    .and_then(|(_, d)| d)
                    .map(|d| {
                        view! {
                            <details class="ui-detail">
                                <summary>"Error details"</summary>
                                <pre>{d}</pre>
                            </details>
                        }
                    })
            }}
            <div class="cf-acts">
                <button
                    type="button"
                    class="linkbtn"
                    disabled=move || installing.get().is_some()
                    on:click=move |_| install("codex")
                >
                    "Install Codex instead"
                </button>
                <span class="grow"></span>
                <button
                    type="button"
                    class="btn primary"
                    disabled=move || installing.get().is_some()
                    on:click=move |_| install("claude")
                >
                    "Install Claude Code"
                </button>
            </div>
        </div>
    }
}
