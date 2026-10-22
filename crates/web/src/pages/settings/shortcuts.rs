use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::status::EmptyState;
use crate::components::{
    dialog::{Choice, ask},
    toast::toast,
};
use crate::storage;

pub const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("palette", "命令面板", "Mod+K"),
    ("side", "收起 / 展开侧栏", "Mod+\\"),
    ("explorer", "文件", "Mod+B"),
    ("chat", "检查器", "Mod+Alt+B"),
    ("panel", "终端", "Mod+J"),
    ("newchat", "新对话", "Mod+T"),
    ("newws", "新建工作区", "Mod+N"),
    ("diff", "更改", "Mod+Shift+D"),
    ("git", "Git", "Mod+Shift+G"),
    ("preview", "预览", "Mod+Shift+P"),
    ("inbox", "收件箱", "Alt+I"),
    ("tasks", "任务", "Alt+T"),
    ("workspaces", "工作区", "Alt+W"),
];

#[component]
pub(super) fn ShortcutSettings() -> impl IntoView {
    let map = RwSignal::new(storage::load::<Value>("blazar.shortcuts").unwrap_or(json!({})));
    let search = RwSignal::new(String::new());
    let recording = RwSignal::new(String::new());
    view! {
        <section class="card settings-card">
            <h3>"快捷键"</h3>
            <p class="muted">
                "点击更改后按下组合键，Esc 取消。不带 ⌘ / Ctrl 的组合在输入框内不生效。"
            </p>
            <input
                class="settings-input"
                aria-label="搜索快捷键"
                placeholder="搜索动作…"
                on:input=move |e| search.set(event_target_value(&e))
            />
            {SHORTCUTS
                .iter()
                .map(move |&(id, label, default)| {
                    view! {
                        <div
                            class="settings-row"
                            style:display=move || {
                                if label.contains(&search.get()) { "flex" } else { "none" }
                            }
                        >
                            <span>{label}</span>
                            <div class="settings-actions">
                                <kbd>
                                    {move || {
                                        let combo = map
                                            .get()[id]
                                            .as_str()
                                            .unwrap_or(default)
                                            .to_owned();
                                        if combo.is_empty() {
                                            "未设置".to_owned()
                                        } else {
                                            crate::shortcuts::label(&combo)
                                        }
                                    }}
                                </kbd>
                                <button
                                    class="btn"
                                    aria-label=format!("更改「{label}」快捷键")
                                    aria-pressed=move || recording.get() == id
                                    on:click=move |_| recording.set(id.into())
                                    on:keydown=move |e| {
                                        if recording.get_untracked() != id {
                                            return;
                                        }
                                        e.prevent_default();
                                        e.stop_propagation();
                                        if e.key() == "Escape" {
                                            recording.set(String::new());
                                            return;
                                        }
                                        let mut parts = Vec::new();
                                        if e.ctrl_key() || e.meta_key() {
                                            parts.push("Mod".to_owned());
                                        }
                                        if e.alt_key() {
                                            parts.push("Alt".to_owned());
                                        }
                                        if e.shift_key() {
                                            parts.push("Shift".to_owned());
                                        }
                                        let code = e.code();
                                        let key = code
                                            .strip_prefix("Key")
                                            .or_else(|| code.strip_prefix("Digit"))
                                            .unwrap_or(
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
                                                    _ => code.as_str(),
                                                },
                                            );
                                        if [
                                            "ShiftLeft",
                                            "ShiftRight",
                                            "MetaLeft",
                                            "MetaRight",
                                            "ControlLeft",
                                            "ControlRight",
                                            "AltLeft",
                                            "AltRight",
                                        ]
                                            .contains(&key)
                                        {
                                            return;
                                        }
                                        let supported = code.starts_with("Key")
                                            || code.starts_with("Digit")
                                            || matches!(
                                                code.as_str(),
                                                "Backslash" | "Slash" | "Comma" | "Period" | "Semicolon"
                                                | "Quote" | "BracketLeft" | "BracketRight" | "Minus"
                                                | "Equal" | "Backquote" | "Space" | "Enter"
                                            )
                                            || (code.starts_with('F')
                                                && code[1..].parse::<u8>().is_ok());
                                        if !supported {
                                            return;
                                        }
                                        if parts.is_empty() && !key.starts_with('F') {
                                            toast("请加上 ⌘ / Ctrl / Alt");
                                            return;
                                        }
                                        parts.push(key.into());
                                        let combo = parts.join("+");
                                        if let Some(&(_, taken, _)) = SHORTCUTS
                                            .iter()
                                            .find(|&&(other, _, def)| {
                                                other != id
                                                    && map.get_untracked()[other].as_str().unwrap_or(def)
                                                        == combo
                                            })
                                        {
                                            toast(format!("这个组合已被「{taken}」使用"));
                                            return;
                                        }
                                        map.update(|m| m[id] = json!(combo));
                                        storage::save("blazar.shortcuts", &map.get_untracked());
                                        crate::shortcuts::changed();
                                        recording.set(String::new());
                                        toast("已保存");
                                    }
                                >
                                    {move || {
                                        if recording.get() == id {
                                            "按下组合键…"
                                        } else {
                                            "更改"
                                        }
                                    }}
                                </button>
                                <button
                                    class="btn ghost"
                                    aria-label=format!("清除「{label}」快捷键")
                                    on:click=move |_| {
                                        map.update(|m| m[id] = json!(""));
                                        storage::save("blazar.shortcuts", &map.get_untracked());
                                        crate::shortcuts::changed();
                                    }
                                >
                                    "清除"
                                </button>
                            </div>
                        </div>
                    }
                })
                .collect_view()}
            <Show when=move || !SHORTCUTS.iter().any(|(_, label, _)| label.contains(&search.get()))>
                <EmptyState
                    title="没有匹配的快捷键"
                    detail="换一个动作名称再试。"
                />
            </Show>
            <button
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
                "恢复默认快捷键"
            </button>
        </section>
    }
}
