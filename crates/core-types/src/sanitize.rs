use crate::EntryKind;

pub const MAX_TEXT_BYTES: usize = 16 * 1024;

const REDACTED: &str = "«已脱敏»";

#[must_use]
pub fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = s.len() - end;
    format!("{}\n…«已截断，省略 {dropped} 字节»", &s[..end])
}

#[must_use]
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }

    if s.ends_with('\n') {
        out.push('\n');
    }
    out
}

const SECRET_KEYS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "accesskey",
    "access_key",
    "credential",
    "private_key",
    "auth",
    "session_key",
    "cookie",
];

const TOKEN_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "sk_test_",
    "pk_live_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xapp-",
    "AKIA",
    "ASIA",
    "AIza",
    "ya29.",
    "glpat-",
    "dop_v1_",
    "hf_",
    "npm_",
    "SG.",
    "Bearer ",
];

fn redact_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    'outer: while !rest.is_empty() {
        for p in TOKEN_PREFIXES {
            if let Some(pos) = rest.find(p) {
                let start = pos + p.len();
                let tail = &rest[start..];
                let end = tail
                    .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ')' | ';'))
                    .unwrap_or(tail.len());

                if end >= 8 {
                    out.push_str(&rest[..pos]);
                    out.push_str(p);
                    out.push_str(REDACTED);
                    rest = &tail[end..];
                    continue 'outer;
                }
            }
        }
        break;
    }
    out.push_str(rest);

    if let Some(eq) = out.find(['=', ':']) {
        let (k, v) = out.split_at(eq);
        let key = k
            .trim()
            .trim_matches(['"', '\'', '{', ',', ' '])
            .to_ascii_lowercase();
        let looks_secret = SECRET_KEYS
            .iter()
            .any(|w| key.ends_with(w) || key.contains(w));

        let val = v[1..].trim().trim_matches(['"', '\'', ',']);
        if looks_secret && val.len() >= 8 && !val.contains(' ') && !val.starts_with(REDACTED) {
            return format!("{k}{}{REDACTED}", &v[..1]);
        }
    }
    out
}

pub fn sanitize(kind: &mut EntryKind) {
    let clean = |s: &mut String| {
        *s = truncate_utf8(&redact(s), MAX_TEXT_BYTES);
    };
    match kind {
        EntryKind::UserMessage { text }
        | EntryKind::AssistantMessage { text }
        | EntryKind::Thinking { text } => clean(text),
        EntryKind::Error { message }
        | EntryKind::Notice { message }
        | EntryKind::ApiRetry { error: message, .. } => clean(message),
        EntryKind::ToolUse { input, .. } => sanitize_json(input),
        EntryKind::ToolResult {
            content,
            structured,
            images,
            ..
        } => {
            clean(content);
            if let Some(v) = structured {
                drop_base64(v);
                sanitize_json(v);
            }
            for i in images.iter_mut() {
                stash_image(i);
            }
        }
        EntryKind::Approval { request, .. } => sanitize_json(request),
        EntryKind::Finished(crate::Outcome::Success {
            text,
            denied,
            denials,
            ..
        }) => {
            if let Some(t) = text {
                clean(t);
            }

            for d in denied.iter_mut() {
                clean(d);
            }
            for d in denials.iter_mut() {
                sanitize_json(&mut d.input);
            }
        }
        EntryKind::DenialDecided { what, by, .. } => {
            clean(what);
            clean(by);
        }
        EntryKind::BackgroundTask {
            description: Some(d),
            ..
        } => clean(d),
        EntryKind::BackgroundTask { .. } => {}
        EntryKind::InputConsumed { text } => clean(text),
        EntryKind::ApprovalResolved {
            decision: crate::ApprovalDecision::Deny { message },
            ..
        } => clean(message),
        EntryKind::ApprovalResolved { .. } => {}
        EntryKind::Finished(crate::Outcome::Failed { message }) => clean(message),
        EntryKind::Finished(_) => {}
        EntryKind::SessionStarted { .. }
        | EntryKind::TokenUsage(_)
        | EntryKind::RateLimit(_)
        | EntryKind::Progress { .. } => {}
    }
}

/// 把图片的 base64 存起来，返回 blob 名（不存就返回 None）。由 hub 启动时注册；
/// 没注册的进程里图片字节直接丢掉，只留类型和大小。
pub type ImageSink = fn(media_type: &str, base64: &str) -> Option<String>;

static IMAGE_SINK: std::sync::OnceLock<ImageSink> = std::sync::OnceLock::new();

pub fn set_image_sink(sink: ImageSink) {
    let _ = IMAGE_SINK.set(sink);
}

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

fn stash_image(i: &mut crate::ResultImage) {
    if i.data.is_empty() {
        return;
    }
    let data = std::mem::take(&mut i.data);
    if i.size == 0 {
        i.size = (data.len() as u64) / 4 * 3;
    }
    if i.blob.is_none() && i.size <= MAX_IMAGE_BYTES {
        i.blob = IMAGE_SINK.get().and_then(|sink| sink(&i.media_type, &data));
    }
}

/// Claude 的 tool_use_result 里会带整张图的 base64（Read 图片时），截成 16 KB 也没用，
/// 图片已经单独存在 images 里，这里直接去掉。
fn drop_base64(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            let media = map.contains_key("media_type") || map.contains_key("mimeType");
            map.retain(|k, val| {
                let blob = k == "base64" || (media && k == "data");
                !(blob && val.as_str().is_some_and(|s| s.len() > 256))
            });
            for val in map.values_mut() {
                drop_base64(val);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(drop_base64),
        _ => {}
    }
}

fn sanitize_json(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            *s = truncate_utf8(&redact(s), MAX_TEXT_BYTES);
        }
        serde_json::Value::Array(items) => {
            for it in items.iter_mut() {
                sanitize_json(it);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                let key = k.to_ascii_lowercase();
                if SECRET_KEYS.iter().any(|w| key.contains(w))
                    && let serde_json::Value::String(s) = val
                    && s.len() >= 8
                {
                    *s = REDACTED.to_owned();
                    continue;
                }
                sanitize_json(val);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let s = "中文".repeat(100);
        let out = truncate_utf8(&s, 17);
        assert!(out.starts_with("中文中文中"), "{out}");
        assert!(out.contains("已截断"));
        assert_eq!(truncate_utf8("短", 1024), "短", "没超限就不该动它");
    }

    #[test]
    fn prefixed_tokens_are_redacted_wherever_they_appear() {
        let cases = [
            "export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123",
            "curl -H 'Authorization: Bearer sk-ant-api03-xxxxxxxxxxxxxxxx' https://x",
            "aws_access_key_id = AKIAIOSFODNN7EXAMPLE",
            "https://user:glpat-xxxxxxxxxxxxxxxxxxxx@gitlab.example.com/a.git",
        ];
        for c in cases {
            let out = redact(c);
            assert!(out.contains(REDACTED), "没打码: {c} -> {out}");
            assert!(
                !out.contains("abcdefghijklmnop")
                    && !out.contains("IOSFODNN7EXAMPLE")
                    && !out.contains("xxxxxxxxxxxxxxxxxxxx"),
                "秘密仍然可见: {out}"
            );
        }
    }

    #[test]
    fn assignment_form_is_redacted_by_key_name() {
        assert!(redact("API_KEY=s0m3th1ngl0ng").contains(REDACTED));
        assert!(redact(r#"  "password": "hunter22222""#).contains(REDACTED));

        assert_eq!(redact("PATH=/usr/bin:/bin"), "PATH=/usr/bin:/bin");
        assert_eq!(redact("token: 1"), "token: 1");
        assert_eq!(redact("说明：这是一段普通中文"), "说明：这是一段普通中文");
    }

    #[test]
    fn newline_shape_survives() {
        assert_eq!(redact("a\nb\n"), "a\nb\n");
        assert_eq!(redact("a\nb"), "a\nb");
        assert_eq!(redact(""), "");
    }

    #[test]
    fn tool_payloads_are_cleaned_recursively() {
        let mut k = EntryKind::ToolUse {
            id: crate::ToolId("t1".into()),
            name: "Bash".into(),
            input: serde_json::json!({
                "command": "echo $GITHUB_TOKEN",
                "env": { "GITHUB_TOKEN": "ghp_abcdefghijklmnopqrst" },
                "nested": [{ "api_key": "sk-abcdefghijklmnop" }],
                "timeout": 30
            }),
        };
        sanitize(&mut k);
        let s = serde_json::to_string(&k).unwrap();
        assert!(!s.contains("ghp_abcdefghij"), "{s}");
        assert!(!s.contains("sk-abcdefghij"), "{s}");
        assert!(s.contains("\"timeout\":30"), "非字符串字段不该被动: {s}");
    }

    #[test]
    fn oversized_tool_output_is_truncated_with_a_marker() {
        let big = "x".repeat(MAX_TEXT_BYTES * 3);
        let mut k = EntryKind::ToolResult {
            id: crate::ToolId("t1".into()),
            ok: true,
            content: big,
            structured: None,
            images: Vec::new(),
        };
        sanitize(&mut k);
        let EntryKind::ToolResult { content, .. } = &k else {
            unreachable!()
        };
        assert!(
            content.len() < MAX_TEXT_BYTES + 200,
            "没截断: {}",
            content.len()
        );
        assert!(content.contains("已截断"), "截断了要说出来");
    }

    #[test]
    fn numeric_only_events_are_left_alone() {
        let mut k = EntryKind::TokenUsage(crate::TokenUsage {
            input: 1,
            output: 2,
            ..Default::default()
        });
        let before = serde_json::to_string(&k).unwrap();
        sanitize(&mut k);
        assert_eq!(before, serde_json::to_string(&k).unwrap());
    }
}
