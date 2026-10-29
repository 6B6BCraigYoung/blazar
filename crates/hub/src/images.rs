//! 工具结果里的图片：入库前从事件里拿出来，按内容哈希存成文件，事件里只留哈希。
//! 这样事件表不会被几 MB 的 base64 撑大，历史加载也不用把图片带上。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use axum::extract::Path as UrlPath;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

static DIR: OnceLock<PathBuf> = OnceLock::new();

const TYPES: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

pub fn install(db_path: &Path) {
    let dir = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("images");
    if DIR.set(dir).is_ok() {
        blazar_core_types::sanitize::set_image_sink(store);
    }
}

fn store(media_type: &str, b64: &str) -> Option<String> {
    use base64::Engine;
    use sha2::Digest;
    let ext = TYPES.iter().find(|(t, _)| *t == media_type)?.1;
    let dir = DIR.get()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let hash = sha2::Sha256::digest(&bytes);
    let name = format!(
        "{}.{ext}",
        hash.iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let path = dir.join(&name);
    if !path.exists() {
        std::fs::create_dir_all(dir).ok()?;
        let tmp = dir.join(format!(".{name}.part"));
        std::fs::write(&tmp, &bytes).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
    }
    Some(name)
}

fn valid(name: &str) -> Option<&'static str> {
    let (stem, ext) = name.split_once('.')?;
    if stem.len() != 32 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    TYPES.iter().find(|(_, e)| *e == ext).map(|(t, _)| *t)
}

pub async fn serve(UrlPath(name): UrlPath<String>) -> Response {
    let (Some(ctype), Some(dir)) = (valid(&name), DIR.get()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(dir.join(&name)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, ctype),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_hashes_with_known_extensions() {
        assert_eq!(
            valid("0123456789abcdef0123456789abcdef.png"),
            Some("image/png")
        );
        assert_eq!(valid("../etc/passwd.png"), None);
        assert_eq!(valid("0123456789abcdef0123456789abcdef.svg"), None);
    }
}
