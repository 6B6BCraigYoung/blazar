use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/interaction.js")]
extern "C" {
    #[wasm_bindgen(js_name = menuKeydown)]
    fn menu_keydown_js(event: &web_sys::KeyboardEvent, root: &web_sys::HtmlElement) -> bool;
}

pub fn menu_keydown(
    event: &web_sys::KeyboardEvent,
    root: &web_sys::HtmlElement,
    on_close: Callback<()>,
) {
    if menu_keydown_js(event, root) {
        on_close.run(());
    }
}
