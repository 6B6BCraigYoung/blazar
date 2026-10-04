use serde::Serialize;
use serde::de::DeserializeOwned;

fn store() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

pub fn load<T: DeserializeOwned>(key: &str) -> Option<T> {
    serde_json::from_str(&store()?.get_item(key).ok().flatten()?).ok()
}

pub fn save<T: Serialize>(key: &str, v: &T) {
    if let (Some(s), Ok(text)) = (store(), serde_json::to_string(v)) {
        let _ = s.set_item(key, &text);
    }
}

pub fn load_raw(key: &str) -> Option<String> {
    store()?.get_item(key).ok().flatten()
}

pub fn save_raw(key: &str, v: &str) {
    if let Some(s) = store() {
        let _ = s.set_item(key, v);
    }
}

pub fn remove(key: &str) {
    if let Some(s) = store() {
        let _ = s.remove_item(key);
    }
}
