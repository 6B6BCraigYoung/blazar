use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::components::dialog::{self, Choice};
use crate::drafts::{self, Draft};
use crate::monaco::{self, Mounted};
use crate::storage;

mod version;

#[derive(Clone, Default)]
struct Meta {
    mtime: u64,
    version: Option<String>,
    readonly: bool,
}

#[derive(Clone, Copy)]
pub struct Files {
    ws: StoredValue<String>,
    pub open_tabs: RwSignal<Vec<String>>,
    pub current: RwSignal<Option<String>>,
    pub dirty: RwSignal<HashSet<String>>,
    pub rev: RwSignal<u32>,
    pub revealed: RwSignal<u32>,
    pub error: RwSignal<Option<(String, Option<String>)>>,
    meta: StoredValue<HashMap<String, Meta>>,
    editor: StoredValue<Option<Rc<Mounted>>, LocalStorage>,
    pending: StoredValue<HashMap<String, (String, bool, u32)>>,
    /// 打开时发现本地有没存盘的草稿：等用户选「恢复未保存的改动」或「丢弃」。
    pub draft_offer: RwSignal<Option<String>>,
    drafts_written: StoredValue<HashMap<String, String>>,
    draft_timer: StoredValue<Option<gloo_timers::callback::Interval>, LocalStorage>,
}

/// 多久把没存盘的编辑往本地存一次。
const DRAFT_EVERY_MS: u32 = 2_000;

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
            revealed: RwSignal::new(0),
            error: RwSignal::new(None),
            meta: StoredValue::new(HashMap::new()),
            editor: StoredValue::new_local(None),
            pending: StoredValue::new(HashMap::new()),
            draft_offer: RwSignal::new(None),
            drafts_written: StoredValue::new(HashMap::new()),
            draft_timer: StoredValue::new_local(None),
        }
    }

    fn draft_key(self, path: &str) -> String {
        drafts::key(&self.ws(), path)
    }

    /// 没存盘的文件各存一份草稿，页面重新载入、连接断开都不丢。
    fn write_drafts(self) {
        let Some(dirty) = self.dirty.try_get_untracked() else {
            return;
        };
        if self.ws.try_get_value().is_none() || dirty.is_empty() {
            return;
        }
        let offered = self.draft_offer.get_untracked();
        for path in dirty {
            if offered.as_deref() == Some(path.as_str()) || self.readonly(&path) {
                continue;
            }
            let text = self.value(&path);
            let last = self.drafts_written.with_value(|w| w.get(&path).cloned());
            if !drafts::worth_keeping(&text) || !drafts::changed(last.as_deref(), &text) {
                continue;
            }
            storage::save(
                &self.draft_key(&path),
                &Draft {
                    text: text.clone(),
                    at: String::from(js_sys::Date::new_0().to_iso_string()),
                },
            );
            self.drafts_written.update_value(|w| {
                w.insert(path, text);
            });
        }
    }

    fn drop_draft(self, path: &str) {
        storage::remove(&self.draft_key(path));
        self.drafts_written.update_value(|w| {
            w.remove(path);
        });
        if self
            .draft_offer
            .with_untracked(|o| o.as_deref() == Some(path))
        {
            self.draft_offer.set(None);
        }
    }

    /// 打开文件后：本地草稿和磁盘上不一样就提示恢复；一样就是过期的，删掉。
    fn check_draft(self, path: &str, on_disk: &str) {
        let draft: Option<Draft> = storage::load(&self.draft_key(path));
        if drafts::offer(draft.as_ref(), on_disk) {
            self.draft_offer.set(Some(path.to_owned()));
        } else if draft.is_some() {
            storage::remove(&self.draft_key(path));
        }
    }

    pub fn draft_time(self, path: &str) -> String {
        storage::load::<Draft>(&self.draft_key(path))
            .map(|d| d.at)
            .filter(|at| !at.is_empty())
            .map(|at| crate::fmt::ago(&at))
            .unwrap_or_default()
    }

    pub fn restore_draft(self, path: String) {
        let Some(draft) = storage::load::<Draft>(&self.draft_key(&path)) else {
            self.draft_offer.set(None);
            return;
        };
        if self.with_editor(|e| e.restore_draft(&path, &draft.text)) == Some(true) {
            self.draft_offer.set(None);
            self.rev.update(|n| *n = n.wrapping_add(1));
        }
    }

    pub fn discard_draft(self, path: String) {
        self.drop_draft(&path);
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

    pub fn mount(self, host: web_sys::HtmlElement) {
        spawn_local(async move {
            let on_save = move |p: String| self.save(p, false);
            let on_dirty = move |p: String, d: bool| {
                let offered = self
                    .draft_offer
                    .try_with_untracked(|o| o.as_deref() == Some(p.as_str()))
                    .unwrap_or(true);
                if !d && !offered {
                    self.drop_draft(&p);
                }
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
                        return;
                    }
                    let timer = gloo_timers::callback::Interval::new(DRAFT_EVERY_MS, move || {
                        self.write_drafts();
                    });
                    let _ = self.draft_timer.try_set_value(Some(timer));
                    if let Some(cur) = self.current.try_get_untracked().flatten()
                        && let Some((text, ro, line)) =
                            self.pending.try_update_value(|p| p.remove(&cur)).flatten()
                    {
                        self.with_editor(|e| e.open(&cur, &text, ro, line));
                        self.rev.update(|n| *n = n.wrapping_add(1));
                    }
                }
                Err(e) => self
                    .error
                    .set(Some(("编辑器加载失败".into(), Some(format!("{e:?}"))))),
            }
        });
    }

    pub fn dispose(self) {
        self.write_drafts();
        let _ = self.draft_timer.try_update_value(Option::take);
        if let Some(Some(m)) = self.editor.try_update_value(Option::take) {
            m.editor.dispose();
        }
    }

    pub fn open(self, path: String, line: u32) {
        self.revealed.update(|n| *n = n.wrapping_add(1));
        self.restore(path, line);
    }

    pub fn restore(self, path: String, line: u32) {
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
                                    version: r.version,
                                    readonly: ro,
                                },
                            )
                        })
                        .is_none()
                    {
                        return;
                    }
                    if !self.open_tabs.with_untracked(|t| t.contains(&path)) {
                        return;
                    }
                    if !ro {
                        self.check_draft(&path, &text);
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
                Err(e) => self
                    .error
                    .set(Some(crate::vocab::lead("打开失败", e.explained()))),
            }
        });
    }

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
            self.drop_draft(&path);
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
        let known = self.meta.with_value(|m| m.get(&path).cloned());
        spawn_local(async move {
            let expect = if force {
                None
            } else {
                known.as_ref().map(|m| m.mtime)
            };
            let expect_version = if force {
                None
            } else {
                known.as_ref().and_then(|m| m.version.as_deref())
            };
            match api::write_file(&ws, &path, &content, expect, expect_version).await {
                Ok(w) if w.saved => {
                    self.meta.update_value(|m| {
                        if let Some(x) = m.get_mut(&path) {
                            x.mtime = w.mtime;
                            x.version = w.version;
                        }
                    });
                    if self.value(&path) == content {
                        self.with_editor(|e| e.mark_saved(&path));
                    }
                }
                Ok(_) => self.conflict(path),
                Err(e) => self
                    .error
                    .set(Some(crate::vocab::lead("保存失败", e.explained()))),
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

    pub fn reload(self, path: String, discard: bool) {
        if !discard && self.dirty.with_untracked(|d| d.contains(&path)) {
            return;
        }
        let ws = self.ws();
        let known = self.meta.with_value(|m| m.get(&path).cloned());
        spawn_local(async move {
            let Ok(r) = api::read_file(&ws, &path).await else {
                return;
            };
            if r.binary
                || r.too_large
                || (!discard
                    && known.is_some_and(|k| {
                        version::unchanged(
                            k.mtime,
                            k.version.as_deref(),
                            r.mtime,
                            r.version.as_deref(),
                        )
                    }))
            {
                return;
            }
            if !discard && self.dirty.try_with_untracked(|d| d.contains(&path)) != Some(false) {
                return;
            }
            self.with_editor(|e| e.replace(&path, &r.content));
            self.meta.update_value(|m| {
                if let Some(x) = m.get_mut(&path) {
                    x.mtime = r.mtime;
                    x.version = r.version;
                }
            });
            self.rev.update(|n| *n = n.wrapping_add(1));
        });
    }
}
