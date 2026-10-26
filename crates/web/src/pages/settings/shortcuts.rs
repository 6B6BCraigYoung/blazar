use leptos::ev;
use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::prefs::{PrefGroup, PrefRow};
use crate::components::{
    dialog::{Choice, ask},
    toast::{flash, toast},
};
use crate::storage;

pub const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("palette", "命令面板", "Mod+K"),
    ("side", "收起 / 展开侧栏", "Mod+\\"),
    ("explorer", "文件", "Mod+B"),
    ("chat", "检查器", "Mod+Alt+B"),
    ("panel", "终端", "Mod+J"),
    ("newchat", "新对话", "Mod+T"),
    ("prevchat", "上一个对话", "Mod+Shift+["),
    ("nextchat", "下一个对话", "Mod+Shift+]"),
    ("newws", "新建工作区", "Mod+N"),
    ("inspect", "简介", "Mod+I"),
    ("delete", "删除", "Mod+Backspace"),
    ("copypath", "复制路径", "Mod+Alt+C"),
    ("reveal", "在访达中显示", "Mod+Alt+R"),
    ("diff", "更改", "Mod+Shift+D"),
    ("git", "Git", "Mod+Shift+G"),
    ("preview", "预览", "Mod+Shift+P"),
    ("diffview", "差异：并排 / 统一", "Mod+Alt+S"),
    ("diffws", "差异：忽略空白", "Mod+Alt+W"),
    ("diffwrap", "差异：自动换行", "Alt+Z"),
    ("difflist", "差异：改动文件列表", "Mod+Alt+L"),
    ("difffold", "差异：全部折叠 / 展开", "Mod+Alt+["),
    ("inbox", "收件箱", "Alt+I"),
    ("tasks", "任务", "Alt+T"),
    ("workspaces", "工作区", "Alt+W"),
    ("settings", "设置", "Mod+,"),
];

enum Pressed {
    Ignore,
    Cancel,
    Clear,
    Unmodified,
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
    let key = code
        .strip_prefix("Key")
        .or_else(|| code.strip_prefix("Digit"))
        .unwrap_or(match code.as_str() {
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
            _ => code.as_str(),
        });
    let function = code.starts_with('F') && code[1..].parse::<u8>().is_ok();
    let supported = code.starts_with("Key")
        || code.starts_with("Digit")
        || function
        || matches!(
            code.as_str(),
            "Backslash"
                | "Slash"
                | "Comma"
                | "Period"
                | "Semicolon"
                | "Quote"
                | "BracketLeft"
                | "BracketRight"
                | "Minus"
                | "Equal"
                | "Backquote"
                | "Space"
                | "Enter"
                | "Backspace"
                | "Delete"
        );
    if !supported {
        return Pressed::Ignore;
    }
    let mut parts = Vec::new();
    if e.ctrl_key() || e.meta_key() {
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

#[component]
pub(super) fn ShortcutSettings() -> impl IntoView {
    let map = RwSignal::new(storage::load::<Value>("blazar.shortcuts").unwrap_or(json!({})));
    let recording = RwSignal::new(String::new());
    let hint = RwSignal::new(None::<(&'static str, String)>);
    let combo =
        move |id: &str, default: &str| map.with(|m| m[id].as_str().unwrap_or(default).to_owned());
    let store = move |id: &str, value: &str| {
        map.update(|m| m[id] = json!(value));
        storage::save("blazar.shortcuts", &map.get_untracked());
        crate::shortcuts::changed();
        recording.set(String::new());
        hint.set(None);
        flash("已保存");
    };
    view! {
        <PrefGroup
            note="点一下快捷键，按下新的组合键；⌫ 清除，Esc 取消。不带 ⌘ / Ctrl 的组合在输入框内不生效。"
        >
            {SHORTCUTS
                .iter()
                .map(move |&(id, label, default)| {
                    let caption = move || {
                        if recording.get() == id {
                            "按下组合键…".to_owned()
                        } else {
                            let c = combo(id, default);
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
                                class:unset=move || {
                                    recording.get() != id && combo(id, default).is_empty()
                                }
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
                                        Pressed::Unmodified => {
                                            hint.set(Some((id, "请加上 ⌘ / Ctrl / Alt".into())));
                                        }
                                        Pressed::Combo(c) => {
                                            let taken = SHORTCUTS
                                                .iter()
                                                .find(|&&(other, _, def)| {
                                                    other != id && combo(other, def) == c
                                                });
                                            match taken {
                                                Some(&(_, name, _)) => {
                                                    hint.set(
                                                        Some((
                                                            id,
                                                            format!(
                                                                "{} 已被「{name}」使用",
                                                                crate::shortcuts::label(&c),
                                                            ),
                                                        )),
                                                    );
                                                }
                                                None => store(id, &c),
                                            }
                                        }
                                    }
                                }
                            >
                                {caption}
                            </button>
                        </PrefRow>
                    }
                })
                .collect_view()}
            <PrefRow label="恢复默认快捷键" detail="清除所有自定义的组合键。">
                <button
                    type="button"
                    class="btn"
                    on:click=move |_| leptos::task::spawn_local(async move {
                        if ask(
                                "恢复默认快捷键",
                                "自定义快捷键将被清除。",
                                vec![Choice::plain("保留自定义"), Choice::danger("恢复默认")],
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
