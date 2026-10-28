use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/client_log.js")]
extern "C" {
    #[wasm_bindgen(js_name = install)]
    pub fn install();
    #[wasm_bindgen(js_name = note)]
    fn note_js(kind: &str, message: &str);
    #[wasm_bindgen(js_name = panicked)]
    pub fn panicked(message: &str);
}

pub fn note(kind: &str, message: &str) {
    note_js(kind, message);
}

pub fn api_failed(method: &str, path: &str, status: Option<u16>) {
    if matches!(status, Some(404 | 409)) {
        return;
    }
    let path = path.split('?').next().unwrap_or(path);
    let status = status.map_or_else(|| "no response".to_owned(), |s| s.to_string());
    note_js("api", &format!("{method} {path} -> {status}"));
}
