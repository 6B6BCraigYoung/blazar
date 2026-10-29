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

pub const DELAY_MS: u32 = 5000;
const SETTLE_MS: u32 = 1200;

pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub body: Value,
    /// 5 秒后真的发出去却没成功时，toast 的前缀（“没删掉：服务繁忙，稍后再试”）。
    pub failed: &'static str,
}

impl Request {
    pub fn delete(path: String) -> Self {
        Self {
            method: "DELETE",
            path,
            body: Value::Object(Default::default()),
            failed: "没删掉",
        }
    }

    pub fn post(path: String) -> Self {
        Self {
            method: "POST",
            failed: "没做成",
            ..Self::delete(path)
        }
    }

    #[must_use]
    pub const fn failing(mut self, lead: &'static str) -> Self {
        self.failed = lead;
        self
    }
}

struct Pending {
    requests: Vec<Request>,
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
    later_all(text, vec![request], undo, done);
}

/// [`later`] for several requests behind one toast (a batch archive): 撤销 cancels them all.
pub fn later_all(
    text: impl Into<String>,
    requests: Vec<Request>,
    undo: impl Fn() + 'static,
    done: impl Fn() + 'static,
) {
    let id = LEDGER.with(|l| {
        l.borrow_mut().add(Pending {
            requests,
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
    remove_rows(text, vec![(id, request)], done);
}

/// Hide several rows at once and send their requests after 5 seconds, behind one 撤销
/// (e.g. 「已归档 3 个智能体，正在运行的任务会被取消」).
pub fn remove_rows(
    text: impl Into<String>,
    rows: Vec<(String, Request)>,
    done: impl Fn() + 'static,
) {
    let (ids, requests): (Vec<String>, Vec<Request>) = rows.into_iter().unzip();
    hidden().update(|h| h.extend(ids.iter().cloned()));
    let back = ids.clone();
    later_all(
        text,
        requests,
        move || {
            hidden().update(|h| {
                for id in &back {
                    h.remove(id);
                }
            });
        },
        move || {
            done();
            let ids = ids.clone();
            gloo_timers::callback::Timeout::new(SETTLE_MS, move || {
                hidden().update(|h| {
                    for id in &ids {
                        h.remove(id);
                    }
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
        let total = p.requests.len();
        let mut failed = 0;
        for r in &p.requests {
            if let Err(e) = api::send::<Value>(r.method, &r.path, &r.body).await {
                if failed == 0 {
                    toast::toast_error(r.failed, &e);
                }
                failed += 1;
            }
        }
        if failed > 0 {
            (p.undo)();
        }
        if failed < total {
            (p.done)();
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
        for r in &p.requests {
            let init = web_sys::RequestInit::new();
            init.set_method(r.method);
            let _ = js_sys::Reflect::set(init.as_ref(), &"keepalive".into(), &JsValue::TRUE);
            init.set_body(&JsValue::from_str(&r.body.to_string()));
            if let Ok(headers) = web_sys::Headers::new() {
                let _ = headers.set("content-type", "application/json");
                init.set_headers_headers(&headers);
            }
            let _ = window().fetch_with_str_and_init(&r.path, &init);
        }
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
