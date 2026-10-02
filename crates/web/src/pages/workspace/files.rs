//! 编辑器里开着的文件：标签页、未保存标记、保存（带冲突检测）、agent 改过文件后自动重载。

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::components::dialog::{self, Choice};
use crate::monaco::{self, Mounted};

#[derive(Clone, Copy, Default)]
struct Meta {
    mtime: u64,
    readonly: bool,
}

#[derive(Clone, Copy)]
pub struct Files {
    ws: StoredValue<String>,
    pub open_tabs: RwSignal<Vec<String>>,
    pub current: RwSignal<Option<String>>,
    pub dirty: RwSignal<HashSet<String>>,
    /// 当前文件内容每变一次加一（Markdown 预览靠它刷新）。
    pub rev: RwSignal<u32>,
    pub error: RwSignal<Option<String>>,
    meta: StoredValue<HashMap<String, Meta>>,
    editor: StoredValue<Option<Rc<Mounted>>, LocalStorage>,
    /// 编辑器还没加载好时先读到的内容，等它好了再放进去。
    pending: StoredValue<HashMap<String, (String, bool, u32)>>,
}

fn binary_note(path: &str, r: &api::FileContent) -> Option<String> {
    let mb = r.size as f64 / 1_048_576.0;
    if r.binary {
        Some(format!(
            "// {path}\n// 二进制文件（{mb:.1} MB），不在编辑器里显示。\n// 要看内容请用下方「终端」面板。"
        ))
    } else if r.too_large {
        Some(format!(
            "// {path}\n// 文件过大（{mb:.1} MB），超出 2 MB 上限未加载。\n// 远程读是整文件传输，大文件会把链路占满。"
        ))
    } else {
        None
    }
}

impl Files {
    pub fn new(ws: String) -> Self {
        Self {
            ws: StoredValue::new(ws),
            open_tabs: RwSignal::new(Vec::new()),
            current: RwSignal::new(None),
            dirty: RwSignal::new(HashSet::new()),
            rev: RwSignal::new(0),
            error: RwSignal::new(None),
            meta: StoredValue::new(HashMap::new()),
            editor: StoredValue::new_local(None),
            pending: StoredValue::new(HashMap::new()),
        }
    }

    pub fn ws(self) -> String {
        self.ws.try_get_value().unwrap_or_default()
    }

    fn with_editor<R>(self, f: impl FnOnce(&monaco::Editor) -> R) -> Option<R> {
        self.editor
            .try_with_value(|e| e.as_ref().map(|m| f(&m.editor)))
            .flatten()
    }

    pub fn is_md(path: &str) -> bool {
        let p = path.to_ascii_lowercase();
        p.ends_with(".md") || p.ends_with(".markdown") || p.ends_with(".mdx")
    }

    pub fn readonly(self, path: &str) -> bool {
        self.meta
            .with_value(|m| m.get(path).is_some_and(|x| x.readonly))
    }

    pub fn value(self, path: &str) -> String {
        self.with_editor(|e| e.value(path)).unwrap_or_default()
    }

    /// 编辑器宿主节点挂上之后调用一次。
    pub fn mount(self, host: web_sys::HtmlElement) {
        spawn_local(async move {
            let on_save = move |p: String| self.save(p, false);
            let on_dirty = move |p: String, d: bool| {
                if self.dirty.try_with_untracked(|s| s.contains(&p) != d) == Some(true) {
                    self.dirty.update(|s| {
                        if d {
                            s.insert(p.clone());
                        } else {
                            s.remove(&p);
                        }
                    });
                }
                if self
                    .current
                    .try_with_untracked(|c| c.as_deref() == Some(p.as_str()))
                    == Some(true)
                {
                    self.rev.update(|n| *n = n.wrapping_add(1));
                }
            };
            match monaco::mount(&host, on_save, on_dirty).await {
                Ok(m) => {
                    if self.editor.try_set_value(Some(Rc::new(m))).is_some() {
                        // 页面已经离开了。
                        return;
                    }
                    if let Some(cur) = self.current.get_untracked()
                        && let Some((text, ro, line)) =
                            self.pending.try_update_value(|p| p.remove(&cur)).flatten()
                    {
                        self.with_editor(|e| e.open(&cur, &text, ro, line));
                        self.rev.update(|n| *n = n.wrapping_add(1));
                    }
                }
                Err(e) => self.error.set(Some(format!("编辑器加载失败：{e:?}"))),
            }
        });
    }

    pub fn dispose(self) {
        if let Some(Some(m)) = self.editor.try_update_value(Option::take) {
            m.editor.dispose();
        }
    }

    /// 打开（或切到）文件；`line > 0` 时跳到那一行。
    pub fn open(self, path: String, line: u32) {
        self.open_tabs.update(|t| {
            if !t.contains(&path) {
                t.push(path.clone());
            }
        });
        self.current.set(Some(path.clone()));
        if self.with_editor(|e| e.has(&path)) == Some(true) {
            let ro = self.readonly(&path);
            self.with_editor(|e| e.open(&path, "", ro, line));
            self.rev.update(|n| *n = n.wrapping_add(1));
            self.reload(path, false);
            return;
        }
        let ws = self.ws();
        spawn_local(async move {
            match api::read_file(&ws, &path).await {
                Ok(r) => {
                    let note = binary_note(&path, &r);
                    let ro = note.is_some();
                    let text = note.unwrap_or(r.content);
                    if self
                        .meta
                        .try_update_value(|m| {
                            m.insert(
                                path.clone(),
                                Meta {
                                    mtime: r.mtime,
                                    readonly: ro,
                                },
                            )
                        })
                        .is_none()
                    {
                        return;
                    }
                    // 读的过程中标签可能已经被关了。
                    if !self.open_tabs.with_untracked(|t| t.contains(&path)) {
                        return;
                    }
                    let shown = self
                        .current
                        .with_untracked(|c| c.as_deref() == Some(path.as_str()));
                    if self.with_editor(|_| ()).is_some() {
                        if shown {
                            self.with_editor(|e| e.open(&path, &text, ro, line));
                        }
                        self.rev.update(|n| *n = n.wrapping_add(1));
                    } else {
                        self.pending.update_value(|p| {
                            p.insert(path.clone(), (text, ro, line));
                        });
                    }
                    api::put_editor_context(&ws, &path).await;
                }
                Err(e) => self.error.set(Some(format!("打开失败：{e}"))),
            }
        });
    }

    /// 关标签；有没保存的改动先问。
    pub fn close(self, path: String) {
        spawn_local(async move {
            if self.dirty.with_untracked(|d| d.contains(&path)) {
                let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                let pick = dialog::ask(
                    "还没保存",
                    &format!("「{name}」还没保存，关掉的话改动就丢了。"),
                    vec![Choice::plain("取消"), Choice::danger("不保存，关掉")],
                )
                .await;
                if pick != Some(1) {
                    return;
                }
            }
            let Some(i) = self
                .open_tabs
                .try_with_untracked(|t| t.iter().position(|p| *p == path))
                .flatten()
            else {
                return;
            };
            self.open_tabs.update(|t| {
                t.remove(i);
            });
            self.with_editor(|e| e.close(&path));
            self.meta.update_value(|m| {
                m.remove(&path);
            });
            self.pending.update_value(|m| {
                m.remove(&path);
            });
            self.dirty.update(|d| {
                d.remove(&path);
            });
            if self
                .current
                .with_untracked(|c| c.as_deref() == Some(path.as_str()))
            {
                let next = self.open_tabs.with_untracked(|t| {
                    t.get(i)
                        .or_else(|| i.checked_sub(1).and_then(|j| t.get(j)))
                        .cloned()
                });
                match next {
                    Some(n) => self.open(n, 0),
                    None => {
                        self.current.set(None);
                        self.with_editor(monaco::Editor::clear);
                    }
                }
            }
        });
    }

    pub fn save(self, path: String, force: bool) {
        if self.readonly(&path) {
            return;
        }
        if !force && !self.dirty.with_untracked(|d| d.contains(&path)) {
            return;
        }
        let content = self.value(&path);
        let ws = self.ws();
        let mtime = self.meta.with_value(|m| m.get(&path).map(|x| x.mtime));
        spawn_local(async move {
            let expect = if force { None } else { mtime };
            match api::write_file(&ws, &path, &content, expect).await {
                Ok(w) if w.saved => {
                    self.meta.update_value(|m| {
                        if let Some(x) = m.get_mut(&path) {
                            x.mtime = w.mtime;
                        }
                    });
                    // 保存过程中又改了的话，仍然算未保存。
                    if self.value(&path) == content {
                        self.with_editor(|e| e.mark_saved(&path));
                    }
                }
                Ok(_) => self.conflict(path),
                Err(e) => self.error.set(Some(format!("保存失败：{e}"))),
            }
        });
    }

    fn conflict(self, path: String) {
        spawn_local(async move {
            let pick = dialog::ask(
                "这个文件被别人改过了",
                &format!(
                    "{path} 在你打开之后又被改过（多半是 agent）。直接保存会把那边的改动盖掉。"
                ),
                vec![
                    Choice::plain("先不保存"),
                    Choice::plain("丢掉我的改动，载入最新的"),
                    Choice::danger("用我的覆盖"),
                ],
            )
            .await;
            match pick {
                Some(1) => self.reload(path, true),
                Some(2) => self.save(path, true),
                _ => {}
            }
        });
    }

    /// 从磁盘重新读。有未保存改动且不是 `discard` 时不动。
    pub fn reload(self, path: String, discard: bool) {
        if !discard && self.dirty.with_untracked(|d| d.contains(&path)) {
            return;
        }
        let ws = self.ws();
        let known = self.meta.with_value(|m| m.get(&path).copied());
        spawn_local(async move {
            let Ok(r) = api::read_file(&ws, &path).await else {
                return;
            };
            if r.binary || r.too_large || (!discard && known.is_some_and(|k| k.mtime == r.mtime)) {
                return;
            }
            if !discard && self.dirty.try_with_untracked(|d| d.contains(&path)) != Some(false) {
                return;
            }
            self.with_editor(|e| e.replace(&path, &r.content));
            self.meta.update_value(|m| {
                if let Some(x) = m.get_mut(&path) {
                    x.mtime = r.mtime;
                }
            });
            self.rev.update(|n| *n = n.wrapping_add(1));
        });
    }
}
