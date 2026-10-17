use std::collections::HashMap;
use wasm_bindgen::JsCast;

const DEFAULTS: &[(&str, &str)] = &[
    ("palette", "Mod+K"),
    ("side", "Mod+\\"),
    ("explorer", "Mod+B"),
    ("chat", "Mod+Alt+B"),
    ("panel", "Mod+J"),
    ("newchat", "Mod+Shift+N"),
    ("diff", "Mod+Shift+D"),
    ("git", "Mod+Shift+G"),
    ("preview", "Mod+Shift+P"),
    ("inbox", "Alt+I"),
    ("tasks", "Alt+T"),
    ("workspaces", "Alt+W"),
];

pub fn action(e: &web_sys::KeyboardEvent) -> Option<&'static str> {
    if e.is_composing() || e.default_prevented() {
        return None;
    }
    let modified = e.meta_key() || e.ctrl_key();
    if !modified
        && e.target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| {
                t.closest("input, textarea, select, [contenteditable], .monaco-editor, .xterm")
                    .ok()
                    .flatten()
                    .is_some()
            })
    {
        return None;
    }
    let code = e.code();
    let key = if let Some(s) = code
        .strip_prefix("Key")
        .or_else(|| code.strip_prefix("Digit"))
    {
        s
    } else {
        match code.as_str() {
            "Backslash" => "\\",
            "Slash" => "/",
            "Comma" => ",",
            "Period" => ".",
            "Semicolon" => ";",
            "Quote" => "'",
            "BracketLeft" => "[",
            "BracketRight" => "]",
            "Minus" => "-",
            "Equal" => "=",
            "Backquote" => "`",
            "Space" => "Space",
            "Enter" => "Enter",
            c if c.starts_with('F') && c[1..].parse::<u8>().is_ok() => c,
            _ => return None,
        }
    };
    let combo = format!(
        "{}{}{}{key}",
        if modified { "Mod+" } else { "" },
        if e.alt_key() { "Alt+" } else { "" },
        if e.shift_key() { "Shift+" } else { "" }
    );
    let saved: HashMap<String, String> =
        crate::storage::load("blazar.shortcuts").unwrap_or_default();
    DEFAULTS
        .iter()
        .find(|(id, default)| saved.get(*id).map(String::as_str).unwrap_or(default) == combo)
        .map(|(id, _)| *id)
}

pub fn current(id: &str) -> String {
    let saved: HashMap<String, String> =
        crate::storage::load("blazar.shortcuts").unwrap_or_default();
    saved.get(id).cloned().unwrap_or_else(|| {
        DEFAULTS
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, d)| (*d).to_owned())
            .unwrap_or_default()
    })
}

fn mac() -> bool {
    web_sys::window()
        .and_then(|w| w.navigator().platform().ok())
        .is_some_and(|p| p.starts_with("Mac"))
}

pub fn label(combo: &str) -> String {
    let parts: Vec<&str> = combo.split('+').filter(|p| !p.is_empty()).collect();
    let Some((key, mods)) = parts.split_last() else {
        return String::new();
    };
    if mac() {
        let mut out: String = [("Alt", "⌥"), ("Shift", "⇧"), ("Mod", "⌘")]
            .iter()
            .filter(|(name, _)| mods.contains(name))
            .map(|(_, symbol)| *symbol)
            .collect();
        out.push_str(if *key == "Enter" { "↩" } else { key });
        out
    } else {
        mods.iter()
            .map(|m| if *m == "Mod" { "Ctrl" } else { m })
            .chain([*key])
            .collect::<Vec<_>>()
            .join("+")
    }
}

pub fn titled(name: &str, id: &str) -> String {
    let hint = label(&current(id));
    if hint.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {hint}")
    }
}

pub fn changed() {
    if let Ok(event) = web_sys::Event::new("blazar-shortcuts") {
        let _ = leptos::prelude::window().dispatch_event(&event);
    }
}

pub fn on_change(f: impl Fn() + 'static) -> leptos::prelude::WindowListenerHandle {
    leptos::prelude::window_event_listener_untyped("blazar-shortcuts", move |_| f())
}

pub fn on_menu(f: impl Fn(&str) + 'static) -> leptos::prelude::WindowListenerHandle {
    leptos::prelude::window_event_listener_untyped("blazar-menu", move |e| {
        if let Some(id) = js_sys::Reflect::get(&e, &"detail".into())
            .ok()
            .and_then(|d| d.as_string())
        {
            f(&id);
        }
    })
}
