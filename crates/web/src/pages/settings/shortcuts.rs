use std::collections::BTreeMap;

use blazar_core_types::keymap::{self, Group};
use leptos::ev;
use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::prefs::{PrefGroup, PrefRow};
use crate::components::{
    dialog::{Choice, ask},
    toast::{flash, toast},
};
use crate::storage;

enum Pressed {
    Ignore,
    Cancel,
    Clear,
    Unmodified,
    Control,
    Combo(String),
}

fn pressed(e: &ev::KeyboardEvent) -> Pressed {
    let code = e.code();
    let modified = e.ctrl_key() || e.meta_key() || e.alt_key() || e.shift_key();
    match code.as_str() {
        "Escape" => return Pressed::Cancel,
        "Backspace" | "Delete" if !modified => return Pressed::Clear,
        "ShiftLeft" | "ShiftRight" | "MetaLeft" | "MetaRight" | "ControlLeft" | "ControlRight"
        | "AltLeft" | "AltRight" => return Pressed::Ignore,
        _ => {}
    }
    let Some(key) = crate::shortcuts::key_of(&code) else {
        return Pressed::Ignore;
    };
    // ⌃ on a Mac belongs to the text fields (line editing), so nothing is bound to it.
    if crate::shortcuts::mac() && e.ctrl_key() {
        return Pressed::Control;
    }
    let function = code.starts_with('F') && code[1..].parse::<u8>().is_ok();
    let mut parts = Vec::new();
    if crate::shortcuts::mod_key(e) {
        parts.push("Mod");
    }
    if e.alt_key() {
        parts.push("Alt");
    }
    if e.shift_key() {
        parts.push("Shift");
    }
    if parts.is_empty() && !function {
        return Pressed::Unmodified;
    }
    parts.push(key);
    Pressed::Combo(parts.join("+"))
}

fn saved_of(map: &Value) -> BTreeMap<String, String> {
    map.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

#[component]
pub(super) fn ShortcutSettings() -> impl IntoView {
    let map = RwSignal::new(storage::load::<Value>("blazar.shortcuts").unwrap_or(json!({})));
    let recording = RwSignal::new(String::new());
    let hint = RwSignal::new(None::<(&'static str, String)>);
    let effective = Memo::new(move |_| map.with(|m| keymap::effective(&saved_of(m))));
    let combo = move |id: &str| {
        effective.with(|list| {
            list.iter()
                .find(|(i, _)| *i == id)
                .map(|(_, c)| c.clone())
                .unwrap_or_default()
        })
    };
    let store = move |id: &str, value: &str| {
        map.update(|m| m[id] = json!(value));
        storage::save("blazar.shortcuts", &map.get_untracked());
        crate::shortcuts::changed();
        recording.set(String::new());
        hint.set(None);
        flash("已保存");
    };
    let modifiers = if crate::shortcuts::mac() {
        "请加上 ⌘ 或 ⌥"
    } else {
        "请加上 Ctrl 或 Alt"
    };
    let row = move |b: &'static keymap::Binding| {
        let (id, label) = (b.id, b.label);
        let caption = move || {
            if recording.get() == id {
                "按下组合键…".to_owned()
            } else {
                let c = combo(id);
                if c.is_empty() {
                    "未设置".to_owned()
                } else {
                    crate::shortcuts::label(&c)
                }
            }
        };
        view! {
            <PrefRow label=label>
                {move || {
                    hint.get()
                        .filter(|(h, _)| *h == id)
                        .map(|(_, m)| {
                            view! {
                                <span class="pref-hint bad" role="alert">
                                    {m}
                                </span>
                            }
                        })
                }}
                <button
                    type="button"
                    class="keycap"
                    class:unset=move || recording.get() != id && combo(id).is_empty()
                    aria-label=format!("更改「{label}」快捷键")
                    aria-pressed=move || (recording.get() == id).to_string()
                    on:click=move |_| {
                        hint.set(None);
                        recording.set(id.into());
                    }
                    on:blur=move |_| {
                        if recording.get_untracked() == id {
                            recording.set(String::new());
                            hint.set(None);
                        }
                    }
                    on:keydown=move |e| {
                        if recording.get_untracked() != id {
                            return;
                        }
                        e.prevent_default();
                        e.stop_propagation();
                        match pressed(&e) {
                            Pressed::Ignore => {}
                            Pressed::Cancel => {
                                recording.set(String::new());
                                hint.set(None);
                            }
                            Pressed::Clear => store(id, ""),
                            Pressed::Unmodified => hint.set(Some((id, modifiers.into()))),
                            Pressed::Control => {
                                hint.set(
                                    Some((
                                        id,
                                        "⌃ 留给输入框编辑文字，请用 ⌘ 或 ⌥"
                                            .into(),
                                    )),
                                );
                            }
                            Pressed::Combo(c) => {
                                let shown = crate::shortcuts::label(&c);
                                let taken = effective
                                    .with_untracked(|list| {
                                        list.iter()
                                            .find(|(other, used)| *other != id && *used == c)
                                            .and_then(|(other, _)| keymap::binding(other))
                                    });
                                if let Some(system) = keymap::reserved(&c) {
                                    hint.set(
                                        Some((id, format!("{shown} 是系统的「{system}」"))),
                                    );
                                } else if let Some(other) = taken {
                                    hint.set(
                                        Some((
                                            id,
                                            format!("{shown} 已被「{}」使用", other.label),
                                        )),
                                    );
                                } else {
                                    store(id, &c);
                                }
                            }
                        }
                    }
                >
                    {caption}
                </button>
            </PrefRow>
        }
    };
    let sheet_key = crate::shortcuts::hint("help");
    let sheet_label = if sheet_key.is_empty() {
        "全部按键".to_owned()
    } else {
        format!("全部按键 {sheet_key}")
    };
    let rows_of = move |g: Group| {
        keymap::BINDINGS
            .iter()
            .filter(|b| b.group == g)
            .map(row)
            .collect_view()
    };
    view! {
        <PrefGroup
            title=Group::Global.title()
            note="点一下快捷键，按下新的组合键；⌫ 清除，Esc 取消。不带 ⌘ / Ctrl 的组合在输入框内不生效。"
            trailing=ViewFn::from(move || {
                view! {
                    <button
                        type="button"
                        class="linkbtn"
                        on:click=|_| crate::shortcuts::fire("act:help")
                    >
                        {sheet_label.clone()}
                    </button>
                }
            })
        >
            {rows_of(Group::Global)}
        </PrefGroup>
        {Group::ALL
            .into_iter()
            .filter(|g| *g != Group::Global && keymap::BINDINGS.iter().any(|b| b.group == *g))
            .map(|g| view! { <PrefGroup title=g.title()>{rows_of(g)}</PrefGroup> })
            .collect_view()}
        <PrefGroup>
            <PrefRow label="恢复默认快捷键" detail="清除所有自定义的组合键。">
                <button
                    type="button"
                    class="btn"
                    on:click=move |_| leptos::task::spawn_local(async move {
                        if ask(
                                "恢复默认快捷键",
                                "自定义快捷键将被清除。",
                                vec![
                                    Choice::plain("保留自定义"),
                                    Choice::danger("恢复默认"),
                                ],
                            )
                            .await != Some(1)
                        {
                            return;
                        }
                        storage::remove("blazar.shortcuts");
                        map.set(json!({}));
                        crate::shortcuts::changed();
                        toast("已恢复默认");
                    })
                >
                    "恢复默认…"
                </button>
            </PrefRow>
        </PrefGroup>
    }
}
