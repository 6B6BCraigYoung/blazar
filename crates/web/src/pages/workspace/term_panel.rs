use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::storage;
use crate::term;

fn tabs_key(ws: &str) -> String {
    format!("blazar.terms.{ws}")
}

#[derive(Clone, Copy)]
pub struct Terms {
    pub tabs: RwSignal<Vec<u32>>,
    pub cur: RwSignal<u32>,
    pub state: RwSignal<&'static str>,
    pub reconnect: RwSignal<u32>,
}

impl Terms {
    pub fn new(ws: &str) -> Self {
        let tabs = storage::load::<Vec<u32>>(&tabs_key(ws))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| vec![0]);
        let cur = tabs[0];
        Self {
            tabs: RwSignal::new(tabs),
            cur: RwSignal::new(cur),
            state: RwSignal::new("connecting"),
            reconnect: RwSignal::new(0),
        }
    }
}

#[component]
pub fn TermTabs(ws: String, terms: Terms) -> impl IntoView {
    let key = tabs_key(&ws);
    let save = {
        let key = key.clone();
        move || storage::save(&key, &terms.tabs.get_untracked())
    };
    let save2 = save.clone();
    let add = move |_| {
        let n = terms
            .tabs
            .with_untracked(|t| t.iter().max().copied().unwrap_or(0) + 1);
        terms.tabs.update(|t| t.push(n));
        terms.cur.set(n);
        save();
    };
    view! {
        <div class="term-tabs" role="group" aria-label="终端会话">
            {move || {
                let list = terms.tabs.get();
                let many = list.len() > 1;
                list.into_iter().enumerate().map(|(i, n)| {
                    let save = save2.clone();
                    view! {
                        <span class="term-tab" data-on=move || (terms.cur.get() == n).to_string()>
                            <button type="button" class="term-select" aria-pressed=move || (terms.cur.get() == n).to_string() on:click=move |_| if terms.cur.get_untracked() != n { terms.cur.set(n) }>
                            {format!("终端 {}", i + 1)}</button>
                            {many.then(|| view! {
                                <button type="button" class="x" aria-label=format!("关闭终端 {} 标签", i + 1) title="关闭标签，保留远端会话" on:click=move |e| {
                                    e.stop_propagation();
                                    terms.tabs.update(|t| t.retain(|x| *x != n));
                                    if terms.cur.get_untracked() == n {
                                        terms.cur.set(terms.tabs.with_untracked(|t| t[0]));
                                    }
                                    save();
                                }>"×"</button>
                            })}
                        </span>
                    }
                }).collect_view()
            }}
            <button type="button" class="term-tab" aria-label="新建终端" title="新建终端" on:click=add>"＋"</button>
        </div>
    }
}

#[component]
pub fn TermPane(ws: String, terms: Terms, active: Signal<bool>) -> impl IntoView {
    let host = NodeRef::<html::Div>::new();
    let live = StoredValue::new_local(None::<Rc<term::Open>>);
    let wanted = Memo::new(move |was: Option<&bool>| was.copied().unwrap_or(false) || active.get());
    Effect::new(move |_| {
        let tab = terms.cur.get();
        terms.reconnect.track();
        if !wanted.get() {
            return;
        }
        let Some(el) = host.get() else { return };
        live.set_value(None);
        terms.state.set("connecting");
        let loc = window().location();
        let scheme = if loc.protocol().as_deref() == Ok("https:") {
            "wss"
        } else {
            "ws"
        };
        let url = format!(
            "{scheme}://{}/api/workspaces/{ws}/terminal/ws?tab={tab}",
            loc.host().unwrap_or_default()
        );
        let el: web_sys::HtmlElement = el.unchecked_into();
        let open = term::open(&el, &url, move |s| {
            let _ = terms
                .state
                .try_set(if s == "open" { "open" } else { "closed" });
        });
        open.term.focus();
        live.set_value(Some(Rc::new(open)));
    });
    Effect::new(move |_| {
        if active.get() {
            live.with_value(|t| {
                if let Some(t) = t {
                    t.term.focus()
                }
            });
        }
    });
    on_cleanup(move || live.set_value(None));
    view! { <div class="term-host" role="region" aria-label="终端" node_ref=host></div> }
}
