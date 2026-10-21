use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

pub const PERMISSION: &str = "ui-snapshot";
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_NAME: usize = 128;

pub struct Shot {
    png: Vec<u8>,
    width: isize,
    height: isize,
}

pub fn enabled() -> bool {
    std::env::var("BLAZAR_UI_DRIVER").is_ok_and(|v| v == "1")
}

fn file_name(name: Option<&str>, now: Duration) -> Result<String, String> {
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        return Ok(format!("shot-{}.png", now.as_millis()));
    };
    let allowed = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    let stem = name.strip_suffix(".png").unwrap_or_default();
    if !allowed || stem.is_empty() || stem.starts_with('.') || name.len() > MAX_NAME {
        return Err(format!(
            "文件名「{name}」不合法：只能用字母、数字、点、下划线和连字符，以 .png 结尾，不超过 {MAX_NAME} 个字符"
        ));
    }
    Ok(name.to_owned())
}

#[tauri::command]
pub async fn snapshot_webview(
    webview: tauri::Webview,
    name: Option<String>,
) -> Result<Value, String> {
    if !enabled() {
        return Err("界面驱动没有开启，不能截图".into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let name = file_name(name.as_deref(), now)?;
    let shot = tokio::time::timeout(TIMEOUT, capture(&webview))
        .await
        .map_err(|_| {
            format!(
                "{} 秒内没有拿到截图：WebKit 的截图回调没有触发",
                TIMEOUT.as_secs()
            )
        })??;
    let dir = crate::profile::data_dir()
        .map_err(|e| e.to_string())?
        .join("shots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, &shot.png).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    Ok(json!({ "path": path, "width": shot.width, "height": shot.height }))
}

#[cfg(target_os = "macos")]
async fn capture(webview: &tauri::Webview) -> Result<Shot, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    webview
        .with_webview(move |platform| mac::take(platform.inner(), tx))
        .map_err(|e| format!("没法在主线程上截图：{e}"))?;
    rx.await.map_err(|_| "截图回调被丢弃了".to_owned())?
}

#[cfg(not(target_os = "macos"))]
async fn capture(_webview: &tauri::Webview) -> Result<Shot, String> {
    Err("只有 macOS 桌面端支持截图".into())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::cell::Cell;
    use std::ffi::c_void;

    use block2::RcBlock;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use objc2_web_kit::WKWebView;
    use tokio::sync::oneshot::Sender;

    use super::Shot;

    pub fn take(webview: *mut c_void, tx: Sender<Result<Shot, String>>) {
        let Some(view) = (unsafe { webview.cast::<WKWebView>().as_ref() }) else {
            let _ = tx.send(Err("找不到 WKWebView".into()));
            return;
        };
        let tx = Cell::new(Some(tx));
        let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let result = unsafe { encode(image.as_ref(), error.as_ref()) };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &done) };
    }

    fn encode(image: Option<&NSImage>, error: Option<&NSError>) -> Result<Shot, String> {
        if let Some(e) = error {
            return Err(format!("WebKit 截图失败：{}", e.localizedDescription()));
        }
        let image = image.ok_or("WebKit 没有给出图像")?;
        let tiff = image.TIFFRepresentation().ok_or("图像没法转成位图")?;
        let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("图像没法转成位图")?;
        let png = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }
        .ok_or("图像没法编码成 PNG")?;
        Ok(Shot {
            png: png.to_vec(),
            width: rep.pixelsWide(),
            height: rep.pixelsHigh(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Duration = Duration::from_millis(1_700_000_000_123);

    #[test]
    fn missing_or_empty_name_uses_timestamp() {
        assert_eq!(file_name(None, NOW).unwrap(), "shot-1700000000123.png");
        assert_eq!(file_name(Some(""), NOW).unwrap(), "shot-1700000000123.png");
    }

    #[test]
    fn accepts_plain_png_names() {
        for name in ["settings-dark.png", "home_1.png", "a.b-C_9.png"] {
            assert_eq!(file_name(Some(name), NOW).unwrap(), name);
        }
    }

    #[test]
    fn rejects_bad_names() {
        let long = format!("{}.png", "a".repeat(MAX_NAME));
        for name in [
            "shot",
            "shot.jpg",
            "shot.PNG",
            ".png",
            "..png",
            ".hidden.png",
            "../x.png",
            "a/b.png",
            "a\\b.png",
            "a b.png",
            "截图.png",
            "x.png\0",
            long.as_str(),
        ] {
            assert!(file_name(Some(name), NOW).is_err(), "{name:?} 应被拒绝");
        }
    }
}
