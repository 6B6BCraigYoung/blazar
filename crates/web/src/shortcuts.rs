use std::collections::BTreeMap;

use blazar_core_types::keymap;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

const STORE: &str = "blazar.shortcuts";

fn saved() -> BTreeMap<String, String> {
    crate::storage::load(STORE).unwrap_or_default()
}

/// Every action with the combo it answers to right now: the user's own binding, else the
/// default (see [`keymap::effective`]).
pub fn effective() -> Vec<(&'static str, String)> {
    keymap::effective(&saved())
}

fn target(e: &web_sys::KeyboardEvent) -> Option<web_sys::Element> {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
}

fn inside(el: Option<&web_sys::Element>, selector: &str) -> bool {
    el.is_some_and(|t| t.closest(selector).ok().flatten().is_some())
}

/// Whether the press carries the app's main modifier: ⌘ on macOS (Ctrl stays with the text
/// field there — ⌃K, ⌃N, ⌃B are line editing), Ctrl or ⌘ elsewhere.
pub fn mod_key(e: &web_sys::KeyboardEvent) -> bool {
    if mac() {
        e.meta_key()
    } else {
        e.meta_key() || e.ctrl_key()
    }
}

/// The key part of a combo for a `KeyboardEvent.code`, or `None` when it can't be bound.
pub fn key_of(code: &str) -> Option<&str> {
    if let Some(s) = code
        .strip_prefix("Key")
        .or_else(|| code.strip_prefix("Digit"))
    {
        return Some(s);
    }
    Some(match code {
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
        "Backspace" => "Backspace",
        "Delete" => "Delete",
        c if c.starts_with('F') && c[1..].parse::<u8>().is_ok() => c,
        _ => return None,
    })
}

pub fn action(e: &web_sys::KeyboardEvent) -> Option<&'static str> {
    if e.is_composing() || e.default_prevented() {
        return None;
    }
    // On a Mac no binding uses Control, so a ⌃ press always belongs to the field.
    if mac() && e.ctrl_key() {
        return None;
    }
    let modified = mod_key(e);
    let at = target(e);
    let editor = inside(at.as_ref(), ".monaco-editor, .xterm");
    if !modified && (editor || inside(at.as_ref(), "input, textarea, select, [contenteditable]")) {
        return None;
    }
    let code = e.code();
    let key = key_of(&code)?;
    let combo = format!(
        "{}{}{}{key}",
        if modified { "Mod+" } else { "" },
        if e.alt_key() { "Alt+" } else { "" },
        if e.shift_key() { "Shift+" } else { "" }
    );
    let id = effective()
        .into_iter()
        .find(|(_, c)| *c == combo)
        .map(|(id, _)| id)?;
    // The editor and the terminal keep their own keys; only the palette reaches past them.
    (!editor || id == "palette").then_some(id)
}

pub fn current(id: &str) -> String {
    effective()
        .into_iter()
        .find(|(i, _)| *i == id)
        .map(|(_, c)| c)
        .unwrap_or_default()
}

pub fn mac() -> bool {
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
        out.push_str(match *key {
            "Enter" => "↩",
            "Backspace" => "⌫",
            "Delete" => "⌦",
            k => k,
        });
        out
    } else {
        mods.iter()
            .map(|m| if *m == "Mod" { "Ctrl" } else { m })
            .chain([*key])
            .collect::<Vec<_>>()
            .join("+")
    }
}

/// The current shortcut of an action as the user sees it, empty when it has none.
pub fn hint(id: &str) -> String {
    label(&current(id))
}

pub fn titled(name: &str, id: &str) -> String {
    let hint = hint(id);
    if hint.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {hint}")
    }
}

/// Tell the app (and the desktop menu) that a binding changed.
pub fn changed() {
    if let Ok(event) = web_sys::Event::new("blazar-shortcuts") {
        let _ = leptos::prelude::window().dispatch_event(&event);
    }
    sync_menu();
}

/// Hand the desktop shell the combos in effect so its menu shows and answers to the same
/// keys. Older shells without the command just ignore it.
pub fn sync_menu() {
    if !crate::desktop::available() {
        return;
    }
    let keys: serde_json::Map<String, Value> = effective()
        .into_iter()
        .map(|(id, c)| (id.to_owned(), Value::String(c)))
        .collect();
    leptos::task::spawn_local(async move {
        let _ = crate::desktop::invoke("set_menu_shortcuts", &json!({ "keys": keys })).await;
    });
}

pub fn on_change(f: impl Fn() + 'static) -> leptos::prelude::WindowListenerHandle {
    leptos::prelude::window_event_listener_untyped("blazar-shortcuts", move |_| f())
}

/// Run a menu command from inside the page, the same way the native menu does.
pub fn fire(id: &str) {
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&id.into());
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("blazar-menu", &init) {
        let _ = leptos::prelude::window().dispatch_event(&event);
    }
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
