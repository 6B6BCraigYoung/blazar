use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue};

use super::toast;
use crate::api;
use crate::undo_ledger::Ledger;

const DELAY_MS: u32 = 5000;
const SETTLE_MS: u32 = 1200;

pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub body: Value,
}

impl Request {
    pub fn delete(path: String) -> Self {
        Self {
            method: "DELETE",
            path,
            body: Value::Object(Default::default()),
        }
    }

    pub fn post(path: String) -> Self {
        Self {
            method: "POST",
            ..Self::delete(path)
        }
    }
}

struct Pending {
    request: Request,
    undo: Box<dyn Fn()>,
    done: Box<dyn Fn()>,
    toast: u32,
    timer: Option<gloo_timers::callback::Timeout>,
}

thread_local! {
    static LEDGER: RefCell<Ledger<Pending>> = RefCell::new(Ledger::default());
    static HIDDEN: Cell<Option<RwSignal<HashSet<String>>>> = const { Cell::new(None) };
}

pub fn provide() {
    HIDDEN.set(Some(RwSignal::new(HashSet::new())));
}

fn hidden() -> RwSignal<HashSet<String>> {
    HIDDEN.get().expect("undo::provide 还没调用")
}

pub fn shows(id: &str) -> bool {
    hidden().with(|h| !h.contains(id))
}

pub fn later(
    text: impl Into<String>,
    request: Request,
    undo: impl Fn() + 'static,
    done: impl Fn() + 'static,
) {
    let id = LEDGER.with(|l| {
        l.borrow_mut().add(Pending {
            request,
            undo: Box::new(undo),
            done: Box::new(done),
            toast: 0,
            timer: None,
        })
    });
    let shown = toast::offer(text, "撤销", DELAY_MS, move || cancel(id));
    let timer = gloo_timers::callback::Timeout::new(DELAY_MS, move || commit(id));
    LEDGER.with(|l| {
        if let Some(p) = l.borrow_mut().get_mut(id) {
            p.toast = shown;
            p.timer = Some(timer);
        }
    });
}

pub fn remove_row(
    text: impl Into<String>,
    id: String,
    request: Request,
    done: impl Fn() + 'static,
) {
    hidden().update(|h| {
        h.insert(id.clone());
    });
    let back = id.clone();
    later(
        text,
        request,
        move || {
            hidden().update(|h| {
                h.remove(&back);
            });
        },
        move || {
            done();
            let id = id.clone();
            gloo_timers::callback::Timeout::new(SETTLE_MS, move || {
                hidden().update(|h| {
                    h.remove(&id);
                });
            })
            .forget();
        },
    );
}

fn cancel(id: u32) {
    let Some(p) = LEDGER.with(|l| l.borrow_mut().take(id)) else {
        return;
    };
    toast::dismiss(p.toast);
    (p.undo)();
}

fn send(p: Pending) {
    spawn_local(async move {
        match api::send::<Value>(p.request.method, &p.request.path, &p.request.body).await {
            Ok(_) => (p.done)(),
            Err(e) => {
                toast::toast(format!("没删掉：{e}"));
                (p.undo)();
            }
        }
    });
}

fn commit(id: u32) {
    if let Some(p) = LEDGER.with(|l| l.borrow_mut().take(id)) {
        send(p);
    }
}

pub fn undo_last() -> bool {
    let Some((_, p)) = LEDGER.with(|l| l.borrow_mut().take_last()) else {
        return false;
    };
    toast::dismiss(p.toast);
    (p.undo)();
    true
}

fn flush_now() {
    for (_, p) in LEDGER.with(|l| l.borrow_mut().drain()) {
        let init = web_sys::RequestInit::new();
        init.set_method(p.request.method);
        let _ = js_sys::Reflect::set(init.as_ref(), &"keepalive".into(), &JsValue::TRUE);
        init.set_body(&JsValue::from_str(&p.request.body.to_string()));
        if let Ok(headers) = web_sys::Headers::new() {
            let _ = headers.set("content-type", "application/json");
            init.set_headers_headers(&headers);
        }
        let _ = window().fetch_with_str_and_init(&p.request.path, &init);
    }
}

fn in_field(e: &ev::KeyboardEvent) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|t| {
            t.closest("input, textarea, select, [contenteditable], .monaco-editor, .xterm")
                .ok()
                .flatten()
                .is_some()
        })
}

#[component]
pub fn UndoHost() -> impl IntoView {
    let keys = window_event_listener(ev::keydown, move |e| {
        if (e.meta_key() || e.ctrl_key())
            && !e.shift_key()
            && !e.alt_key()
            && e.code() == "KeyZ"
            && !e.default_prevented()
            && !in_field(&e)
            && undo_last()
        {
            e.prevent_default();
        }
    });
    let unload = window_event_listener_untyped("pagehide", move |_| flush_now());
    on_cleanup(move || {
        keys.remove();
        unload.remove();
    });
}
