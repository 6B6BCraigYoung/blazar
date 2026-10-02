//! Markdown 渲染（pulldown-cmark）。原始 HTML 一律当文本显示，不执行；
//! 工作区里的相对链接改成 `#open:<路径>`，由页面拦截后在编辑器里打开；相对图片走 hub 的 /raw 接口。

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};

pub const OPEN_PREFIX: &str = "#open:";

/// 把 `href` 按所在文件 `base`（工作区内相对路径）解析成工作区内路径；越出根目录返回 None。
fn resolve(href: &str, base: &str) -> Option<String> {
    let dir = base.rsplit_once('/').map_or("", |(d, _)| d);
    let joined = if let Some(abs) = href.strip_prefix('/') {
        abs.to_owned()
    } else if dir.is_empty() {
        href.to_owned()
    } else {
        format!("{dir}/{href}")
    };
    let joined = joined.split('#').next().unwrap_or("");
    let mut out: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

fn has_scheme(h: &str) -> bool {
    h.starts_with("//")
        || h.split_once(':').is_some_and(|(s, _)| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        })
}

fn link(dest: &str, base: &str) -> String {
    let h = dest.trim();
    if h.starts_with("http://")
        || h.starts_with("https://")
        || h.starts_with("mailto:")
        || h.starts_with('#')
    {
        h.to_owned()
    } else if has_scheme(h) {
        // javascript: 之类的一律不要。
        "#".to_owned()
    } else {
        resolve(h, base).map_or_else(|| "#".to_owned(), |p| format!("{OPEN_PREFIX}{p}"))
    }
}

fn encode_component(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn image(dest: &str, base: &str, ws: &str) -> String {
    let h = dest.trim();
    if h.starts_with("https://") || h.starts_with("http://") {
        return h.to_owned();
    }
    if has_scheme(h) {
        return String::new();
    }
    resolve(h, base).map_or_else(String::new, |p| {
        format!("/api/workspaces/{ws}/raw?path={}", encode_component(&p))
    })
}

/// `base` 是这篇 Markdown 在工作区里的路径（解析相对链接用），`ws` 是工作区 id。
pub fn render(src: &str, base: &str, ws: &str) -> String {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let events = Parser::new_ext(src, opts).map(|ev| match ev {
        Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Link {
            link_type,
            dest_url: CowStr::from(link(&dest_url, base)),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => Event::Start(Tag::Image {
            link_type,
            dest_url: CowStr::from(image(&dest_url, base, ws)),
            title,
            id,
        }),
        e => e,
    });
    let mut out = String::with_capacity(src.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_links_inside_workspace() {
        assert_eq!(resolve("b.md", "docs/a.md").as_deref(), Some("docs/b.md"));
        assert_eq!(resolve("../x.rs", "docs/a.md").as_deref(), Some("x.rs"));
        assert_eq!(resolve("../../x.rs", "docs/a.md"), None);
        assert_eq!(link("javascript:alert(1)", "a.md"), "#");
        assert_eq!(link("b.md", "docs/a.md"), "#open:docs/b.md");
    }

    #[test]
    fn raw_html_is_shown_as_text() {
        let html = render(
            "<script>x</script>\n\n![p](<img/a b.png>)",
            "README.md",
            "w1",
        );
        assert!(!html.contains("<script>"));
        assert!(html.contains("/api/workspaces/w1/raw?path=img%2Fa%20b.png"));
    }
}
