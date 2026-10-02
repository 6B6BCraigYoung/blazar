//! 资源管理器：工作区文件树。顶上一行是工作区根目录，目录展开时才去磁盘列那一层
//! （隐藏文件、被 git 忽略的都看得到，忽略的变暗）；改动标记来自 git。
//! 有改动的目录默认展开（改动不多时），支持按路径筛选（筛选只在 git 知道的文件里找）。

use std::collections::{BTreeMap, HashMap, HashSet};

use leptos::prelude::*;

use crate::api::{self, ChangeKind, DirItems, TreeEntry};

use super::files::Files;

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub change: Option<ChangeKind>,
    pub has_change: bool,
    pub kids: Vec<Node>,
}

#[derive(Default)]
struct Building {
    dir: bool,
    change: Option<ChangeKind>,
    kids: BTreeMap<String, Building>,
}

fn convert(name: String, path: String, b: Building) -> Node {
    let mut kids: Vec<Node> = b
        .kids
        .into_iter()
        .map(|(n, k)| {
            let p = if path.is_empty() {
                n.clone()
            } else {
                format!("{path}/{n}")
            };
            convert(n, p, k)
        })
        .collect();
    // 目录在前，再按名字（数字按大小）排。
    kids.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| natural(&a.name, &b.name)));
    let has_change = b.change.is_some() || kids.iter().any(|k| k.has_change);
    Node {
        name,
        path,
        dir: b.dir,
        change: b.change,
        has_change,
        kids,
    }
}

fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| {
        let mut out: Vec<(u8, String, u64)> = Vec::new();
        let mut buf = String::new();
        let mut num = false;
        for c in s.chars() {
            if c.is_ascii_digit() != num && !buf.is_empty() {
                out.push(if num {
                    (0, String::new(), buf.parse().unwrap_or(0))
                } else {
                    (1, buf.to_lowercase(), 0)
                });
                buf.clear();
            }
            num = c.is_ascii_digit();
            buf.push(c);
        }
        if !buf.is_empty() {
            out.push(if num {
                (0, String::new(), buf.parse().unwrap_or(0))
            } else {
                (1, buf.to_lowercase(), 0)
            });
        }
        out
    };
    key(a).cmp(&key(b))
}

pub fn build(entries: &[TreeEntry]) -> Node {
    let mut root = Building {
        dir: true,
        ..Building::default()
    };
    for e in entries {
        let mut cur = &mut root;
        let parts: Vec<&str> = e.path.split('/').filter(|s| !s.is_empty()).collect();
        for (i, part) in parts.iter().enumerate() {
            cur = cur
                .kids
                .entry((*part).to_owned())
                .or_insert_with(|| Building {
                    dir: true,
                    ..Building::default()
                });
            if i + 1 == parts.len() {
                cur.dir = e.is_dir;
                cur.change = e.change;
            }
        }
    }
    convert(String::new(), String::new(), root)
}

pub fn expand_changed(n: &Node, into: &mut HashSet<String>) {
    for k in &n.kids {
        if k.dir && k.has_change {
            into.insert(k.path.clone());
            expand_changed(k, into);
        }
    }
}

pub fn change_mark(c: Option<ChangeKind>) -> (&'static str, &'static str) {
    match c {
        Some(ChangeKind::Modified) => ("M", "modified"),
        Some(ChangeKind::Added) => ("A", "added"),
        Some(ChangeKind::Deleted) => ("D", "deleted"),
        Some(ChangeKind::Untracked) => ("U", "untracked"),
        None => ("", ""),
    }
}

#[derive(Clone, PartialEq)]
struct Row {
    path: String,
    name: String,
    depth: usize,
    dir: bool,
    open: bool,
    change: Option<ChangeKind>,
    has_change: bool,
    ignored: bool,
    /// 这层条目太多、后面没列出来的提示行
    more: bool,
}

fn find<'a>(root: &'a Node, path: &str) -> Option<&'a Node> {
    if path.is_empty() {
        return Some(root);
    }
    let mut cur = root;
    for part in path.split('/') {
        cur = cur.kids.iter().find(|k| k.name == part)?;
    }
    Some(cur)
}

/// 照磁盘列出来的目录（还没列到的先用 git 知道的顶上），从 `depth` 1 开始（0 是根目录那行）。
fn disk_rows(
    git: Option<&Node>,
    dirs: &HashMap<String, DirItems>,
    opened: &HashSet<String>,
) -> Vec<Row> {
    fn emit(
        git: Option<&Node>,
        dirs: &HashMap<String, DirItems>,
        opened: &HashSet<String>,
        dir: &str,
        depth: usize,
        out: &mut Vec<Row>,
    ) {
        let gnode = git.and_then(|g| find(g, dir));
        let by_name: HashMap<&str, &Node> = gnode
            .map(|n| n.kids.iter().map(|k| (k.name.as_str(), k)).collect())
            .unwrap_or_default();
        // (名字, 是目录, 被忽略, git 节点)
        let mut kids: Vec<(String, bool, bool, Option<&Node>)> = match dirs.get(dir) {
            Some(l) => {
                let mut v: Vec<_> = l
                    .items
                    .iter()
                    .map(|i| {
                        let g = by_name.get(i.name.as_str()).copied();
                        (i.name.clone(), i.is_dir, i.ignored, g)
                    })
                    .collect();
                // 磁盘上已经没有、但 git 里记着改动的（删掉的文件）也列出来
                let on_disk: HashSet<&str> = l.items.iter().map(|i| i.name.as_str()).collect();
                if !l.truncated {
                    for k in gnode.map(|n| n.kids.as_slice()).unwrap_or_default() {
                        if k.has_change && !on_disk.contains(k.name.as_str()) {
                            v.push((k.name.clone(), k.dir, false, Some(k)));
                        }
                    }
                }
                v
            }
            None => gnode
                .map(|n| {
                    n.kids
                        .iter()
                        .map(|k| (k.name.clone(), k.dir, false, Some(k)))
                        .collect()
                })
                .unwrap_or_default(),
        };
        kids.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| natural(&a.0, &b.0)));
        for (name, is_dir, ignored, g) in kids {
            let path = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            let open = is_dir && opened.contains(&path);
            out.push(Row {
                path: path.clone(),
                name,
                depth,
                dir: is_dir,
                open,
                change: g.and_then(|g| g.change),
                has_change: g.is_some_and(|g| g.has_change),
                ignored,
                more: false,
            });
            if open {
                emit(git, dirs, opened, &path, depth + 1, out);
            }
        }
        if dirs.get(dir).is_some_and(|l| l.truncated) {
            out.push(Row {
                path: format!("{dir}/…"),
                name: format!("只列出了前 {} 项", crate::api::MAX_DIR_ITEMS),
                depth,
                dir: false,
                open: false,
                change: None,
                has_change: false,
                ignored: true,
                more: true,
            });
        }
    }
    let mut out = Vec::new();
    emit(git, dirs, opened, "", 1, &mut out);
    out
}

fn rows(root: &Node, opened: &HashSet<String>, filter: &str) -> Vec<Row> {
    let f = filter.trim().to_lowercase();
    let mut keep = HashSet::new();
    if !f.is_empty() {
        fn walk(n: &Node, f: &str, keep: &mut HashSet<String>) -> bool {
            let mut hit = !n.path.is_empty() && n.path.to_lowercase().contains(f);
            for k in &n.kids {
                hit |= walk(k, f, keep);
            }
            if hit && !n.path.is_empty() {
                keep.insert(n.path.clone());
            }
            hit
        }
        walk(root, &f, &mut keep);
    }
    let mut out = Vec::new();
    fn emit(
        n: &Node,
        depth: usize,
        opened: &HashSet<String>,
        keep: Option<&HashSet<String>>,
        out: &mut Vec<Row>,
    ) {
        for k in &n.kids {
            if keep.is_some_and(|s| !s.contains(&k.path)) {
                continue;
            }
            let open = k.dir && (keep.is_some() || opened.contains(&k.path));
            out.push(Row {
                path: k.path.clone(),
                name: k.name.clone(),
                depth,
                dir: k.dir,
                open,
                change: k.change,
                has_change: k.has_change,
                ignored: false,
                more: false,
            });
            if open {
                emit(k, depth + 1, opened, keep, out);
            }
        }
    }
    emit(root, 1, opened, (!f.is_empty()).then_some(&keep), &mut out);
    out
}

const CHEV: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>"#;

#[component]
pub fn FileTree(
    root: Memo<Option<Node>>,
    opened: RwSignal<HashSet<String>>,
    files: Files,
    /// 根目录那一行显示的名字（工作区目录名）
    #[prop(into)]
    root_name: Signal<String>,
    /// 变了就把已经列过的目录重新列一遍
    #[prop(into)]
    reload: Signal<u32>,
) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let root_open = RwSignal::new(true);
    let dirs = RwSignal::new(HashMap::<String, DirItems>::new());
    // 已经去列过的目录（含失败的），免得失败的目录一直重试
    let tried = StoredValue::new(HashSet::<String>::new());
    let ws = files.ws();
    let fetch = StoredValue::new(move |dir: String| {
        let url = format!("/api/workspaces/{ws}/ls?path={}", api::enc(&dir));
        leptos::task::spawn_local(async move {
            if let Ok(l) = api::get::<DirItems>(&url).await {
                dirs.try_update(|m| m.insert(dir, l));
            }
        });
    });
    // 根目录和展开的目录：没列过的去列
    Effect::new(move |_| {
        if !root_open.get() {
            return;
        }
        let want: Vec<String> = opened.with(|o| {
            std::iter::once(String::new())
                .chain(o.iter().cloned())
                .filter(|d| !tried.with_value(|t| t.contains(d)))
                .collect()
        });
        for d in want {
            tried.update_value(|t| {
                t.insert(d.clone());
            });
            fetch.with_value(|f| f(d));
        }
    });
    // 文件变了：开着的目录重新列，收起的丢掉缓存
    Effect::new(move |prev: Option<()>| {
        reload.track();
        if prev.is_none() {
            return;
        }
        let keep: HashSet<String> = opened.with_untracked(|o| {
            std::iter::once(String::new())
                .chain(o.iter().cloned())
                .collect()
        });
        dirs.update(|m| m.retain(|k, _| keep.contains(k)));
        tried.set_value(HashSet::new());
        opened.update(|_| {});
    });
    let list = move || {
        let f = filter.get();
        root.with(|r| {
            if !f.trim().is_empty() {
                return r.as_ref().map(|r| opened.with(|o| rows(r, o, &f)));
            }
            dirs.with(|d| {
                (r.is_some() || d.contains_key(""))
                    .then(|| opened.with(|o| disk_rows(r.as_ref(), d, o)))
            })
        })
    };
    view! {
        <input class="tree-filter" placeholder="筛选文件…" prop:value=move || filter.get()
            on:input=move |e| filter.set(event_target_value(&e))/>
        <div class="tree">
            <div class="tn root" data-open=move || root_open.get().to_string()
                title=move || root_name.get() on:click=move |_| root_open.update(|o| *o = !*o)>
                <span class="chev" inner_html=CHEV></span>
                <span class="nm dir">{move || root_name.get()}</span>
            </div>
            {move || (root_open.get() || !filter.with(|f| f.trim().is_empty())).then(|| match list() {
                None => view! { <div class="empty">"加载中…"</div> }.into_any(),
                Some(r) if r.is_empty() => view! { <div class="empty">{move || if filter.with(|f| f.trim().is_empty()) { "空目录" } else { "无匹配" }}</div> }.into_any(),
                Some(r) => r.into_iter().map(|row| {
                    let (mark, cls) = change_mark(row.change);
                    let p = row.path.clone();
                    let dir = row.dir;
                    let more = row.more;
                    let sel = {
                        let p = p.clone();
                        move || !dir && files.current.with(|c| c.as_deref() == Some(p.as_str()))
                    };
                    let click = move |_| {
                        if more {
                        } else if dir {
                            opened.update(|o| if !o.remove(&p) { o.insert(p.clone()); });
                        } else {
                            files.open(p.clone(), 0);
                        }
                    };
                    view! {
                        <div class="tn" data-open=row.open.to_string() data-sel=move || sel().to_string()
                            data-ign=row.ignored.to_string() data-more=more.to_string()
                            data-chg=(dir && row.has_change).to_string()
                            title=row.path.clone() on:click=click
                            style=format!("padding-left:{}px", 8 + row.depth * 12)>
                            <span class="chev" inner_html=if dir { CHEV } else { "" }></span>
                            <span class="nm" class:dir=dir>{row.name}</span>
                            <span class=format!("ch {cls}")>{mark}</span>
                        </div>
                    }
                }).collect_view().into_any(),
            })}
        </div>
    }
}
