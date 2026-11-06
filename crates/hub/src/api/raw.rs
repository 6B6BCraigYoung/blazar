use super::workspaces::{download_body, download_refused, raw_target};
use super::*;
use axum::http::{HeaderMap, HeaderValue, header};

const IMAGE_MAX: u64 = blazar_core_types::sanitize::MAX_IMAGE_BYTES;

/// The most one ranged request sends back; players ask again for the rest.
pub(crate) const SEGMENT_MAX: u64 = 8 * 1024 * 1024;

const SANDBOX: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; sandbox";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Image,
    Video,
    Audio,
    Pdf,
}

pub(crate) fn media_type(path: &str) -> Option<(&'static str, Class)> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ("image/png", Class::Image),
        "jpg" | "jpeg" => ("image/jpeg", Class::Image),
        "gif" => ("image/gif", Class::Image),
        "webp" => ("image/webp", Class::Image),
        "svg" => ("image/svg+xml", Class::Image),
        "ico" => ("image/x-icon", Class::Image),
        "bmp" => ("image/bmp", Class::Image),
        "avif" => ("image/avif", Class::Image),
        "mp4" | "m4v" => ("video/mp4", Class::Video),
        "mov" => ("video/quicktime", Class::Video),
        "webm" => ("video/webm", Class::Video),
        "mkv" => ("video/x-matroska", Class::Video),
        "ogv" => ("video/ogg", Class::Video),
        "avi" => ("video/x-msvideo", Class::Video),
        "wmv" => ("video/x-ms-wmv", Class::Video),
        "flv" => ("video/x-flv", Class::Video),
        "wav" => ("audio/wav", Class::Audio),
        "mp3" => ("audio/mpeg", Class::Audio),
        "ogg" | "oga" | "opus" => ("audio/ogg", Class::Audio),
        "flac" => ("audio/flac", Class::Audio),
        "m4a" => ("audio/mp4", Class::Audio),
        "aac" => ("audio/aac", Class::Audio),
        "pdf" => ("application/pdf", Class::Pdf),
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ranged {
    Whole,
    Part { start: u64, len: u64 },
    Unsatisfiable,
}

/// Reads a `Range` header against a file of `size` bytes, sending at most `cap` bytes back.
/// A header that doesn't parse, or asks for several ranges, is ignored and the whole file goes.
pub(crate) fn parse_range(header: Option<&str>, size: u64, cap: u64) -> Ranged {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Ranged::Whole;
    };
    if spec.contains(',') {
        return Ranged::Whole;
    }
    let Some((a, b)) = spec.trim().split_once('-') else {
        return Ranged::Whole;
    };
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ranged::Whole,
        (true, false) => {
            let Ok(n) = b.parse::<u64>() else {
                return Ranged::Whole;
            };
            if n == 0 || size == 0 {
                return Ranged::Unsatisfiable;
            }
            (size.saturating_sub(n), size - 1)
        }
        (false, _) => {
            let Ok(start) = a.parse::<u64>() else {
                return Ranged::Whole;
            };
            let end = if b.is_empty() {
                u64::MAX
            } else {
                match b.parse::<u64>() {
                    Ok(end) if end >= start => end,
                    _ => return Ranged::Whole,
                }
            };
            if start >= size {
                return Ranged::Unsatisfiable;
            }
            (start, end.min(size - 1))
        }
    };
    Ranged::Part {
        start,
        len: (end - start + 1).min(cap),
    }
}

pub async fn workspace_raw(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
    headers: HeaderMap,
) -> Response {
    let Some((ctype, class)) = media_type(&q.path) else {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "这种文件不能直接预览").into_response();
    };
    let (node, root) = match locate(&st, &id).await {
        Ok(v) => v,
        Err(e) => return ApiError(e).into_response(),
    };
    let (root, rel) = raw_target(&root, &q.path);
    let vfs = vfs_for(&st, &node, &root);
    let max = (class == Class::Image).then_some(IMAGE_MAX);
    let size = match vfs.file_size(&rel, max).await {
        Ok(size) => size,
        Err(e) => return download_refused(&e).into_response(),
    };
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let (status, start, len) = match parse_range(range, size, SEGMENT_MAX) {
        Ranged::Unsatisfiable => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [
                    (header::CONTENT_RANGE, format!("bytes */{size}")),
                    (header::ACCEPT_RANGES, "bytes".to_owned()),
                ],
            )
                .into_response();
        }
        Ranged::Whole => (StatusCode::OK, 0, size),
        Ranged::Part { start, len } => (StatusCode::PARTIAL_CONTENT, start, len),
    };
    let download = match vfs.open_range(&rel, start, len).await {
        Ok(d) => d,
        Err(e) => return download_refused(&e).into_response(),
    };
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, ctype)
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_DISPOSITION, "inline")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "private, max-age=30");
    if class != Class::Pdf {
        response = response.header(header::CONTENT_SECURITY_POLICY, SANDBOX);
    }
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{}/{size}", start + len - 1))
                .unwrap_or(HeaderValue::from_static("bytes */0")),
        );
    }
    response
        .body(axum::body::Body::from_stream(download_body(download, len)))
        .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_read_like_browsers_send_them() {
        let p = |h: &str| parse_range(Some(h), 1000, 300);
        assert_eq!(p("bytes=0-99"), Ranged::Part { start: 0, len: 100 });
        assert_eq!(
            p("bytes=900-"),
            Ranged::Part {
                start: 900,
                len: 100
            }
        );
        assert_eq!(
            p("bytes=0-"),
            Ranged::Part { start: 0, len: 300 },
            "capped to one segment"
        );
        assert_eq!(
            p("bytes=950-5000"),
            Ranged::Part {
                start: 950,
                len: 50
            }
        );
        assert_eq!(
            p("bytes=-200"),
            Ranged::Part {
                start: 800,
                len: 200
            }
        );
        assert_eq!(p("bytes=-5000"), Ranged::Part { start: 0, len: 300 });
        assert_eq!(p("bytes=1000-"), Ranged::Unsatisfiable);
        assert_eq!(p("bytes=-0"), Ranged::Unsatisfiable);
        assert_eq!(p("bytes=5-1"), Ranged::Whole);
        assert_eq!(p("bytes=0-1,5-9"), Ranged::Whole);
        assert_eq!(p("items=0-1"), Ranged::Whole);
        assert_eq!(p("bytes=x-"), Ranged::Whole);
        assert_eq!(parse_range(None, 1000, 300), Ranged::Whole);
        assert_eq!(parse_range(Some("bytes=0-"), 0, 300), Ranged::Unsatisfiable);
    }

    #[test]
    fn media_types_cover_what_the_viewer_opens() {
        assert_eq!(media_type("a/b.PNG"), Some(("image/png", Class::Image)));
        assert_eq!(media_type("clip.mov").map(|m| m.1), Some(Class::Video));
        assert_eq!(media_type("take.flac").map(|m| m.1), Some(Class::Audio));
        assert_eq!(media_type("paper.pdf").map(|m| m.1), Some(Class::Pdf));
        assert_eq!(media_type("main.rs"), None);
        assert_eq!(media_type("Makefile"), None);
    }

    async fn workspace(dir: &std::path::Path) -> (Shared, String) {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.join("mesh")),
            crate::services::Services::Isolated,
        );
        let now = Utc::now().to_rfc3339();
        let node = super::super::workspaces::ensure_node(&st, "local", &now)
            .await
            .unwrap_or_else(|e| panic!("{}", e.message()));
        let id = WorkspaceId::new().to_string();
        sqlx::query(
            "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, ?2, 'w', ?3, ?4)",
        )
        .bind(&id)
        .bind(&node)
        .bind(dir.join("ws").to_string_lossy().into_owned())
        .bind(&now)
        .execute(st.db.pool())
        .await
        .unwrap();
        (st, id)
    }

    async fn get(
        st: &Shared,
        id: &str,
        path: &str,
        range: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut headers = HeaderMap::new();
        if let Some(r) = range {
            headers.insert(header::RANGE, r.parse().unwrap());
        }
        let r = workspace_raw(
            State(st.clone()),
            Path(id.to_owned()),
            Query(FileQuery { path: path.into() }),
            headers,
        )
        .await;
        let (parts, body) = r.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .unwrap_or_default();
        (parts.status, parts.headers, bytes.to_vec())
    }

    #[tokio::test]
    async fn a_ranged_request_gets_206_with_just_that_piece() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ws")).unwrap();
        let bytes: Vec<u8> = (0..50_000u32).map(|i| (i % 253) as u8).collect();
        std::fs::write(dir.path().join("ws/clip.mp4"), &bytes).unwrap();
        std::fs::write(dir.path().join("ws/notes.txt"), "hi").unwrap();
        let (st, id) = workspace(dir.path()).await;

        let (status, h, body) = get(&st, &id, "clip.mp4", Some("bytes=1000-1999")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(h[header::CONTENT_RANGE], "bytes 1000-1999/50000");
        assert_eq!(h[header::ACCEPT_RANGES], "bytes");
        assert_eq!(h[header::CONTENT_LENGTH], "1000");
        assert_eq!(h[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(body, bytes[1000..2000]);

        let (status, h, body) = get(&st, &id, "clip.mp4", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(h[header::ACCEPT_RANGES], "bytes");
        assert_eq!(body.len(), 50_000);

        let (status, h, _) = get(&st, &id, "clip.mp4", Some("bytes=60000-")).await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(h[header::CONTENT_RANGE], "bytes */50000");

        let (status, _, _) = get(&st, &id, "notes.txt", None).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let (status, _, _) = get(&st, &id, "missing.mp4", Some("bytes=0-")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
