//! 浏览器本地存储（只放每个人自己的界面偏好，比如面板宽度）。读写失败一律当没有。

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
