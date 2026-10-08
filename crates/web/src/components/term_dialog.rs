use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::term;

use super::modal::Modal;

#[derive(Clone)]
pub struct TermLogin {
    pub title: String,
    pub hint: String,
    pub path: String,
    pub verify_node: Option<String>,
    pub after: Callback<bool>,
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
            .map(|t| view! { <Dialog t on_close=Callback::new(move |_| open.set(None))/> })
    }
}

#[component]
fn Dialog(t: TermLogin, on_close: Callback<()>) -> impl IntoView {
    let host = NodeRef::<html::Div>::new();
    let live = StoredValue::new_local(None::<Rc<term::Open>>);
    let state = RwSignal::new("登录中…");
    let verified = RwSignal::new(false);
    let path = t.path.clone();
    let after = t.after;
    let finish = move || {
        live.set_value(None);
        on_close.run(());
        after.run(verified.get_untracked());
    };
    let verify_node = t.verify_node.clone();
    let on_end = move || {
        let Some(node) = verify_node.clone() else {
            let _ = state.try_set("登录流程已结束");
            return;
        };
        let _ = state.try_set("正在确认登录状态…");
        leptos::task::spawn_local(async move {
            let ok = crate::api::get::<Vec<serde_json::Value>>(&format!(
                "/api/nodes/{}/agents",
                crate::api::enc(&node)
            ))
            .await
            .is_ok_and(|list| {
                list.iter()
                    .any(|a| a["id"] == "codex" && a["authed"].as_bool() == Some(true))
            });
            if ok {
                let _ = verified.try_set(true);
                if live.try_with_value(Option::is_some) == Some(true) {
                    finish();
                }
            } else {
                let _ = state.try_set("没有登录成功，可以关闭后重试");
            }
        });
    };
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
        let on_end = on_end.clone();
        let o = term::open(el.unchecked_ref(), &url, move |s| {
            if s == "closed" {
                on_end();
            }
        });
        o.term.focus();
        live.set_value(Some(Rc::new(o)));
    });
    view! {
        <Modal label=t.title.clone() class="dlg wide" close_on_backdrop=false on_close=Callback::new(move |_| finish())>
                <h3>{t.title.clone()}</h3>
                <div class="muted small">{t.hint.clone()}</div>
                <div class="login-term" node_ref=host></div>
                <div class="dlg-foot">
                    <span class="muted small grow">{move || state.get()}</span>
                    <button type="button" class="btn primary" on:click=move |_| finish()>"完成"</button>
                </div>
        </Modal>
    }
}
