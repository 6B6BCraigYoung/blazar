use leptos::prelude::*;

use super::super::state::Chat;
use super::Pop;

const LOW_AT: f64 = 0.8;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Usage {
    pub used: Option<u64>,
    pub window: u64,
    pub cost: f64,
}

impl Usage {
    fn ratio(self) -> Option<f64> {
        let used = self.used?;
        (self.window > 0).then(|| used as f64 / self.window as f64)
    }

    pub fn low(self) -> bool {
        self.ratio().is_some_and(|r| r >= LOW_AT)
    }

    pub fn label(self) -> String {
        match (self.used, self.ratio()) {
            (_, Some(r)) if r >= LOW_AT => format!("Context low · {}%", percent(r)),
            (_, Some(r)) => format!("{}% context", percent(r)),
            (Some(used), None) => format!("{} context", short(used)),
            (None, None) => "Context".to_owned(),
        }
    }

    pub fn cost_label(self) -> String {
        if self.cost < 0.005 {
            "<$0.01".to_owned()
        } else {
            format!("${:.2}", self.cost)
        }
    }

    pub fn tip(self) -> String {
        let mut out = match (self.used, self.window) {
            (Some(used), w) if w > 0 => {
                format!("{} of {} tokens in context", short(used), short(w))
            }
            (Some(used), _) => format!("{} tokens in context", short(used)),
            (None, _) => "Context size shows after the next reply".to_owned(),
        };
        if self.cost > 0.0 {
            out.push_str(&format!(
                "\n{} spent in this conversation",
                self.cost_label()
            ));
        }
        out
    }
}

fn percent(r: f64) -> u64 {
    (r * 100.0).round().clamp(0.0, 100.0) as u64
}

fn short(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => {
            let m = n as f64 / 1_000_000.0;
            if (m - m.round()).abs() < 0.05 {
                format!("{}M", m.round() as u64)
            } else {
                format!("{m:.1}M")
            }
        }
        n if n >= 1000 => format!("{}k", (n as f64 / 1000.0).round() as u64),
        n => n.to_string(),
    }
}

pub(super) fn context_usage(chat: Chat) -> Option<Usage> {
    if chat.fresh.get() || chat.view.get().is_none() {
        return None;
    }
    let (context, cost) = chat.transcript.with(|t| (t.context, t.cost));
    (context.is_some() || cost > 0.0).then(|| Usage {
        used: context.map(|c| c.0),
        window: context.map_or(0, |c| c.1),
        cost,
    })
}

pub(super) fn usage_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    move || {
        let u = context_usage(chat);
        let busy = chat.running.get();
        let claude = chat.runtime() == "claude";
        view! {
            <div class="cp-h">"Context"</div>
            <div class="cp-usage">
                {u
                    .map(|u| {
                        view! {
                            <p>{u.tip()}</p>
                            {u
                                .low()
                                .then(|| {
                                    view! {
                                        <p class="cp-usage-low">
                                            "Context is almost full. Compact it, or start a new chat and carry on from memory."
                                        </p>
                                    }
                                })}
                        }
                    })}
                <div class="cp-usage-acts">
                    {claude
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="btn small"
                                    disabled=busy
                                    title=if busy {
                                        "Wait for this turn to finish"
                                    } else {
                                        "Summarize the conversation to free up context (/compact)"
                                    }
                                    on:click=move |_| {
                                        pop.set(None);
                                        chat.send_text("/compact".to_owned());
                                    }
                                >
                                    "Compact"
                                </button>
                            }
                        })}
                    <button
                        type="button"
                        class="btn small"
                        title="Start a new session with empty context (/clear)"
                        on:click=move |_| {
                            pop.set(None);
                            chat.new_chat();
                        }
                    >
                        "New chat"
                    </button>
                </div>
            </div>
        }
    }
}
