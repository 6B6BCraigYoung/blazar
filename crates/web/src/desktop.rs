use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue};

fn core() -> Option<JsValue> {
    let window: JsValue = web_sys::window()?.into();
    let tauri = js_sys::Reflect::get(&window, &"__TAURI__".into()).ok()?;
    js_sys::Reflect::get(&tauri, &"core".into())
        .ok()
        .filter(|c| !c.is_undefined() && !c.is_null())
}

fn invoker() -> Option<(JsValue, js_sys::Function)> {
    let core = core()?;
    let invoke = js_sys::Reflect::get(&core, &"invoke".into())
        .ok()?
        .dyn_into()
        .ok()?;
    Some((core, invoke))
}

pub fn available() -> bool {
    invoker().is_some()
}

fn message(value: JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            js_sys::JSON::stringify(&value)
                .ok()
                .and_then(|s| s.as_string())
        })
        .unwrap_or_else(|| "桌面端没有回应".into())
}

pub async fn invoke(command: &str, args: &Value) -> Result<Value, String> {
    let (core, invoke) = invoker().ok_or_else(|| "只有桌面端能用".to_owned())?;
    let args = js_sys::JSON::parse(&args.to_string()).map_err(message)?;
    let promise: js_sys::Promise = invoke
        .call2(&core, &command.into(), &args)
        .map_err(message)?
        .dyn_into()
        .map_err(message)?;
    let out = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(message)?;
    if out.is_undefined() || out.is_null() {
        return Ok(Value::Null);
    }
    let text = js_sys::JSON::stringify(&out)
        .map_err(message)?
        .as_string()
        .unwrap_or_default();
    serde_json::from_str(&text).map_err(|e| e.to_string())
}
