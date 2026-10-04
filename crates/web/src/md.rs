use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};

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

pub fn render(src: &str, base: &str, ws: &str) -> String {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let events = Parser::new_ext(src, opts).map(|ev| match ev {
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
    fn insight_markers_become_label_and_rule() {
        let html = render("`★ Insight ─────`\n- a\n\n`─────────`", "", "w1");
        assert!(html.contains(r#"<span class="md-insight">★ Insight</span>"#));
        assert!(html.contains(r#"<span class="md-rule"></span>"#));
        assert!(!html.contains("<code>"));
        let plain = render("★ Insight ─────\n\n- a\n\n─────────", "", "w1");
        assert!(plain.contains("md-insight") && plain.contains("md-rule"));
    }
}
