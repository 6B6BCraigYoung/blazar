use std::collections::HashSet;

use leptos::prelude::*;

use crate::components::status::{EmptyState, InlineError, LoadingState};
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};

use crate::api::{self, DiffResp};
use crate::diff::{self, File, Kind, Line};
use crate::realtime::use_bus;
use crate::storage;

use super::files::Files;
use super::git_panel::Git;

mod latest;

const PREFS_KEY: &str = "blazar.diffprefs";
const BIG: u32 = 800;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub base: String,
    pub view: String,
    pub w: bool,
    pub wrap: bool,
    pub tree: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            base: "head".into(),
            view: "unified".into(),
            w: false,
            wrap: true,
            tree: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub file: String,
    pub side: String,
    pub line: u32,
    pub code: String,
    pub text: String,
}

fn review_key(ws: &str) -> String {
    format!("blazar.review.{ws}")
}

#[derive(Clone, Copy)]
pub struct DiffState {
    pub files: RwSignal<Vec<File>>,
    pub note: RwSignal<String>,
    pub loading: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub truncated: RwSignal<bool>,
    pub base: RwSignal<String>,
    pub prefs: RwSignal<Prefs>,
    pub comments: RwSignal<Vec<Comment>>,
    pub reload: RwSignal<u32>,
}

impl DiffState {
    pub fn new(ws: &str) -> Self {
        Self {
            files: RwSignal::new(Vec::new()),
            note: RwSignal::new(String::new()),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            truncated: RwSignal::new(false),
            base: RwSignal::new(String::new()),
            prefs: RwSignal::new(storage::load(PREFS_KEY).unwrap_or_default()),
            comments: RwSignal::new(storage::load(&review_key(ws)).unwrap_or_default()),
            reload: RwSignal::new(0),
        }
    }

    fn set_prefs(self, f: impl FnOnce(&mut Prefs)) {
        self.prefs.update(f);
        storage::save(PREFS_KEY, &self.prefs.get_untracked());
    }

    fn save_comments(self, ws: &str) {
        let list = self.comments.get_untracked();
        if list.is_empty() {
            if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
                let _ = s.remove_item(&review_key(ws));
            }
        } else {
            storage::save(&review_key(ws), &list);
        }
    }
}

pub fn keep_loaded(ws: String, d: DiffState, git: Git) {
    let bus = use_bus();
    let latest = std::rc::Rc::new(latest::Latest::default());
    Effect::new(move |_| {
        bus.workspaces.track();
        d.reload.track();
        let (base_kind, w) = d.prefs.with(|p| (p.base.clone(), p.w));
        let target = git.status.with(|g| {
            g.as_ref()
                .filter(|g| g.has_target())
                .map(|g| g.target.clone())
                .unwrap_or_default()
        });
        let base = if base_kind == "target" {
            target
        } else {
            String::new()
        };
        let ws = ws.clone();
        let request = latest.begin();
        d.loading.set(true);
        d.error.set(None);
        let latest = latest.clone();
        spawn_local(async move {
            let mut q = Vec::new();
            if !base.is_empty() {
                q.push(format!("base={}", api::enc(&base)));
            }
            if w {
                q.push("w=1".to_owned());
            }
            let r =
                api::get::<DiffResp>(&format!("/api/workspaces/{ws}/diff?{}", q.join("&"))).await;
            if !latest.accepts(request) {
                return;
            }
            let _ = d.loading.try_set(false);
            match r {
                Ok(r) => {
                    let _ = d.files.try_set(diff::parse(&r.diff));
                    let _ = d.note.try_set(r.reason);
                    let _ = d.truncated.try_set(r.truncated);
                }
                Err(e) => {
                    let _ = d.error.try_set(Some(e.to_string()));
                    let _ = d.files.try_set(Vec::new());
                    let _ = d.note.try_set(e.to_string());
                    let _ = d.truncated.try_set(false);
                }
            }
            let _ = d.base.try_set(base);
        });
    });
}

#[component]
pub fn DiffBar(
    d: DiffState,
    git: Git,
    collapsed: RwSignal<HashSet<String>>,
    panel_max: RwSignal<bool>,
    draft: RwSignal<Option<String>>,
) -> impl IntoView {
    let review = move |_| {
        let n = d.files.with_untracked(Vec::len);
        let base = d.base.get_untracked();
        let scope = if d.prefs.with_untracked(|p| p.base == "target") && !base.is_empty() {
            format!("这条分支相对 {base} 的全部改动（git diff {base}...HEAD，加上还没提交的部分）")
        } else {
            "这个工作区里还没提交的改动（git diff HEAD，包括新建的文件）".to_owned()
        };
        draft.set(Some(format!(
            "请审阅{scope}，一共 {n} 个文件。\n\n重点看：正确性和边界情况、有没有引入回归、错误处理、命名和可读性、该有而没有的测试。\n按严重程度从高到低列出问题，每条给出文件和行号、问题是什么、建议怎么改；没问题的地方不用说。\n只审阅，不要改任何代码。"
        )));
    };
    let p = move || d.prefs.get();
    let sums = move || {
        d.files.with(|f| {
            (
                f.len(),
                f.iter().map(|x| x.added).sum::<u32>(),
                f.iter().map(|x| x.removed).sum::<u32>(),
            )
        })
    };
    let target = move || {
        git.status.with(|g| {
            g.as_ref()
                .filter(|g| g.has_target())
                .map(|g| g.target.clone())
        })
    };
    view! {
        <div class="diffbar">
            <button class="laybtn" aria-label="切换改动文件列表" title="改动文件列表" aria-pressed=move || p().tree.to_string()
                on:click=move |_| d.set_prefs(|p| p.tree = !p.tree) inner_html=super::ICON_LEFT></button>
            <span class="seg" title="和谁比">
                <button data-on=move || (p().base == "head").to_string() title="还没提交的改动（相对 HEAD）"
                    on:click=move |_| d.set_prefs(|p| p.base = "head".into())>"未提交"</button>
                <button data-on=move || (p().base == "target").to_string() title="这条分支相对目标分支一共改了什么"
                    on:click=move |_| d.set_prefs(|p| p.base = "target".into())>
                    {move || target().map_or_else(|| "整条分支".to_owned(), |t| format!("整条分支 · {t}"))}
                </button>
            </span>
            <span class="seg">
                <button data-on=move || (p().view == "unified").to_string() on:click=move |_| d.set_prefs(|p| p.view = "unified".into())>"统一"</button>
                <button data-on=move || (p().view == "split").to_string() on:click=move |_| d.set_prefs(|p| p.view = "split".into())>"并排"</button>
            </span>
            <label class="chk"><input type="checkbox" prop:checked=move || p().w on:change=move |_| d.set_prefs(|p| p.w = !p.w)/>"忽略空白"</label>
            <label class="chk"><input type="checkbox" prop:checked=move || p().wrap on:change=move |_| d.set_prefs(|p| p.wrap = !p.wrap)/>"自动换行"</label>
            <span class="sum">
                {move || { let (n, a, r) = sums(); view! { {format!("{n} 个文件 ")}<b class="ok">{format!("+{a}")}</b>" "<b class="bad">{format!("−{r}")}</b> } }}
            </span>
            <span class="grow"></span>
            {move || {
                let n = d.comments.with(Vec::len);
                (n > 0).then(|| view! { <span class="pill-note" title="随下一条消息一起发给 agent">{format!("{n} 条意见待发")}</span> })
            }}
            <button class="btn small" disabled=move || d.files.with(Vec::is_empty)
                on:click=move |_| {
                    let all = d.files.with_untracked(|f| f.iter().map(|x| x.path.clone()).collect::<HashSet<_>>());
                    collapsed.update(|c| if c.len() >= all.len() { c.clear() } else { *c = all });
                }>
                {move || if !d.files.with(Vec::is_empty) && collapsed.with(HashSet::len) >= d.files.with(Vec::len) { "全部展开" } else { "全部折叠" }}
            </button>
            <button class="btn small" disabled=move || d.files.with(Vec::is_empty) title="开一个新对话，让 agent 只审阅不改代码" on:click=review>"审阅改动"</button>
            <button class="laybtn" aria-label="刷新差异" title="刷新" inner_html=super::ICON_REFRESH
                on:click=move |_| { d.reload.update(|n| *n += 1); git.reload.update(|n| *n += 1); }></button>
            <button class="laybtn" title=move || if panel_max.get() { "还原面板" } else { "最大化面板" }
                aria-pressed=move || panel_max.get().to_string() on:click=move |_| panel_max.update(|m| *m = !*m)>"⤢"</button>
        </div>
    }
}

fn line_code(l: &Line) -> String {
    if l.text.is_empty() {
        " ".to_owned()
    } else {
        l.text.clone()
    }
}

#[derive(Clone, PartialEq)]
struct Editing {
    file: String,
    side: String,
    line: u32,
    code: String,
    id: Option<String>,
}

#[component]
pub fn DiffView(
    ws: String,
    d: DiffState,
    files: Files,
    collapsed: RwSignal<HashSet<String>>,
) -> impl IntoView {
    let expanded = RwSignal::new(HashSet::<String>::new());
    let editing = RwSignal::new(None::<Editing>);
    let filter = RwSignal::new(String::new());
    let p = move || d.prefs.get();
    let ws_c = StoredValue::new(ws);

    let body = move || {
        if d.loading.get() && d.files.with(Vec::is_empty) {
            return view! { <LoadingState text="读取差异…"/> }.into_any();
        }
        if let Some(error) = d.error.get() {
            return view! { <InlineError message=error class="empty" retry=Callback::new(move |_| d.reload.update(|n| *n += 1))/> }.into_any();
        }
        let list = d.files.get();
        if list.is_empty() {
            let note = d.note.get();
            let msg = if !note.is_empty() {
                note
            } else if p().base == "target" {
                let b = d.base.get();
                if b.is_empty() {
                    "请在 Git 面板选择可对比的目标分支".to_owned()
                } else {
                    format!("相对 {b} 没有改动")
                }
            } else {
                "没有未提交的改动".to_owned()
            };
            return view! { <EmptyState title=msg/> }.into_any();
        }
        let split = p().view == "split";
        view! {
            {move || d.truncated.get().then(|| view! { <div class="df-note warn">"差异较大，仅显示前 3MB"</div> })}
            {list.into_iter().enumerate().map(|(i, f)| file_view(i, f, split, collapsed, expanded, editing, d, files, ws_c)).collect_view()}
        }.into_any()
    };

    let tree = move || {
        let q = filter.get().to_lowercase();
        d.files.with(|fs| fs.iter().enumerate().filter(|(_, f)| q.is_empty() || f.path.to_lowercase().contains(&q)).map(|(i, f)| {
            let (mk, cls) = f.status.mark();
            let (name, dir) = f.path.rsplit_once('/').map_or((f.path.clone(), String::new()), |(d, n)| (n.to_owned(), d.to_owned()));
            let n = d.comments.with(|c| c.iter().filter(|c| c.file == f.path).count());
            let path = f.path.clone();
            view! {
                <button class="dt-row" title=f.path.clone() on:click=move |_| {
                    collapsed.update(|c| { c.remove(&path); });
                    if let Some(el) = document().get_element_by_id(&format!("df-{i}")) { el.scroll_into_view(); }
                }>
                    <b class=format!("mk {cls}")>{mk}</b>
                    <span class="nm">{name}<span class="dir">{dir}</span></span>
                    {(n > 0).then(|| view! { <span class="cm">{n}</span> })}
                    <span class="st"><i class="ok">{format!("+{}", f.added)}</i>" "<i class="bad">{format!("−{}", f.removed)}</i></span>
                </button>
            }
        }).collect_view())
    };

    view! {
        <div class="diffwrap">
            <Show when=move || p().tree && !d.files.with(Vec::is_empty)>
                <div class="difftree">
                    <input class="tree-filter" aria-label="筛选改动文件" placeholder="筛选文件…" prop:value=move || filter.get()
                        on:input=move |e| filter.set(event_target_value(&e))/>
                    {tree}
                </div>
            </Show>
            <div class="diffview" data-wrap=move || (p().wrap || p().view == "split").to_string() data-view=move || p().view>
                {body}
            </div>
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
fn file_view(
    i: usize,
    f: File,
    split: bool,
    collapsed: RwSignal<HashSet<String>>,
    expanded: RwSignal<HashSet<String>>,
    editing: RwSignal<Option<Editing>>,
    d: DiffState,
    files: Files,
    ws: StoredValue<String>,
) -> impl IntoView {
    let (mk, cls) = f.status.mark();
    let path = f.path.clone();
    let p_fold = path.clone();
    let p_toggle = path.clone();
    let p_open = path.clone();
    let p_exp = path.clone();
    let folded = Signal::derive(move || collapsed.with(|c| c.contains(&p_fold)));
    let deleted = f.status == diff::Status::Deleted;
    let title = if f.status == diff::Status::Renamed {
        format!("{} → {}", f.old_path, f.path)
    } else {
        f.path.clone()
    };
    let f = StoredValue::new(f);
    view! {
        <section class="df" id=format!("df-{i}") data-folded=move || folded.get().to_string()>
            <header class="df-head" on:click=move |_| collapsed.update(|c| if !c.remove(&p_toggle) { c.insert(p_toggle.clone()); })>
                <button type="button" class="df-toggle" aria-expanded=move || (!folded.get()).to_string() aria-label=title.clone()>
                <span class="caret">"▾"</span>
                <b class=format!("mk {cls}")>{mk}</b>
                <span class="path">{title.clone()}</span>
                <span class="st">
                    <i class="ok">{format!("+{}", f.with_value(|f| f.added))}</i>" "
                    <i class="bad">{format!("−{}", f.with_value(|f| f.removed))}</i>
                </span>
                </button>
                {(!deleted).then(|| view! {
                    <button class="linkbtn" title="在编辑器里打开" on:click=move |e| { e.stop_propagation(); files.open(p_open.clone(), 0); }>"打开"</button>
                })}
            </header>
            {move || (!folded.get()).then(|| {
                let big = f.with_value(|f| f.lines > BIG) && !expanded.with(|e| e.contains(&p_exp));
                if big {
                    let p = p_exp.clone();
                    let n = f.with_value(|f| f.lines);
                    return view! {
                        <div class="df-body"><button class="df-more" on:click=move |_| expanded.update(|e| { e.insert(p.clone()); })>
                            {format!("展开 {n} 行改动")}
                        </button></div>
                    }.into_any();
                }
                view! { <div class="df-body">{f.with_value(|f| file_body(f, split, editing, d, ws))}</div> }.into_any()
            })}
        </section>
    }
}

fn file_body(
    f: &File,
    split: bool,
    editing: RwSignal<Option<Editing>>,
    d: DiffState,
    ws: StoredValue<String>,
) -> AnyView {
    if f.binary {
        return view! { <div class="df-note">"二进制文件，不显示内容"</div> }.into_any();
    }
    if f.hunks.is_empty() {
        let msg = if f.status == diff::Status::Renamed {
            format!("从 {} 改名，内容没变", f.old_path)
        } else {
            "没有文本改动（可能只改了权限位）".to_owned()
        };
        return view! { <div class="df-note">{msg}</div> }.into_any();
    }
    let path = f.path.clone();
    let mut out: Vec<AnyView> = Vec::new();
    for h in &f.hunks {
        out.push(view! { <div class="dh">{format!("@@ −{} +{} @@ ", h.old, h.new)}<span>{h.ctx.clone()}</span></div> }.into_any());
        if split {
            let mut i = 0;
            let ls = &h.lines;
            while i < ls.len() {
                let l = &ls[i];
                match l.kind {
                    Kind::Note => {
                        i += 1;
                    }
                    Kind::Ctx => {
                        out.push(view! { <div class="dr2">{half(Some(l), "old", &path, editing)}{half(Some(l), "new", &path, editing)}</div> }.into_any());
                        out.push(
                            comments_at(&path, "new", l.new.unwrap_or(0), editing, d, ws)
                                .into_any(),
                        );
                        i += 1;
                    }
                    _ => {
                        let mut dels = Vec::new();
                        let mut adds = Vec::new();
                        while i < ls.len() && ls[i].kind == Kind::Del {
                            dels.push(&ls[i]);
                            i += 1;
                        }
                        while i < ls.len() && ls[i].kind == Kind::Add {
                            adds.push(&ls[i]);
                            i += 1;
                        }
                        for k in 0..dels.len().max(adds.len()) {
                            let dl = dels.get(k).copied();
                            let al = adds.get(k).copied();
                            out.push(view! { <div class="dr2">{half(dl, "old", &path, editing)}{half(al, "new", &path, editing)}</div> }.into_any());
                            if let Some(l) = dl {
                                out.push(
                                    comments_at(&path, "old", l.old.unwrap_or(0), editing, d, ws)
                                        .into_any(),
                                );
                            }
                            if let Some(l) = al {
                                out.push(
                                    comments_at(&path, "new", l.new.unwrap_or(0), editing, d, ws)
                                        .into_any(),
                                );
                            }
                        }
                    }
                }
            }
        } else {
            for l in &h.lines {
                if l.kind == Kind::Note {
                    out.push(view! { <div class="dr" data-t="n"><span class="ln"></span><span class="ln"></span><span class="dg"></span><span class="dc muted">{l.text.clone()}</span></div> }.into_any());
                    continue;
                }
                let (side, line) = if l.kind == Kind::Del {
                    ("old", l.old.unwrap_or(0))
                } else {
                    ("new", l.new.unwrap_or(0))
                };
                let t = match l.kind {
                    Kind::Add => "+",
                    Kind::Del => "-",
                    _ => " ",
                };
                let g = match l.kind {
                    Kind::Add => "+",
                    Kind::Del => "−",
                    _ => "",
                };
                let code = line_code(l);
                out.push(view! {
                    <div class="dr" data-t=t>
                        <span class="ln">{l.old.map(|n| n.to_string()).unwrap_or_default()}</span>
                        <span class="ln">{l.new.map(|n| n.to_string()).unwrap_or_default()}{add_btn(&path, side, line, &code, editing)}</span>
                        <span class="dg">{g}</span>
                        <span class="dc">{code.clone()}</span>
                    </div>
                }.into_any());
                out.push(comments_at(&path, side, line, editing, d, ws).into_any());
            }
        }
    }
    out.collect_view().into_any()
}

fn add_btn(
    path: &str,
    side: &'static str,
    line: u32,
    code: &str,
    editing: RwSignal<Option<Editing>>,
) -> impl IntoView + use<> {
    let e = Editing {
        file: path.to_owned(),
        side: side.to_owned(),
        line,
        code: code.to_owned(),
        id: None,
    };
    view! { <button class="dcm" aria-label="添加行级意见" title="点行号对这一行写意见" on:click=move |_| editing.set(Some(e.clone()))></button> }
}

fn half(
    l: Option<&Line>,
    side: &'static str,
    path: &str,
    editing: RwSignal<Option<Editing>>,
) -> AnyView {
    let Some(l) = l else {
        return view! { <div class="half" data-t="_"></div> }.into_any();
    };
    let n = if side == "old" { l.old } else { l.new }.unwrap_or(0);
    let t = match l.kind {
        Kind::Add => "+",
        Kind::Del => "-",
        _ => " ",
    };
    let code = line_code(l);
    view! {
        <div class="half" data-t=t>
            <span class="ln">{n.to_string()}{add_btn(path, side, n, &code, editing)}</span>
            <span class="dc">{code.clone()}</span>
        </div>
    }
    .into_any()
}

fn comments_at(
    path: &str,
    side: &'static str,
    line: u32,
    editing: RwSignal<Option<Editing>>,
    d: DiffState,
    ws: StoredValue<String>,
) -> impl IntoView + use<> {
    let path = path.to_owned();
    let here = {
        let path = path.clone();
        move |c: &Comment| c.file == path && c.side == side && c.line == line
    };
    let path2 = path.clone();
    move || {
        let mine: Vec<Comment> = d
            .comments
            .with(|cs| cs.iter().filter(|c| here(c)).cloned().collect());
        let ed = editing
            .get()
            .filter(|e| e.file == path2 && e.side == side && e.line == line);
        let edit_id = ed.as_ref().and_then(|e| e.id.clone());
        let boxes = mine.into_iter().map(|c| {
            if edit_id.as_deref() == Some(c.id.as_str()) {
                return ().into_any();
            }
            let id_edit = c.id.clone();
            let id_del = c.id.clone();
            let c2 = c.clone();
            view! {
                <div class="dcmt">
                    <div class="tx">{c.text.clone()}</div>
                    <div class="ft">
                        <span class="muted">"随下一条消息发送"</span><span class="grow"></span>
                        <button class="linkbtn" on:click=move |_| editing.set(Some(Editing { file: c2.file.clone(), side: c2.side.clone(), line: c2.line, code: c2.code.clone(), id: Some(id_edit.clone()) }))>"编辑"</button>
                        <button class="linkbtn" on:click=move |_| { d.comments.update(|l| l.retain(|x| x.id != id_del)); d.save_comments(&ws.get_value()); }>"删除"</button>
                    </div>
                </div>
            }.into_any()
        }).collect_view();
        let editor = ed.map(|e| view! { <CommentEditor e d editing ws/> });
        view! { {boxes}{editor} }
    }
}

#[component]
fn CommentEditor(
    e: Editing,
    d: DiffState,
    editing: RwSignal<Option<Editing>>,
    ws: StoredValue<String>,
) -> impl IntoView {
    let init =
        e.id.as_ref()
            .and_then(|id| {
                d.comments
                    .with_untracked(|l| l.iter().find(|c| &c.id == id).map(|c| c.text.clone()))
            })
            .unwrap_or_default();
    let text = RwSignal::new(init);
    let is_edit = e.id.is_some();
    let ta = NodeRef::<leptos::html::Textarea>::new();
    Effect::new(move |_| {
        if let Some(t) = ta.get() {
            let _ = t.focus();
        }
    });
    let done = move |ok: bool| {
        let t = text.get_untracked().trim().to_owned();
        if ok && !t.is_empty() {
            let e = e.clone();
            d.comments.update(|l| match &e.id {
                Some(id) => {
                    if let Some(c) = l.iter_mut().find(|c| &c.id == id) {
                        c.text = t;
                    }
                }
                None => l.push(Comment {
                    id: format!("{:x}", (js_sys::Math::random() * 1e12) as u64),
                    file: e.file,
                    side: e.side,
                    line: e.line,
                    code: e.code,
                    text: t,
                }),
            });
            d.save_comments(&ws.get_value());
        }
        editing.set(None);
    };
    let done2 = done.clone();
    let done3 = done.clone();
    view! {
        <div class="dcmt-edit">
            <textarea node_ref=ta rows="3" aria-label="行级审阅意见" title="⌘↩ 保存" placeholder="写下意见…"
                prop:value=move || text.get() on:input=move |ev| text.set(event_target_value(&ev))
                on:keydown=move |ev| {
                    if ev.key() == "Escape" { ev.prevent_default(); ev.stop_propagation(); done(false); }
                    if ev.key() == "Enter" && (ev.meta_key() || ev.ctrl_key()) { ev.prevent_default(); done(true); }
                }></textarea>
            <div class="row">
                <span class="muted">"随下一条消息发送给智能体"</span><span class="grow"></span>
                <button class="btn small" on:click=move |_| done2(false)>"取消"</button>
                <button class="btn small primary" on:click=move |_| done3(true)>{if is_edit { "保存" } else { "添加意见" }}</button>
            </div>
        </div>
    }
}
