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
        "chip" => {
            r#"<rect x="7" y="7" width="10" height="10" rx="2"/><path d="M9 3v4M15 3v4M9 17v4M15 17v4M3 9h4M3 15h4M17 9h4M17 15h4"/>"#
        }
        "chat" => {
            r#"<path d="M5 5h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-7l-4 3.5V17H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z"/>"#
        }
        "tick" => r#"<path d="m5 12.5 4.5 4.5L19 7"/>"#,
        "chev" => r#"<path d="M6 9l6 6 6-6"/>"#,
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

pub fn model_name(id: &str) -> String {
    let (base, wide) = id.strip_suffix("[1m]").map_or((id, false), |b| (b, true));
    let Some(rest) = base.strip_prefix("claude-").or_else(|| {
        ["opus", "sonnet", "haiku", "fable"]
            .contains(&base)
            .then_some(base)
    }) else {
        return id.to_owned();
    };
    let mut family = String::new();
    let mut version = Vec::new();
    for part in rest.split('-') {
        if part.chars().all(|c| c.is_ascii_digit()) {
            if part.len() < 4 {
                version.push(part);
            }
        } else if family.is_empty() {
            let mut c = part.chars();
            family = c
                .next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default();
        }
    }
    let mut out = if version.is_empty() {
        family
    } else {
        format!("{family} {}", version.join("."))
    };
    if wide {
        out.push_str(" (1M context)");
    }
    out
}

pub(super) fn eff_label(e: &str) -> String {
    EFF_LABEL
        .iter()
        .find(|x| x.0 == e)
        .map_or_else(|| e.to_owned(), |x| x.1.to_owned())
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
