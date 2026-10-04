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

#[derive(Clone, Copy)]
pub struct Dialogs(RwSignal<Option<Open>, LocalStorage>);

thread_local! {
    static DIALOGS: std::cell::Cell<Option<Dialogs>> = const { std::cell::Cell::new(None) };
}

pub fn provide() {
    DIALOGS.set(Some(Dialogs(RwSignal::new_local(None))));
}

fn dialogs() -> Dialogs {
    DIALOGS.get().expect("dialog::provide 还没调用")
}

pub async fn ask(title: &str, body: &str, choices: Vec<Choice>) -> Option<usize> {
    let d = dialogs();
    let (tx, rx) = oneshot::channel();
    if let Some(old) = d.0.write().replace(Open {
        title: title.to_owned(),
        body: body.to_owned(),
        choices,
        reply: tx,
    }) {
        let _ = old.reply.send(None);
    }
    rx.await.ok().flatten()
}

fn answer(d: Dialogs, v: Option<usize>) {
    if let Some(o) = d.0.write().take() {
        let _ = o.reply.send(v);
    }
}

#[component]
pub fn DialogHost() -> impl IntoView {
    let d = dialogs();
    move || {
        d.0.with(|o| {
            o.as_ref().map(|o| {
                let buttons = o
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let cls = if c.danger { "btn danger" } else { "btn" };
                        let label = c.label.clone();
                        view! { <button type="button" class=cls on:click=move |_| answer(d, Some(i))>{label}</button> }
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
    }
}
