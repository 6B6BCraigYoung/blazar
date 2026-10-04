use js_sys::{Function, Promise};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::HtmlElement;

#[wasm_bindgen(module = "/js/monaco.js")]
extern "C" {
    pub type Editor;

    #[wasm_bindgen(js_name = createEditor)]
    fn create_editor(host: &HtmlElement, on_save: &Function, on_dirty: &Function) -> Promise;

    #[wasm_bindgen(js_name = colorize)]
    fn colorize_js(text: &str, lang: &str) -> Promise;

    #[wasm_bindgen(method)]
    pub fn has(this: &Editor, path: &str) -> bool;
    #[wasm_bindgen(method)]
    pub fn open(this: &Editor, path: &str, text: &str, readonly: bool, line: u32);
    #[wasm_bindgen(method)]
    pub fn value(this: &Editor, path: &str) -> String;
    #[wasm_bindgen(method)]
    pub fn replace(this: &Editor, path: &str, text: &str);
    #[wasm_bindgen(method, js_name = markSaved)]
    pub fn mark_saved(this: &Editor, path: &str);
    #[wasm_bindgen(method)]
    pub fn close(this: &Editor, path: &str);
    #[wasm_bindgen(method)]
    pub fn clear(this: &Editor);
    #[wasm_bindgen(method)]
    pub fn focus(this: &Editor);
    #[wasm_bindgen(method)]
    pub fn dispose(this: &Editor);
}

pub struct Mounted {
    pub editor: Editor,
    _on_save: Closure<dyn Fn(String)>,
    _on_dirty: Closure<dyn Fn(String, bool)>,
}

pub async fn mount(
    host: &HtmlElement,
    on_save: impl Fn(String) + 'static,
    on_dirty: impl Fn(String, bool) + 'static,
) -> Result<Mounted, JsValue> {
    let on_save = Closure::<dyn Fn(String)>::new(on_save);
    let on_dirty = Closure::<dyn Fn(String, bool)>::new(on_dirty);
    let ed = JsFuture::from(create_editor(
        host,
        on_save.as_ref().unchecked_ref(),
        on_dirty.as_ref().unchecked_ref(),
    ))
    .await?;
    Ok(Mounted {
        editor: ed.unchecked_into(),
        _on_save: on_save,
        _on_dirty: on_dirty,
    })
}

pub async fn colorize(text: &str, lang: &str) -> Option<String> {
    JsFuture::from(colorize_js(text, lang))
        .await
        .ok()?
        .as_string()
}
