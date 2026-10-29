//! 对话里的「Copy image」：网页把图片原始字节发过来，这里写一份临时文件，
//! 再把图片数据（PNG / TIFF）和这个文件的 URL 放进同一个剪贴板条目。
//! 微信、飞书取图片数据，访达取文件 URL，都能直接粘贴。

use std::path::PathBuf;

const MAX_BYTES: usize = 20 * 1024 * 1024;
const MAX_NAME: usize = 120;

fn safe_name(raw: &str) -> String {
    let decoded = url::form_urlencoded::parse(format!("n={raw}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    let name: String = decoded
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':'))
        .take(MAX_NAME)
        .collect();
    let name = name.trim().trim_start_matches('.').to_owned();
    if name.is_empty() {
        "image.png".to_owned()
    } else {
        name
    }
}

fn temp_file(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let dir = std::env::temp_dir()
        .join("blazar-clipboard")
        .join(stamp.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, bytes).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    Ok(path)
}

#[tauri::command]
pub fn copy_image(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("图片数据格式不对".into());
    };
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err("图片是空的或超过 20 MB".into());
    }
    let name = safe_name(
        request
            .headers()
            .get("x-image-name")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
    );
    let path = temp_file(&name, bytes)?;
    let url =
        url::Url::from_file_path(&path).map_err(|()| format!("{} 不是绝对路径", path.display()))?;
    mac::put(bytes, url.as_str())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use objc2::AllocAnyThread;
    use objc2_app_kit::{
        NSImage, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeTIFF,
    };
    use objc2_foundation::{NSData, NSString};

    const PNG_MAGIC: &[u8] = b"\x89PNG";

    pub fn put(bytes: &[u8], file_url: &str) -> Result<(), String> {
        let data = NSData::with_bytes(bytes);
        let image = NSImage::initWithData(NSImage::alloc(), &data)
            .ok_or_else(|| "系统读不了这张图片".to_owned())?;
        let tiff = image.TIFFRepresentation();
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        let mut ok = false;
        unsafe {
            if bytes.starts_with(PNG_MAGIC) {
                ok |= board.setData_forType(Some(&data), NSPasteboardTypePNG);
            }
            if let Some(tiff) = tiff.as_deref() {
                ok |= board.setData_forType(Some(tiff), NSPasteboardTypeTIFF);
            }
            board.setString_forType(&NSString::from_str(file_url), NSPasteboardTypeFileURL);
        }
        if ok {
            Ok(())
        } else {
            Err("写剪贴板失败".into())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod mac {
    pub fn put(_bytes: &[u8], _file_url: &str) -> Result<(), String> {
        Err("只有 macOS 桌面端支持复制图片".into())
    }
}

#[cfg(test)]
mod tests {
    use super::safe_name;

    #[test]
    fn names_stay_inside_the_temp_dir() {
        assert_eq!(safe_name("a%20b.png"), "a b.png");
        assert_eq!(safe_name("..%2F..%2Fetc%2Fpasswd"), "passwd");
        assert_eq!(safe_name(""), "image.png");
        assert_eq!(safe_name(".hidden.png"), "hidden.png");
    }
}
