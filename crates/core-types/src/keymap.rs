//! The one table of keyboard shortcuts. The web UI matches key presses against it, the
//! settings page lists it, and the desktop shell builds its menu accelerators from it.
//!
//! A combo is written `Mod+Alt+Shift+Key`: `Mod` is ⌘ on macOS and Ctrl elsewhere, the key
//! is a letter, a digit, an F-key, one of `\ / , . ; ' [ ] - = `` ` or `Space`, `Enter`,
//! `Backspace`, `Delete`. An empty combo means "no shortcut".

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Global,
    Chat,
    Console,
    Changes,
}

impl Group {
    pub const ALL: [Self; 4] = [Self::Global, Self::Chat, Self::Console, Self::Changes];

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Global => "全局",
            Self::Chat => "对话",
            Self::Console => "中控台与批准",
            Self::Changes => "更改",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub id: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    pub group: Group,
}

const fn b(id: &'static str, label: &'static str, default: &'static str, group: Group) -> Binding {
    Binding {
        id,
        label,
        default,
        group,
    }
}

pub const BINDINGS: &[Binding] = &[
    b("palette", "命令面板", "Mod+K", Group::Global),
    b("help", "键盘快捷键", "Mod+/", Group::Global),
    b("newws", "新建工作区", "Mod+Shift+N", Group::Global),
    b("console", "中控台", "Mod+1", Group::Global),
    b("workspaces", "工作区", "Mod+2", Group::Global),
    b("tasks", "任务", "Mod+3", Group::Global),
    b("settings", "设置", "Mod+,", Group::Global),
    b("inbox", "全部消息", "", Group::Global),
    b("side", "收起 / 展开侧栏", "Mod+\\", Group::Global),
    b("explorer", "文件", "Mod+B", Group::Global),
    b("chat", "右侧面板", "Mod+Alt+B", Group::Global),
    b("panel", "终端", "Mod+J", Group::Global),
    b("preview", "预览", "Mod+Shift+P", Group::Global),
    b("inspect", "简介", "Mod+I", Group::Global),
    b("delete", "删除", "Mod+Backspace", Group::Global),
    b("copypath", "复制路径", "Mod+Alt+C", Group::Global),
    b("reveal", "在访达中显示", "Mod+Alt+R", Group::Global),
    b("newchat", "新对话", "Mod+N", Group::Chat),
    b("focus", "聚焦输入框", "Mod+L", Group::Chat),
    b("prevchat", "上一个对话", "Mod+Shift+[", Group::Chat),
    b("nextchat", "下一个对话", "Mod+Shift+]", Group::Chat),
    b("diff", "打开更改", "Mod+Shift+D", Group::Changes),
    b("diffview", "并排 / 统一", "Mod+Alt+S", Group::Changes),
    b("diffws", "忽略空白", "Mod+Alt+W", Group::Changes),
    b("diffwrap", "自动换行", "Alt+Z", Group::Changes),
    b("difflist", "改动文件列表", "Mod+Alt+L", Group::Changes),
    b("difffold", "全部折叠 / 展开", "Mod+Alt+[", Group::Changes),
];

/// Combos the system or the standard Edit and Window menus already own. Binding one of them
/// would either never fire or take copy, paste, quit and friends away from the user.
pub const RESERVED: &[(&str, &str)] = &[
    ("Mod+Q", "退出"),
    ("Mod+W", "关闭窗口"),
    ("Mod+H", "隐藏"),
    ("Mod+Alt+H", "隐藏其他"),
    ("Mod+M", "最小化"),
    ("Mod+C", "拷贝"),
    ("Mod+V", "粘贴"),
    ("Mod+X", "剪切"),
    ("Mod+Z", "撤销"),
    ("Mod+Shift+Z", "重做"),
    ("Mod+A", "全选"),
    ("Mod+Tab", "切换应用"),
    ("Mod+`", "切换窗口"),
    ("Mod+Space", "聚焦搜索"),
    ("Mod+Shift+3", "截屏"),
    ("Mod+Shift+4", "截屏"),
    ("Mod+Shift+5", "截屏"),
];

#[must_use]
pub fn binding(id: &str) -> Option<&'static Binding> {
    BINDINGS.iter().find(|b| b.id == id)
}

#[must_use]
pub fn default_of(id: &str) -> &'static str {
    binding(id).map_or("", |b| b.default)
}

/// What the system uses this combo for, if it is one of [`RESERVED`].
#[must_use]
pub fn reserved(combo: &str) -> Option<&'static str> {
    RESERVED.iter().find(|(c, _)| *c == combo).map(|(_, n)| *n)
}

/// The combo each action answers to, given the bindings the user saved.
///
/// A saved binding always wins, including an empty one (the user cleared it). Every other
/// action keeps its default, unless the user already gave that combo to another action:
/// then the default steps aside instead of firing two things at once. This is also how
/// bindings saved before the defaults moved (⌘N used to be 新建工作区) carry over.
#[must_use]
pub fn effective(saved: &BTreeMap<String, String>) -> Vec<(&'static str, String)> {
    let custom = |id: &str| saved.get(id).filter(|_| binding(id).is_some());
    BINDINGS
        .iter()
        .map(|b| {
            if let Some(c) = custom(b.id) {
                return (b.id, c.clone());
            }
            let taken = !b.default.is_empty()
                && saved
                    .iter()
                    .any(|(id, c)| id != b.id && c == b.default && binding(id).is_some());
            (
                b.id,
                if taken {
                    String::new()
                } else {
                    b.default.to_owned()
                },
            )
        })
        .collect()
}

/// The same combo in the form the native menu understands, or `None` when it has none.
#[must_use]
pub fn accelerator(combo: &str) -> Option<String> {
    let parts: Vec<&str> = combo.split('+').filter(|p| !p.is_empty()).collect();
    let (key, mods) = parts.split_last()?;
    let key = match *key {
        "\\" => "Backslash",
        "/" => "Slash",
        "," => "Comma",
        "." => "Period",
        ";" => "Semicolon",
        "'" => "Quote",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "-" => "Minus",
        "=" => "Equal",
        "`" => "Backquote",
        k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => k,
        k @ ("Space" | "Enter" | "Backspace" | "Delete" | "Tab") => k,
        k if k.starts_with('F') && k[1..].parse::<u8>().is_ok() => k,
        _ => return None,
    };
    let mut out = Vec::new();
    for m in mods {
        out.push(match *m {
            "Mod" => "CmdOrCtrl",
            "Alt" => "Alt",
            "Shift" => "Shift",
            _ => return None,
        });
    }
    out.push(key);
    Some(out.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    fn of(list: &[(&'static str, String)], id: &str) -> String {
        list.iter()
            .find(|(i, _)| *i == id)
            .map(|(_, c)| c.clone())
            .unwrap()
    }

    #[test]
    fn defaults_never_collide_and_never_take_a_system_combo() {
        for (i, a) in BINDINGS.iter().enumerate() {
            assert!(
                reserved(a.default).is_none(),
                "{} uses a reserved combo",
                a.id
            );
            for b in &BINDINGS[i + 1..] {
                assert_ne!(a.id, b.id);
                assert!(
                    a.default.is_empty() || a.default != b.default,
                    "{} and {} share {}",
                    a.id,
                    b.id,
                    a.default
                );
            }
        }
    }

    #[test]
    fn new_chat_is_mod_n_and_pages_are_numbers() {
        assert_eq!(default_of("newchat"), "Mod+N");
        assert_eq!(default_of("newws"), "Mod+Shift+N");
        assert_eq!(default_of("console"), "Mod+1");
        assert_eq!(default_of("workspaces"), "Mod+2");
        assert_eq!(default_of("tasks"), "Mod+3");
        assert_eq!(default_of("focus"), "Mod+L");
        assert_eq!(default_of("help"), "Mod+/");
        assert!(BINDINGS.iter().all(|b| !b.default.starts_with("Alt+I")));
    }

    #[test]
    fn a_saved_binding_wins_and_the_default_it_collides_with_steps_aside() {
        let list = effective(&saved(&[("workspaces", "Mod+N"), ("tasks", "")]));
        assert_eq!(of(&list, "workspaces"), "Mod+N");
        assert_eq!(of(&list, "newchat"), "");
        assert_eq!(of(&list, "tasks"), "");
        assert_eq!(of(&list, "console"), "Mod+1");
    }

    #[test]
    fn old_custom_bindings_keep_working_after_the_defaults_moved() {
        let list = effective(&saved(&[("newws", "Mod+N"), ("inbox", "Alt+I")]));
        assert_eq!(of(&list, "newws"), "Mod+N");
        assert_eq!(of(&list, "inbox"), "Alt+I");
        assert_eq!(of(&list, "newchat"), "");
        let untouched = effective(&BTreeMap::new());
        assert_eq!(of(&untouched, "newchat"), "Mod+N");
        assert_eq!(of(&untouched, "newws"), "Mod+Shift+N");
    }

    #[test]
    fn bindings_of_removed_actions_are_ignored() {
        let list = effective(&saved(&[("gitpanel", "Mod+N")]));
        assert_eq!(of(&list, "newchat"), "Mod+N");
    }

    #[test]
    fn menu_accelerators() {
        assert_eq!(accelerator("Mod+K").as_deref(), Some("CmdOrCtrl+K"));
        assert_eq!(
            accelerator("Mod+\\").as_deref(),
            Some("CmdOrCtrl+Backslash")
        );
        assert_eq!(
            accelerator("Mod+Shift+[").as_deref(),
            Some("CmdOrCtrl+Shift+BracketLeft")
        );
        assert_eq!(accelerator("Mod+/").as_deref(), Some("CmdOrCtrl+Slash"));
        assert_eq!(accelerator("Mod+1").as_deref(), Some("CmdOrCtrl+1"));
        assert_eq!(accelerator("Mod+,").as_deref(), Some("CmdOrCtrl+Comma"));
        assert_eq!(accelerator("Alt+Z").as_deref(), Some("Alt+Z"));
        assert_eq!(accelerator("Mod+F5").as_deref(), Some("CmdOrCtrl+F5"));
        assert_eq!(accelerator(""), None);
        assert_eq!(accelerator("Mod+Weird"), None);
        for b in BINDINGS.iter().filter(|b| !b.default.is_empty()) {
            assert!(accelerator(b.default).is_some(), "{}", b.id);
        }
    }

    #[test]
    fn system_combos_are_reserved() {
        for c in [
            "Mod+Q", "Mod+W", "Mod+H", "Mod+M", "Mod+C", "Mod+V", "Mod+X", "Mod+Z", "Mod+A",
        ] {
            assert!(reserved(c).is_some(), "{c}");
        }
        assert!(reserved("Mod+K").is_none());
    }
}
