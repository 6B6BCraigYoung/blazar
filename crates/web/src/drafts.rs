//! 没存盘的编辑在本地存一份草稿：页面重新载入、连接断开都不丢。
//! 纯逻辑，原生也能编译，方便单测；读写 localStorage 在 `pages::workspace::files`。

use serde::{Deserialize, Serialize};

const PREFIX: &str = "blazar.unsaved.";

#[must_use]
pub fn key(ws: &str, path: &str) -> String {
    format!("{PREFIX}{ws}.{path}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    /// 写下草稿的时间（ISO 8601），给“几分钟前”用。
    #[serde(default)]
    pub at: String,
}

/// 草稿太大就不存：localStorage 总共只有几 MB，别把别的设置挤掉。
pub const MAX_CHARS: usize = 1_000_000;

#[must_use]
pub fn worth_keeping(text: &str) -> bool {
    text.len() <= MAX_CHARS
}

/// 打开文件时要不要提示“恢复未保存的改动”：有草稿，而且和磁盘上的内容不一样。
#[must_use]
pub fn offer(draft: Option<&Draft>, on_disk: &str) -> bool {
    draft.is_some_and(|d| d.text != on_disk)
}

/// 定时存草稿时，只有内容变了才写。
#[must_use]
pub fn changed(last_written: Option<&str>, now: &str) -> bool {
    last_written != Some(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_per_workspace_and_file() {
        assert_eq!(key("w1", "src/a.rs"), "blazar.unsaved.w1.src/a.rs");
        assert_ne!(key("w1", "a"), key("w2", "a"));
    }

    #[test]
    fn a_draft_is_offered_only_when_it_differs_from_disk() {
        let d = Draft {
            text: "edited".into(),
            at: String::new(),
        };
        assert!(offer(Some(&d), "original"));
        assert!(!offer(Some(&d), "edited"));
        assert!(!offer(None, "original"));
    }

    #[test]
    fn drafts_round_trip_and_tolerate_missing_time() {
        let d = Draft {
            text: "x".into(),
            at: "2026-10-30T00:00:00Z".into(),
        };
        let back: Draft = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        let old: Draft = serde_json::from_str(r#"{"text":"y"}"#).unwrap();
        assert_eq!(old.text, "y");
        assert!(old.at.is_empty());
    }

    #[test]
    fn only_changed_text_is_rewritten_and_huge_buffers_are_skipped() {
        assert!(changed(None, "a"));
        assert!(!changed(Some("a"), "a"));
        assert!(changed(Some("a"), "b"));
        assert!(worth_keeping("small"));
        assert!(!worth_keeping(&"x".repeat(MAX_CHARS + 1)));
    }
}
