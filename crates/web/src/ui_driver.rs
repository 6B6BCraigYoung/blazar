use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/driver.js")]
extern "C" {
    #[wasm_bindgen(js_name = runUiCommand)]
    fn run_ui_command(raw: &str) -> js_sys::Promise;
}

pub fn handle(id: u64, command: &serde_json::Value) {
    let raw = command.to_string();
    leptos::task::spawn_local(async move {
        let out = wasm_bindgen_futures::JsFuture::from(run_ui_command(&raw))
            .await
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let result = serde_json::from_str(&out)
            .unwrap_or_else(|_| serde_json::json!({ "error": "页面没有返回结果" }));
        let _ =
            crate::api::send::<serde_json::Value>("POST", &format!("/api/dev/ui/{id}"), &result)
                .await;
    });
}
