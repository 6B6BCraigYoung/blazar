use js_sys::Function;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

#[wasm_bindgen(module = "/js/stream.js")]
extern "C" {
    pub type Stream;

    #[wasm_bindgen(js_name = openStream)]
    fn open_stream(canvas: &HtmlCanvasElement, url: &str, on_state: &Function) -> Stream;

    #[wasm_bindgen(method)]
    pub fn snapshot(this: &Stream);

    #[wasm_bindgen(method)]
    pub fn fullscreen(this: &Stream);

    #[wasm_bindgen(method)]
    pub fn dispose(this: &Stream);
}

pub struct Open {
    pub stream: Stream,
    _on_state: Closure<dyn Fn(String)>,
}

pub fn open(canvas: &HtmlCanvasElement, path: &str, on_state: impl Fn(String) + 'static) -> Open {
    let cb = Closure::<dyn Fn(String)>::new(on_state);
    let url = crate::api::websocket_url(path);
    let stream = open_stream(canvas, &url, cb.as_ref().unchecked_ref());
    Open {
        stream,
        _on_state: cb,
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        self.stream.dispose();
    }
}
