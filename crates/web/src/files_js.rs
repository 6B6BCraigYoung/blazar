use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/files.js")]
extern "C" {
    #[wasm_bindgen(js_name = setNavigationGuard)]
    pub fn set_navigation_guard(workspace: &str, dirty: &js_sys::Function);
    #[wasm_bindgen(js_name = clearNavigationGuard)]
    pub fn clear_navigation_guard(workspace: &str);
    #[wasm_bindgen(js_name = leaveWarning)]
    fn leave_warning() -> String;
    #[wasm_bindgen(js_name = setLeaveConfirm)]
    fn set_leave_confirm(confirm: &js_sys::Function);
    #[wasm_bindgen(js_name = downloadText)]
    pub fn download_text(name: &str, text: &str, mime: &str);
    #[wasm_bindgen(js_name = downloadUrl)]
    pub fn download_url(url: &str, name: &str);
    #[wasm_bindgen(js_name = canPickSaveFile)]
    pub fn can_pick_save_file() -> bool;
    #[wasm_bindgen(js_name = desktopSaveDownload, catch)]
    pub fn desktop_save_download(
        url: &str,
        name: &str,
        on_event: &js_sys::Function,
    ) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_name = pickSaveFile)]
    pub fn pick_save_file(name: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = savedName)]
    pub fn saved_name(handle: &JsValue) -> String;
    #[wasm_bindgen(js_name = saveToFile)]
    pub fn save_to_file(
        handle: &JsValue,
        url: &str,
        signal: &web_sys::AbortSignal,
        on_progress: &js_sys::Function,
    ) -> js_sys::Promise;
    #[wasm_bindgen(js_name = readText)]
    fn read_text_js(file: &web_sys::File) -> js_sys::Promise;
}

async fn ask_to_leave(message: String) -> bool {
    use crate::components::dialog::{Choice, ask};
    ask(
        "离开这个页面？",
        &message,
        vec![Choice::plain("留下"), Choice::danger("离开")],
    )
    .await
        == Some(1)
}

pub fn leave_blocked() -> bool {
    !leave_warning().is_empty()
}

pub async fn confirm_leave() -> bool {
    let message = leave_warning();
    message.is_empty() || ask_to_leave(message).await
}

pub fn install_leave_confirm() {
    let confirm = Closure::<dyn Fn(String) -> js_sys::Promise>::new(|message: String| {
        wasm_bindgen_futures::future_to_promise(async move {
            Ok(JsValue::from_bool(ask_to_leave(message).await))
        })
    });
    set_leave_confirm(confirm.as_ref().unchecked_ref());
    confirm.forget();
}

pub async fn read_text(file: &web_sys::File) -> Option<String> {
    wasm_bindgen_futures::JsFuture::from(read_text_js(file))
        .await
        .ok()?
        .as_string()
}
