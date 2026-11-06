#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Image,
    Video,
    Audio,
    Pdf,
}

fn ext(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// What the file viewer opens a path as; `None` stays in the code editor.
pub fn kind_of(path: &str) -> Option<Media> {
    Some(match mime_of(path)?.split('/').next()? {
        "image" => Media::Image,
        "video" => Media::Video,
        "audio" => Media::Audio,
        _ => Media::Pdf,
    })
}

/// The type the hub serves the file as, matching its `raw` endpoint.
pub fn mime_of(path: &str) -> Option<&'static str> {
    Some(match ext(path)?.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "ogv" => "video/ogg",
        "avi" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// 「MP4 视频」「PNG 图片」 for the viewer's file line.
pub fn label_of(path: &str) -> String {
    let e = ext(path).unwrap_or_default().to_ascii_uppercase();
    let what = match kind_of(path) {
        Some(Media::Image) => "图片",
        Some(Media::Video) => "视频",
        Some(Media::Audio) => "音频",
        Some(Media::Pdf) => "文档",
        None => "文件",
    };
    format!("{e} {what}")
}

pub fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// A code span the agent wrote that names a media file, like `out/demo.mp4`.
pub fn mentioned(text: &str) -> Option<&str> {
    let t = text.trim();
    let plain = !t.is_empty()
        && t.len() < 512
        && !t.contains(char::is_whitespace)
        && !t.contains("://")
        && !t.contains(['*', '?', '<', '>', '|', '"', '`']);
    (plain && kind_of(t).is_some()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_open_as_the_right_kind() {
        assert_eq!(kind_of("figs/a.PNG"), Some(Media::Image));
        assert_eq!(kind_of("videos/run.mov"), Some(Media::Video));
        assert_eq!(kind_of("take.flac"), Some(Media::Audio));
        assert_eq!(kind_of("paper.pdf"), Some(Media::Pdf));
        assert_eq!(kind_of("old.avi"), Some(Media::Video));
        assert_eq!(kind_of("src/main.rs"), None);
        assert_eq!(kind_of(".png"), None);
        assert_eq!(kind_of("dir.mp4/notes"), None);
        assert_eq!(mime_of("a.m4a"), Some("audio/mp4"));
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(label_of("a/b.mp4"), "MP4 视频");
        assert_eq!(label_of("x.svg"), "SVG 图片");
        assert_eq!(size_label(512), "512 B");
        assert_eq!(size_label(1536), "1.5 KB");
        assert_eq!(size_label(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(size_label(300 * 1024 * 1024), "300 MB");
        assert_eq!(size_label(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn code_spans_naming_media_are_picked_up() {
        assert_eq!(mentioned("videos/out.mp4"), Some("videos/out.mp4"));
        assert_eq!(mentioned(" /tmp/shot.png "), Some("/tmp/shot.png"));
        assert_eq!(mentioned("ffmpeg -i a.mp4"), None);
        assert_eq!(mentioned("https://x.invalid/a.mp4"), None);
        assert_eq!(mentioned("*.png"), None);
        assert_eq!(mentioned("main.rs"), None);
    }
}
