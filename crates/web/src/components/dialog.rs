//! 页内对话框：`ask(...)` 弹出一个带若干按钮的框，等用户点完返回按钮序号（关掉返回 None）。

use futures::channel::oneshot;
use leptos::prelude::*;

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

// 全局一份，不走 context：`ask` 常在 JS 回调（比如编辑器里按 ⌘S）后的异步任务里调用，那里没有组件上下文。
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
    // 已经开着一个的话，旧的那个当作取消。
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
                        view! { <button class=cls on:click=move |_| answer(d, Some(i))>{label}</button> }
                    })
                    .collect_view();
                view! {
                    <div class="dlg-mask" on:click=move |_| answer(d, None)>
                        <div class="dlg" role="dialog" on:click=|e| e.stop_propagation()>
                            <h3>{o.title.clone()}</h3>
                            <div class="dlg-body">{o.body.clone()}</div>
                            <div class="dlg-foot">{buttons}</div>
                        </div>
                    </div>
                }
            })
        })
    }
}
