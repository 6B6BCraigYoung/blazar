//! 可自定义快捷键，沿用旧界面的浏览器存储。
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
