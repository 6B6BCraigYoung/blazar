//! xterm.js 终端的绑定，实现在 js/term.js。

use js_sys::Function;
use wasm_bindgen::prelude::*;
use web_sys::HtmlElement;

#[wasm_bindgen(module = "/js/term.js")]
extern "C" {
    pub type Term;

    #[wasm_bindgen(js_name = openTerm)]
    fn open_term(host: &HtmlElement, url: &str, on_state: &Function) -> Term;

    #[wasm_bindgen(method)]
    pub fn focus(this: &Term);
    #[wasm_bindgen(method)]
    pub fn dispose(this: &Term);
}

/// 连着的终端；回调要跟它活得一样久。
pub struct Open {
    pub term: Term,
    _on_state: Closure<dyn Fn(String)>,
}

/// `on_state` 收到 "open" / "closed"。
pub fn open(host: &HtmlElement, url: &str, on_state: impl Fn(String) + 'static) -> Open {
    let cb = Closure::<dyn Fn(String)>::new(on_state);
    let term = open_term(host, url, cb.as_ref().unchecked_ref());
    Open {
        term,
        _on_state: cb,
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.term.dispose();
    }
}
