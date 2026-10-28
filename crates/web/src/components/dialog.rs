use futures::channel::oneshot;
use leptos::prelude::*;

use super::modal::Modal;

#[derive(Clone)]
pub struct Choice {
    pub label: String,
    pub danger: bool,
}

impl Choice {
    pub fn plain(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            danger: false,
        }
    }
    pub fn danger(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            danger: true,
        }
    }
}

struct Open {
    title: String,
    body: String,
    choices: Vec<Choice>,
    reply: oneshot::Sender<Option<usize>>,
}

struct Prompt {
    title: String,
    label: String,
    initial: String,
    ok: String,
    cancel: String,
    reply: oneshot::Sender<Option<String>>,
}

#[derive(Clone, Copy)]
pub struct Dialogs {
    ask: RwSignal<Option<Open>, LocalStorage>,
    prompt: RwSignal<Option<Prompt>, LocalStorage>,
}

thread_local! {
    static DIALOGS: std::cell::Cell<Option<Dialogs>> = const { std::cell::Cell::new(None) };
}

pub fn provide() {
    DIALOGS.set(Some(Dialogs {
        ask: RwSignal::new_local(None),
        prompt: RwSignal::new_local(None),
    }));
}

fn dialogs() -> Dialogs {
    DIALOGS.get().expect("dialog::provide 还没调用")
}

pub async fn ask(title: &str, body: &str, choices: Vec<Choice>) -> Option<usize> {
    let d = dialogs();
    let (tx, rx) = oneshot::channel();
    if let Some(old) = d.ask.write().replace(Open {
        title: title.to_owned(),
        body: body.to_owned(),
        choices,
        reply: tx,
    }) {
        let _ = old.reply.send(None);
    }
    rx.await.ok().flatten()
}

pub async fn prompt(title: &str, label: &str, initial: &str) -> Option<String> {
    prompt_with(title, label, initial, "确定").await
}

pub async fn prompt_with(title: &str, label: &str, initial: &str, ok: &str) -> Option<String> {
    prompt_full(title, label, initial, ok, "取消").await
}

async fn prompt_full(
    title: &str,
    label: &str,
    initial: &str,
    ok: &str,
    cancel: &str,
) -> Option<String> {
    let d = dialogs();
    let (tx, rx) = oneshot::channel();
    if let Some(old) = d.prompt.write().replace(Prompt {
        title: title.to_owned(),
        label: label.to_owned(),
        initial: initial.to_owned(),
        ok: ok.to_owned(),
        cancel: cancel.to_owned(),
        reply: tx,
    }) {
        let _ = old.reply.send(None);
    }
    rx.await
        .ok()
        .flatten()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn answer(d: Dialogs, v: Option<usize>) {
    if let Some(o) = d.ask.write().take() {
        let _ = o.reply.send(v);
    }
}

fn finish(d: Dialogs, v: Option<String>) {
    if let Some(p) = d.prompt.write().take() {
        let _ = p.reply.send(v);
    }
}

#[component]
pub fn DialogHost() -> impl IntoView {
    let d = dialogs();
    let asks = move || {
        d.ask.with(|o| {
            o.as_ref().map(|o| {
                let buttons = o
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let cls = if c.danger { "btn danger" } else { "btn" };
                        let label = c.label.clone();
                        view! {
                            <button type="button" class=cls on:click=move |_| answer(d, Some(i))>
                                {label}
                            </button>
                        }
                    })
                    .collect_view();
                let title = o.title.clone();
                let body = o.body.clone();
                view! {
                    <Modal label=title.clone() on_close=Callback::new(move |_| answer(d, None))>
                        <h3>{title}</h3>
                        <div class="dlg-body">{body}</div>
                        <div class="dlg-foot">{buttons}</div>
                    </Modal>
                }
            })
        })
    };
    let prompts = move || {
        d.prompt.with(|p| {
            p.as_ref().map(|p| {
                let title = p.title.clone();
                let label = p.label.clone();
                let ok = p.ok.clone();
                let cancel = p.cancel.clone();
                let value = RwSignal::new(p.initial.clone());
                let empty = move || value.with(|v| v.trim().is_empty());
                let submit = move || {
                    if !empty() {
                        finish(d, Some(value.get_untracked()));
                    }
                };
                view! {
                    <Modal label=title.clone() on_close=Callback::new(move |_| finish(d, None))>
                        <h3>{title}</h3>
                        <label class="field">
                            {label}
                            <input
                                data-modal-initial-focus=""
                                maxlength="200"
                                autocomplete="off"
                                spellcheck="false"
                                prop:value=move || value.get()
                                on:input=move |e| value.set(event_target_value(&e))
                                on:keydown=move |e| {
                                    if e.key() == "Enter" {
                                        e.prevent_default();
                                        submit()
                                    }
                                }
                            />
                        </label>
                        <div class="dlg-foot">
                            <button type="button" class="btn" on:click=move |_| finish(d, None)>
                                {cancel}
                            </button>
                            <button
                                type="button"
                                class="btn primary"
                                disabled=empty
                                on:click=move |_| submit()
                            >
                                {ok}
                            </button>
                        </div>
                    </Modal>
                }
            })
        })
    };
    view! {
        {asks}
        {prompts}
    }
}
