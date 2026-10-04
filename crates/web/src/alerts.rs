use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::storage;

#[wasm_bindgen(module = "/js/alerts.js")]
extern "C" {
    #[wasm_bindgen(js_name = playSound)]
    fn play_sound_js(kind: &str, tone: &str, volume: f64);
    #[wasm_bindgen(js_name = notifySupported)]
    pub fn notify_supported() -> bool;
    #[wasm_bindgen(js_name = notifyGranted)]
    pub fn notify_granted() -> bool;
    #[wasm_bindgen(js_name = requestNotify)]
    fn request_notify_js() -> js_sys::Promise;
    #[wasm_bindgen(js_name = notify)]
    fn notify_js(title: &str, body: &str, tag: &str, sticky: bool, on_click: &js_sys::Function);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundPrefs {
    pub on: bool,
    pub tone: String,
    pub volume: f64,
}

impl Default for SoundPrefs {
    fn default() -> Self {
        Self {
            on: false,
            tone: "soft".into(),
            volume: 0.5,
        }
    }
}

pub fn sound_prefs() -> SoundPrefs {
    storage::load("blazar.sound").unwrap_or_default()
}

pub fn set_sound_prefs(p: &SoundPrefs) {
    storage::save("blazar.sound", p);
}

pub fn play(kind: &str, force: bool) {
    let p = sound_prefs();
    if p.on || force {
        play_sound_js(kind, &p.tone, p.volume);
    }
}

pub fn notify_on() -> bool {
    storage::load_raw("blazar.notify").as_deref() == Some("1")
}

pub fn set_notify(on: bool) {
    storage::save_raw("blazar.notify", if on { "1" } else { "0" });
}

pub async fn request_permission() -> String {
    wasm_bindgen_futures::JsFuture::from(request_notify_js())
        .await
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}

pub fn notify(title: &str, body: &str, tag: &str, sticky: bool, on_click: impl Fn() + 'static) {
    let cb = Closure::<dyn Fn()>::new(on_click);
    notify_js(title, body, tag, sticky, cb.as_ref().unchecked_ref());

    cb.forget();
}
