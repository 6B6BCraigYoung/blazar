use std::collections::{BTreeMap, HashMap, HashSet};

use leptos::prelude::*;

use crate::components::status::{EmptyState, InlineError};

use crate::api::{self, ChangeKind, DirItems, TreeEntry};

use super::files::Files;

fn guide_dirs(path: &str, depth: usize) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect();
    (0..depth.min(parts.len()))
        .map(|i| parts[..i].join("/"))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct Menu {
    x: i32,
    y: i32,
    path: String,
    name: String,
    dir: bool,
}

#[component]
fn TreeMenu(menu: RwSignal<Option<Menu>>, ws: String) -> impl IntoView {
    let close_down = window_event_listener(leptos::ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
            .and_then(|el| el.closest(".ctx-menu").ok().flatten())
            .is_some();
        if !inside {
            menu.set(None);
        }
    });
    let close_key = window_event_listener(leptos::ev::keydown, move |e| {
        if e.key() == "Escape" {
            menu.set(None);
        }
    });
    let close_blur = window_event_listener(leptos::ev::blur, move |_| menu.set(None));
    on_cleanup(move || {
        close_down.remove();
        close_key.remove();
        close_blur.remove();
    });
    move || {
        menu.get().map(|m| {
            let (w, h) = (
                window()
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as i32,
                window()
                    .inner_height()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as i32,
            );
            let x = m.x.min(w - 200).max(0);
            let y = m.y.min(h - 48).max(0);
            let url = format!(
                "/api/workspaces/{}/download?path={}{}",
                api::enc(&ws),
                api::enc(&m.path),
                if m.dir { "&dir=true" } else { "" }
            );
            let file = if m.dir {
                format!("{}.tar.gz", m.name)
            } else {
                m.name.clone()
            };
            view! {
                <div class="ctx-menu" role="menu" style=format!("left:{x}px;top:{y}px")>
                    <button type="button" role="menuitem" class="ctx-item" on:click=move |_| {
                        menu.set(None);
                        crate::files_js::download_url(&url, &file);
                    }>{if m.dir { "下载文件夹" } else { "下载" }}</button>
                </div>
            }
        })
    }
}

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

const ICON_FOLDER: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M1.5 4.5a1 1 0 0 1 1-1h3.1l1.4 1.5h6.5a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z"/></svg>"#;
const ICON_FOLDER_OPEN: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M1.5 12V4.5a1 1 0 0 1 1-1h3.1l1.4 1.5h5.5a1 1 0 0 1 1 1V7"/><path d="M1.5 12l1.9-4.3a1 1 0 0 1 .9-.6h10.2l-2.1 4.9a1 1 0 0 1-.9.6H2.5a1 1 0 0 1-1-.6z"/></svg>"#;
const ICON_FILE: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"><path d="M4 1.5h5L12.5 5v8.5a1 1 0 0 1-1 1h-7.5a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1z"/><path d="M9 1.5V5h3.5"/></svg>"#;
const ICON_CODE: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" stroke-linecap="round"><path d="M4 1.5h5L12.5 5v8.5a1 1 0 0 1-1 1h-7.5a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1z"/><path d="M9 1.5V5h3.5"/><path d="M6.6 8.4 5.3 9.7l1.3 1.3M9.4 8.4l1.3 1.3-1.3 1.3"/></svg>"#;
const ICON_TEXT: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" stroke-linecap="round"><path d="M4 1.5h5L12.5 5v8.5a1 1 0 0 1-1 1h-7.5a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1z"/><path d="M9 1.5V5h3.5"/><path d="M5.8 8.3h4.4M5.8 10.6h4.4"/></svg>"#;
const ICON_IMAGE: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" stroke-linecap="round"><path d="M4 1.5h5L12.5 5v8.5a1 1 0 0 1-1 1h-7.5a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1z"/><path d="M9 1.5V5h3.5"/><circle cx="6.4" cy="8.3" r=".9"/><path d="M4.6 13 7 10.3l1.5 1.6 1.2-1.1 1.7 2.2"/></svg>"#;

const CODE_EXT: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "c", "h", "cpp", "hpp", "cc",
    "cs", "rb", "php", "swift", "kt", "lua", "sh", "bash", "zsh", "fish", "sql", "toml", "yaml",
    "yml", "json", "xml", "html", "css", "scss", "less", "vue", "svelte", "proto", "cmake", "mk",
    "gradle", "dart", "scala", "zig", "ex", "exs", "erl", "hs", "ml", "r", "jl", "m", "mm", "nix",
];
const TEXT_EXT: &[&str] = &[
    "md", "txt", "rst", "csv", "tsv", "log", "adoc", "org", "lock",
];
const IMAGE_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "bmp", "tiff", "avif", "heic",
];

fn file_icon(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    if CODE_EXT.contains(&ext) || lower == "makefile" || lower == "dockerfile" {
        ICON_CODE
    } else if TEXT_EXT.contains(&ext) || lower == "license" || lower == "readme" {
        ICON_TEXT
    } else if IMAGE_EXT.contains(&ext) {
        ICON_IMAGE
    } else {
        ICON_FILE
    }
}

const CHEV: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>"#;

#[component]
pub fn FileTree(
    root: Memo<Option<Node>>,
    opened: RwSignal<HashSet<String>>,
    files: Files,
    #[prop(into)] root_name: Signal<String>,
    #[prop(into)] reload: Signal<u32>,
) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let root_open = RwSignal::new(true);
    let dirs = RwSignal::new(HashMap::<String, DirItems>::new());
    let loading = RwSignal::new(HashSet::<String>::new());
    let menu = RwSignal::new(None::<Menu>);
    let errors = RwSignal::new(HashMap::<String, String>::new());
    let tried = StoredValue::new(HashSet::<String>::new());
    let ws = files.ws();
    let fetch = StoredValue::new(move |dir: String| {
        loading.update(|paths| {
            paths.insert(dir.clone());
        });
        errors.update(|messages| {
            messages.remove(&dir);
        });
        let url = format!("/api/workspaces/{ws}/ls?path={}", api::enc(&dir));
        leptos::task::spawn_local(async move {
            match api::get::<DirItems>(&url).await {
                Ok(listing) => {
                    dirs.try_update(|items| items.insert(dir.clone(), listing));
                }
                Err(error) => {
                    errors.try_update(|messages| messages.insert(dir.clone(), error.to_string()));
                }
            }
            loading.try_update(|paths| paths.remove(&dir));
        });
    });
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
        errors.update(|messages| messages.retain(|path, _| keep.contains(path)));
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
        <input class="tree-filter" aria-label="筛选文件" title="按路径筛选 Git 已知的文件" placeholder="筛选文件…" prop:value=move || filter.get()
            on:input=move |e| filter.set(event_target_value(&e))/>
        <TreeMenu menu ws=files.ws()/>
        <div class="tree" on:scroll=move |_| menu.set(None)>
            <button type="button" class="tn root" aria-expanded=move || root_open.get().to_string() data-open=move || root_open.get().to_string()
                title=move || root_name.get() on:click=move |_| root_open.update(|o| *o = !*o)
                on:contextmenu=move |e: leptos::ev::MouseEvent| {
                    e.prevent_default();
                    menu.set(Some(Menu { x: e.client_x(), y: e.client_y(), path: String::new(), name: root_name.get_untracked(), dir: true }));
                }>
                <span class="chev" inner_html=CHEV></span>
                <span class="nm dir">{move || root_name.get()}</span>
            </button>
            <Show when=move || root_open.get()>
                {move || errors.get().into_iter().filter(|(path, _)| path.is_empty() || opened.with(|paths| paths.contains(path))).map(|(path, error)| {
                    let label = if path.is_empty() { "根目录".to_owned() } else { path.clone() };
                    view! { <InlineError message=format!("读取{label}失败：{error}") class="tree-state" retry=Callback::new(move |_| fetch.with_value(|load| load(path.clone())))/> }
                }).collect_view()}
            </Show>
            {move || (root_open.get() || !filter.with(|f| f.trim().is_empty())).then(|| match list() {
                None => ().into_any(),
                Some(r) if r.is_empty() && loading.with(HashSet::is_empty) && errors.with(HashMap::is_empty) => view! { <EmptyState title=if filter.with(|f| f.trim().is_empty()) { "空目录" } else { "没有匹配的文件" }/> }.into_any(),
                Some(r) => r.into_iter().map(|row| {
                    let (mark, cls) = change_mark(row.change);
                    let p = row.path.clone();
                    let dir = row.dir;
                    let more = row.more;
                    let sel = {
                        let p = p.clone();
                        move || !dir && files.current.with(|c| c.as_deref() == Some(p.as_str()))
                    };
                    let accessible_sel = sel.clone();
                    let click = move |_| {
                        if more {
                        } else if dir {
                            opened.update(|o| if !o.remove(&p) { o.insert(p.clone()); });
                        } else {
                            files.open(p.clone(), 0);
                        }
                    };
                    let menu_path = row.path.clone();
                    let menu_name = row.name.clone();
                    let guides = guide_dirs(&row.path, row.depth).into_iter().enumerate().map(|(i, dir)| {
                        let active = move || files.current.with(|c| c.as_deref().is_some_and(|c| c.rsplit_once('/').map_or("", |(parent, _)| parent) == dir));
                        view! { <span class="tn-guide" data-active=move || active().to_string() style=format!("left:calc(var(--tree-base) + {i} * var(--tree-indent) + var(--tree-chev) / 2)")></span> }
                    }).collect_view();
                    view! {
                        <button type="button" class="tn" disabled=more aria-expanded=dir.then(|| row.open.to_string()) aria-pressed=move || (!dir && !more).then(|| accessible_sel().to_string()) data-open=row.open.to_string() data-sel=move || sel().to_string()
                            data-ign=row.ignored.to_string() data-more=more.to_string()
                            data-chg=(dir && row.has_change).to_string()
                            title=row.path.clone() on:click=click
                            on:contextmenu=move |e: leptos::ev::MouseEvent| {
                                e.prevent_default();
                                if !more {
                                    menu.set(Some(Menu { x: e.client_x(), y: e.client_y(), path: menu_path.clone(), name: menu_name.clone(), dir }));
                                }
                            }
                            style=format!("padding-left:calc(var(--tree-base) + {} * var(--tree-indent))", row.depth)>
                            {guides}
                            <span class="chev" inner_html=if dir { CHEV } else { "" }></span>
                            {(!more).then(|| view! { <span class="ic" class:folder=dir inner_html=if dir { if row.open { ICON_FOLDER_OPEN } else { ICON_FOLDER } } else { file_icon(&row.name) }></span> })}
                            <span class="nm" class:dir=dir>{row.name}</span>
                            <span class=format!("ch {cls}")>{mark}</span>
                        </button>
                    }
                }).collect_view().into_any(),
            })}
        </div>
    }
}
