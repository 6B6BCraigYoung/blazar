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

pub struct Open {
    pub term: Term,
    _on_state: Closure<dyn Fn(String)>,
}

pub fn open(host: &HtmlElement, url: &str, on_state: impl Fn(String) + 'static) -> Open {
    let cb = Closure::<dyn Fn(String)>::new(on_state);
    let path = url.find("/api/").map_or(url, |start| &url[start..]);
    let socket_url = crate::api::websocket_url(path);
    let term = open_term(host, &socket_url, cb.as_ref().unchecked_ref());
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
