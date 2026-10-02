//! 底部面板的终端：每个工作区可以开几个（远端是各自的 tmux 会话，断线重连回到原处）。

use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::storage;
use crate::term;

fn tabs_key(ws: &str) -> String {
    // 跟旧界面用同一个键：两边开的终端标签一致。
    format!("blazar.terms.{ws}")
}

#[derive(Clone, Copy)]
pub struct Terms {
    pub tabs: RwSignal<Vec<u32>>,
    pub cur: RwSignal<u32>,
    /// 当前终端的连接状态："connecting" / "open" / "closed"
    pub state: RwSignal<&'static str>,
    /// 加一就重连当前终端。
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

/// 终端标签条（放在面板标题栏里）。
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
        <div class="term-tabs">
            {move || {
                let list = terms.tabs.get();
                let many = list.len() > 1;
                list.into_iter().enumerate().map(|(i, n)| {
                    let save = save2.clone();
                    view! {
                        <button class="term-tab" data-on=move || (terms.cur.get() == n).to_string()
                            on:click=move |_| if terms.cur.get_untracked() != n { terms.cur.set(n) }>
                            {format!("终端 {}", i + 1)}
                            {many.then(|| view! {
                                <span class="x" title="关闭这个标签（远端会话保留）" on:click=move |e| {
                                    e.stop_propagation();
                                    terms.tabs.update(|t| t.retain(|x| *x != n));
                                    if terms.cur.get_untracked() == n {
                                        terms.cur.set(terms.tabs.with_untracked(|t| t[0]));
                                    }
                                    save();
                                }>"×"</span>
                            })}
                        </button>
                    }
                }).collect_view()
            }}
            <button class="term-tab" title="新建终端" on:click=add>"＋"</button>
        </div>
    }
}

/// 终端本体。`active` 为真（面板展开且选中终端页签）时才第一次连接，之后切走也保持连接。
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
        // 先断开旧的，再连新的。
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
    // 切回终端页签时把焦点给它。
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
    view! { <div class="term-host" node_ref=host></div> }
}
