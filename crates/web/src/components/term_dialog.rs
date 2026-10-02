//! 在对话框里开一个终端跑登录命令（claude auth login / codex login …），关掉后回调。

use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::term;

#[derive(Clone)]
pub struct TermLogin {
    pub title: String,
    pub hint: String,
    /// hub 上的 WebSocket 路径，比如 /api/accounts/{id}/login/ws
    pub path: String,
    pub after: Callback<()>,
}

thread_local! {
    static OPEN: std::cell::Cell<Option<RwSignal<Option<TermLogin>, LocalStorage>>> = const { std::cell::Cell::new(None) };
}

pub fn provide() {
    OPEN.set(Some(RwSignal::new_local(None)));
}

pub fn open(t: TermLogin) {
    if let Some(s) = OPEN.get() {
        s.set(Some(t));
    }
}

#[component]
pub fn TermDialogHost() -> impl IntoView {
    let open = OPEN.get().expect("term_dialog::provide 还没调用");
    move || {
        open.get()
            .map(|t| view! { <Dialog t on_close=move || open.set(None)/> })
    }
}

#[component]
fn Dialog(t: TermLogin, on_close: impl Fn() + Copy + 'static) -> impl IntoView {
    let host = NodeRef::<html::Div>::new();
    let live = StoredValue::new_local(None::<Rc<term::Open>>);
    let state = RwSignal::new("登录中…");
    let path = t.path.clone();
    Effect::new(move |_| {
        let Some(el) = host.get() else { return };
        if live.with_value(Option::is_some) {
            return;
        }
        let loc = window().location();
        let scheme = if loc.protocol().as_deref() == Ok("https:") {
            "wss"
        } else {
            "ws"
        };
        let url = format!("{scheme}://{}{path}", loc.host().unwrap_or_default());
        let o = term::open(el.unchecked_ref(), &url, move |s| {
            if s == "closed" {
                let _ = state.try_set("登录流程已结束");
            }
        });
        o.term.focus();
        live.set_value(Some(Rc::new(o)));
    });
    let after = t.after;
    let finish = move || {
        live.set_value(None);
        on_close();
        after.run(());
    };
    view! {
        <div class="dlg-mask">
            <div class="dlg wide">
                <h3>{t.title.clone()}</h3>
                <div class="muted small" inner_html=t.hint.clone()></div>
                <div class="login-term" node_ref=host></div>
                <div class="dlg-foot">
                    <span class="muted small grow">{move || state.get()}</span>
                    <button class="btn primary" on:click=move |_| finish()>"完成"</button>
                </div>
            </div>
        </div>
    }
}
