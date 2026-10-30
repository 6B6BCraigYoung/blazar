use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd};

pub const OPEN_PREFIX: &str = "#open:";

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

/// 返回 (img 的 src, 「Copy path」复制的内容)。src 为空表示不支持，直接显示占位。
fn image(dest: &str, base: &str, ws: &str) -> (String, String) {
    let h = dest.trim();
    if h.starts_with("https://") || h.starts_with("http://") {
        return (h.to_owned(), h.to_owned());
    }
    if h.starts_with("data:image/") && !h.starts_with("data:image/svg") {
        return (h.to_owned(), String::new());
    }
    let local = h.strip_prefix("file://").unwrap_or(h);
    if local.starts_with('/') {
        let p = js_free_decode(local);
        return (raw_url(ws, &p), p);
    }
    if has_scheme(h) {
        return (String::new(), h.to_owned());
    }
    resolve(h, base).map_or_else(|| (String::new(), h.to_owned()), |p| (raw_url(ws, &p), p))
}

fn raw_url(ws: &str, path: &str) -> String {
    if ws.is_empty() {
        return String::new();
    }
    format!("/api/workspaces/{ws}/raw?path={}", encode_component(path))
}

/// file:// 链接里的 %20 之类；解不开就原样用。
fn js_free_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|x| u8::from_str_radix(x, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn attr(s: &str) -> String {
    escape(s).replace('"', "&quot;")
}

/// 对话里的一张图：居中、懒加载，悬停出现复制按钮；点击、失败占位和复制由 js/images.js 处理。
#[must_use]
pub fn figure(src: &str, path: &str, name: &str, alt: &str, ws: &str, big: bool) -> String {
    let name = if name.is_empty() {
        path.split(['?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .filter(|n| !n.is_empty() && !n.starts_with("data:"))
            .unwrap_or("image")
    } else {
        name
    };
    let state = if big {
        " data-state=\"big\""
    } else if src.is_empty() {
        " data-state=\"failed\""
    } else {
        ""
    };
    let img = if src.is_empty() {
        String::new()
    } else {
        format!(
            "<img src=\"{}\" alt=\"{}\" loading=\"lazy\" decoding=\"async\">",
            attr(src),
            attr(if alt.is_empty() { name } else { alt })
        )
    };
    let path_btn = if path.is_empty() {
        ""
    } else {
        "<button type=\"button\" data-img=\"path\">Copy path</button>"
    };
    format!(
        "<span class=\"cc-img\"{state} data-path=\"{}\" data-name=\"{}\" data-ws=\"{}\">{img}\
<span class=\"cc-imgbar\"><button type=\"button\" data-img=\"copy\">Copy image</button>{path_btn}</span>\
<span class=\"cc-imgalt\"></span></span>",
        attr(path),
        attr(name),
        attr(ws)
    )
}

pub fn render(src: &str, base: &str, ws: &str) -> String {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut pic: Option<(String, String, String)> = None;
    let events = Parser::new_ext(src, opts).filter_map(|ev| {
        if let Some((_, _, alt)) = pic.as_mut() {
            return match ev {
                Event::End(TagEnd::Image) => {
                    let (s, p, alt) = pic.take().unwrap_or_default();
                    Some(Event::InlineHtml(CowStr::from(figure(
                        &s, &p, "", &alt, ws, false,
                    ))))
                }
                Event::Text(t) | Event::Code(t) => {
                    alt.push_str(&t);
                    None
                }
                _ => None,
            };
        }
        Some(match ev {
            Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),

            Event::Code(c) | Event::Text(c) if insight(&c).is_some() => {
                Event::InlineHtml(CowStr::from(insight(&c).unwrap_or_default()))
            }
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
            Event::Start(Tag::Image { dest_url, .. }) => {
                let (s, p) = image(&dest_url, base, ws);
                pic = Some((s, p, String::new()));
                return None;
            }
            e => e,
        })
    });
    let mut out = String::with_capacity(src.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut out, events);
    out.replace("<table>", "<div class=\"md-table\"><table>")
        .replace("</table>", "</table></div>")
}

fn insight(c: &str) -> Option<String> {
    let t = c.trim();
    if t.starts_with('★') && (t.contains('─') || t.chars().count() < 40) {
        let label = t.trim_end_matches(['─', ' ']);
        return Some(format!(
            "<span class=\"md-insight\">{}</span>",
            escape(label)
        ));
    }
    (t.chars().count() >= 3 && t.chars().all(|ch| ch == '─'))
        .then(|| "<span class=\"md-rule\"></span>".to_owned())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

    #[test]
    fn images_become_figures() {
        let html = render("![shot](</tmp/a b.png>) ![](https://x.io/c.jpg)", "", "w1");
        assert!(html.contains(r#"class="cc-img""#));
        assert!(html.contains("raw?path=%2Ftmp%2Fa%20b.png"), "{html}");
        assert!(html.contains(r#"data-path="/tmp/a b.png""#), "{html}");
        assert!(html.contains(r#"data-name="c.jpg""#));
        assert!(html.contains(r#"alt="shot""#));
        let bad = render("![x](javascript:alert(1))", "", "w1");
        assert!(bad.contains(r#"data-state="failed""#) && !bad.contains("<img"));
    }

    #[test]
    fn insight_markers_become_label_and_rule() {
        let html = render("`★ Insight ─────`\n- a\n\n`─────────`", "", "w1");
        assert!(html.contains(r#"<span class="md-insight">★ Insight</span>"#));
        assert!(html.contains(r#"<span class="md-rule"></span>"#));
        assert!(!html.contains("<code>"));
        let plain = render("★ Insight ─────\n\n- a\n\n─────────", "", "w1");
        assert!(plain.contains("md-insight") && plain.contains("md-rule"));
    }
}
