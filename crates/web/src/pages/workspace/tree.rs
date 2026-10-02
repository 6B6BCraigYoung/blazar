//! 资源管理器：工作区文件树。有改动的目录默认展开（改动不多时），支持按路径筛选。

use std::collections::{BTreeMap, HashSet};

use leptos::prelude::*;

use crate::api::{ChangeKind, TreeEntry};

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
            });
            if open {
                emit(k, depth + 1, opened, keep, out);
            }
        }
    }
    emit(root, 0, opened, (!f.is_empty()).then_some(&keep), &mut out);
    out
}

const CHEV: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>"#;

#[component]
pub fn FileTree(
    root: Memo<Option<Node>>,
    opened: RwSignal<HashSet<String>>,
    files: Files,
) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let list = move || {
        root.with(|r| {
            r.as_ref()
                .map(|r| opened.with(|o| filter.with(|f| rows(r, o, f))))
        })
    };
    view! {
        <input class="tree-filter" placeholder="筛选文件…" prop:value=move || filter.get()
            on:input=move |e| filter.set(event_target_value(&e))/>
        <div class="tree">
            {move || match list() {
                None => view! { <div class="empty">"加载中…"</div> }.into_any(),
                Some(r) if r.is_empty() => view! { <div class="empty">"无匹配"</div> }.into_any(),
                Some(r) => r.into_iter().map(|row| {
                    let (mark, cls) = change_mark(row.change);
                    let p = row.path.clone();
                    let dir = row.dir;
                    let sel = {
                        let p = p.clone();
                        move || !dir && files.current.with(|c| c.as_deref() == Some(p.as_str()))
                    };
                    let click = move |_| {
                        if dir {
                            opened.update(|o| if !o.remove(&p) { o.insert(p.clone()); });
                        } else {
                            files.open(p.clone(), 0);
                        }
                    };
                    view! {
                        <div class="tn" data-open=row.open.to_string() data-sel=move || sel().to_string()
                            title=row.path.clone() on:click=click
                            style=format!("padding-left:{}px", 8 + row.depth * 14)>
                            <span class="chev" inner_html=if dir { CHEV } else { "" }></span>
                            <span class="nm" class:dir=dir>{row.name}</span>
                            <span class=format!("ch {cls}")>{mark}</span>
                        </div>
                    }
                }).collect_view().into_any(),
            }}
        </div>
    }
}
