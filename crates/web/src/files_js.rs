use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/files.js")]
extern "C" {
    #[wasm_bindgen(js_name = setNavigationGuard)]
    pub fn set_navigation_guard(workspace: &str, dirty: &js_sys::Function);
    #[wasm_bindgen(js_name = clearNavigationGuard)]
    pub fn clear_navigation_guard(workspace: &str);
    #[wasm_bindgen(js_name = confirmNavigation)]
    pub fn confirm_navigation() -> bool;
    #[wasm_bindgen(js_name = downloadText)]
    pub fn download_text(name: &str, text: &str, mime: &str);
    #[wasm_bindgen(js_name = readText)]
    fn read_text_js(file: &web_sys::File) -> js_sys::Promise;
}

pub async fn read_text(file: &web_sys::File) -> Option<String> {
    wasm_bindgen_futures::JsFuture::from(read_text_js(file))
        .await
        .ok()?
        .as_string()
}
