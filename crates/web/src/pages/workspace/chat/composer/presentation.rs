use crate::storage;

pub(super) fn svg(path: &str) -> String {
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{path}</svg>"#
    )
}

pub(super) fn icon(k: &str) -> String {
    svg(match k {
        "hand" => {
            r#"<path d="M8 12V6.5a1.5 1.5 0 0 1 3 0V11M11 10V5a1.5 1.5 0 0 1 3 0v6M14 10V6.5a1.5 1.5 0 0 1 3 0V14a6 6 0 0 1-6 6h-.5a6 6 0 0 1-4.9-2.5L3.8 14a1.5 1.5 0 0 1 2.4-1.8L8 14"/>"#
        }
        "code" => r#"<path d="M9 8l-4 4 4 4M15 8l4 4-4 4"/>"#,
        "plan" => r#"<path d="M4 5h16v14H4z"/><path d="M8 15l3-3 2 2 3-4"/>"#,
        "bolt" => r#"<path d="M13 3L5 14h6l-1 7 8-11h-6z"/>"#,
        "warn" => r#"<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17v.5"/>"#,
        "up" => r#"<path d="M12 16V4M7 9l5-5 5 5"/><path d="M4 16v4h16v-4"/>"#,
        "file" => r#"<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4M9 13h6M9 17h4"/>"#,
        "plus" => r#"<path d="M12 5v14M5 12h14"/>"#,
        "slash" => r#"<rect x="3.5" y="3.5" width="17" height="17" rx="3"/><path d="M14 8l-4 8"/>"#,
        "send" => r#"<path d="M12 19V5M5.5 11.5 12 5l6.5 6.5"/>"#,
        "plug" => r#"<path d="M9 7V3M15 7V3M7 7h10v4a5 5 0 0 1-10 0z"/><path d="M12 16v5"/>"#,
        _ => "",
    })
}

const EFF_LABEL: [(&str, &str); 7] = [
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra high"),
    ("max", "Max"),
    ("ultra", "Ultra"),
];

pub(super) fn eff_label(e: &str) -> String {
    EFF_LABEL
        .iter()
        .find(|x| x.0 == e)
        .map_or_else(|| e.to_owned(), |x| x.1.to_owned())
}

pub(super) fn fmt_k(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub(super) fn fmt_dur(sec: u64) -> String {
    if sec < 60 {
        format!("{sec}s")
    } else if sec < 3600 {
        format!("{}m {}s", sec / 60, sec % 60)
    } else {
        format!("{}h {}m", sec / 3600, sec % 3600 / 60)
    }
}

pub(super) fn editor_url(node: &str, path: &str, line: Option<u32>) -> (String, String) {
    let k = storage::load_raw("blazar.editor").unwrap_or_else(|| "vscode".into());
    let (label, scheme) = match k.as_str() {
        "cursor" => ("Cursor", "cursor"),
        "windsurf" => ("Windsurf", "windsurf"),
        "insiders" => ("VS Code Insiders", "vscode-insiders"),
        _ => ("VS Code", "vscode"),
    };
    let p: String = path
        .split('/')
        .map(|s| String::from(js_sys::encode_uri_component(s)))
        .collect::<Vec<_>>()
        .join("/");
    let tail = line.map(|l| format!(":{l}")).unwrap_or_default();
    let url = if node == "local" {
        format!("{scheme}://file{p}{tail}")
    } else {
        format!(
            "{scheme}://vscode-remote/ssh-remote+{}{p}{tail}",
            js_sys::encode_uri_component(node)
        )
    };
    (label.to_owned(), url)
}
