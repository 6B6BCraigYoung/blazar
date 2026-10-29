use std::collections::HashSet;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::empty::Blank;
use crate::components::icon;
use crate::components::menu::menu_keydown;
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::{self, toast};
use crate::components::undo::{self, Request};
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{self, DiffResp};
use crate::diff::{self, File, Kind, Line};
use crate::realtime::use_bus;
use crate::storage;

use super::files::Files;
use super::git_panel::{Git, GitMenuItems, NextButton};
use super::layout::{LayoutState, Tab};

const CHEV_DOWN: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6.5 8 10.5l4-4"/></svg>"#;

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
    ws: StoredValue<String>,
    pub reload: RwSignal<u32>,
    pub loaded: RwSignal<u32>,
    pub focus: RwSignal<Option<(String, u32, u32)>>,
    pub mark: RwSignal<Option<(String, u32)>>,
}

impl DiffState {
    pub fn new(ws: &str) -> Self {
        let d = Self {
            files: RwSignal::new(Vec::new()),
            note: RwSignal::new(String::new()),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            truncated: RwSignal::new(false),
            base: RwSignal::new(String::new()),
            prefs: RwSignal::new(storage::load(PREFS_KEY).unwrap_or_default()),
            comments: RwSignal::new(Vec::new()),
            ws: StoredValue::new(ws.to_owned()),
            reload: RwSignal::new(0),
            loaded: RwSignal::new(0),
            focus: RwSignal::new(None),
            mark: RwSignal::new(None),
        };
        d.load_comments();
        d
    }

    pub fn locate(self, path: String, line: u32) {
        self.focus
            .set(Some((path, line, self.loaded.get_untracked())));
        self.reload.update(|n| *n += 1);
    }

    fn set_prefs(self, f: impl FnOnce(&mut Prefs)) {
        self.prefs.update(f);
        storage::save(PREFS_KEY, &self.prefs.get_untracked());
    }

    fn comments_path(self) -> String {
        format!("/api/workspaces/{}/review-comments", self.ws.get_value())
    }

    fn load_comments(self) {
        let path = self.comments_path();
        let key = review_key(&self.ws.get_value());
        spawn_local(async move {
            // 意见以前只存在这个浏览器里：第一次打开时搬到 hub 上，搬完清掉。
            let old: Vec<Comment> = storage::load(&key).unwrap_or_default();
            let moved = old.is_empty()
                || api::send::<Value>("POST", &path, &json!({ "comments": old }))
                    .await
                    .is_ok();
            if moved {
                storage::remove(&key);
            }
            let mut list = api::get::<Vec<Comment>>(&path).await.unwrap_or_default();
            if !moved {
                list.extend(old);
            }
            let _ = self.comments.try_update(|cs| {
                for c in cs.drain(..) {
                    if !list.iter().any(|x| x.id == c.id) {
                        list.push(c);
                    }
                }
                *cs = list;
            });
        });
    }

    fn save_comment(self, c: Comment) {
        let path = self.comments_path();
        spawn_local(async move {
            if let Err(e) = api::send::<Value>("POST", &path, &json!({ "comments": [c] })).await {
                toast(format!("意见没存上：{e}"));
            }
        });
    }

    fn delete_comment(self, id: &str) {
        let Some((at, c)) = self
            .comments
            .with_untracked(|l| l.iter().position(|x| x.id == id).map(|i| (i, l[i].clone())))
        else {
            return;
        };
        self.comments.update(|l| {
            l.remove(at);
        });
        let path = format!("{}/{}", self.comments_path(), api::enc(id));
        undo::later(
            "已删除意见",
            Request::delete(path),
            move || {
                let c = c.clone();
                let _ = self.comments.try_update(|l| {
                    if !l.iter().any(|x| x.id == c.id) {
                        l.insert(at.min(l.len()), c.clone());
                    }
                });
                // 这 5 秒里意见可能已经随消息发出、整批清掉了，再存一次。
                self.save_comment(c);
            },
            || {},
        );
    }

    /// 意见随消息发出去之后清空，hub 上的也一起清掉。
    pub fn clear_comments(self) {
        self.comments.set(Vec::new());
        let path = self.comments_path();
        spawn_local(async move {
            let _ = api::send::<Value>("DELETE", &path, &json!({})).await;
        });
    }
}

pub fn keep_loaded(ws: String, d: DiffState, git: Git, state: LayoutState) {
    let bus = use_bus();
    let latest = std::rc::Rc::new(latest::Latest::default());
    let inflight = StoredValue::new(None::<futures::future::AbortHandle>);
    let running = StoredValue::new(false);
    on_cleanup(move || {
        if let Some(h) = inflight.try_update_value(Option::take).flatten() {
            h.abort();
        }
    });
    let refresh = move || {
        d.reload.update(|n| *n += 1);
        git.reload.update(|n| *n += 1);
    };
    crate::refocus::on_refocus(refresh);
    Effect::new(move |was: Option<bool>| {
        let shown = state.lay.with(|l| l.inspect && l.tab == Tab::Changes);
        if shown && was == Some(false) {
            refresh();
        }
        shown
    });
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
        // 同一工作区只留一个 diff 在路上：旧的取消掉，连续触发合并成一次。
        if let Some(h) = inflight.try_update_value(Option::take).flatten() {
            h.abort();
        }
        let busy = running.try_update_value(|r| std::mem::replace(r, true)) == Some(true);
        let (task, handle) = futures::future::abortable(async move {
            if busy {
                gloo_timers::future::TimeoutFuture::new(250).await;
            }
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
            running.try_set_value(false);
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
            let _ = d.loaded.try_update(|n| *n += 1);
        });
        inflight.set_value(Some(handle));
        spawn_local(async move {
            let _ = task.await;
        });
    });
}

/// 工具栏上的一个下拉：按钮加菜单，焦点离开就收起。
#[component]
fn BarMenu(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] aria: String,
    #[prop(optional)] more: bool,
    #[prop(optional, into)] title: Option<String>,
    #[prop(into)] class: String,
    open: RwSignal<bool>,
    children: ChildrenFn,
) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let button = NodeRef::<html::Button>::new();
    let close = Callback::new(move |()| {
        open.set(false);
        if let Some(b) = button.get_untracked() {
            let _ = b.focus();
        }
    });
    let aria_menu = aria.clone();
    view! {
        <span
            class="more"
            on:focusout=move |e| {
                let inside = e
                    .related_target()
                    .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                    .zip(e.current_target().and_then(|t| t.dyn_into::<web_sys::Node>().ok()))
                    .is_some_and(|(t, me)| me.contains(Some(&t)));
                if !inside {
                    open.set(false);
                }
            }
        >
            <button
                type="button"
                class=if more { "btn ghost more-btn" } else { "dv-btn" }
                node_ref=button
                title=title
                aria-label=more.then(|| aria.clone())
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)
            >
                {move || label.get()}
                {(!more)
                    .then(|| {
                        view! { <span class="chev" aria-hidden="true" inner_html=CHEV_DOWN></span> }
                    })}
            </button>
            <Show when=move || open.get()>
                <div
                    class=format!("menu dv-menu {class}")
                    node_ref=root
                    role="menu"
                    aria-label=aria_menu.clone()
                    on:keydown=move |event| {
                        if let Some(r) = root.get_untracked() {
                            menu_keydown(&event, r.unchecked_ref(), close);
                        }
                    }
                >
                    {children()}
                </div>
            </Show>
        </span>
    }
}

#[component]
pub fn DiffBar(
    d: DiffState,
    git: Git,
    collapsed: RwSignal<HashSet<String>>,
    draft: RwSignal<Option<String>>,
    state: LayoutState,
) -> impl IntoView {
    let empty = move || d.files.with(Vec::is_empty);
    let all_folded = move || !empty() && collapsed.with(HashSet::len) >= d.files.with(Vec::len);
    let fold_all = move || {
        let all = d
            .files
            .with_untracked(|f| f.iter().map(|x| x.path.clone()).collect::<HashSet<_>>());
        collapsed.update(|c| {
            if c.len() >= all.len() {
                c.clear()
            } else {
                *c = all
            }
        });
    };
    let act = move |what: &str| {
        match what {
            "diffview" => d.set_prefs(|p| {
                p.view = if p.view == "split" {
                    "unified"
                } else {
                    "split"
                }
                .into();
            }),
            "diffws" => d.set_prefs(|p| p.w = !p.w),
            "diffwrap" => d.set_prefs(|p| p.wrap = !p.wrap),
            "difflist" => d.set_prefs(|p| p.tree = !p.tree),
            "difffold" => fold_all(),
            _ => return false,
        }
        true
    };
    let keys = window_event_listener(ev::keydown, move |e| {
        let shown = state
            .lay
            .try_with_untracked(|l| l.inspect && l.tab == Tab::Changes);
        if shown != Some(true)
            || e.target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                .is_some_and(|t| t.closest(".monaco-editor, .xterm").ok().flatten().is_some())
        {
            return;
        }
        if let Some(a) = crate::shortcuts::action(&e)
            && act(a)
        {
            e.prevent_default();
        }
    });
    on_cleanup(move || keys.remove());
    let show_open = RwSignal::new(false);
    let base_open = RwSignal::new(false);
    let more_open = RwSignal::new(false);
    let pick = move |what: &'static str| {
        move |_| {
            show_open.set(false);
            act(what);
        }
    };
    let hint = |id: &str| crate::shortcuts::label(&crate::shortcuts::current(id));
    let review = move |_| {
        more_open.set(false);
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
                .map(super::git_panel::short_target)
        })
    };
    let set_base = move |b: &'static str| {
        move |_| {
            base_open.set(false);
            d.set_prefs(|p| p.base = b.into());
        }
    };
    let base_label = Signal::derive(move || {
        if p().base == "target" {
            "这条分支的全部改动".to_owned()
        } else {
            "这次的改动".to_owned()
        }
    });
    let close_more = Callback::new(move |()| more_open.set(false));
    view! {
        <div class="diffbar">
            <BarMenu label=base_label aria="比较的范围" class="dv-base" open=base_open>
                <button
                    type="button"
                    role="menuitemradio"
                    aria-checked=move || (p().base != "target").to_string()
                    on:click=set_base("head")
                >
                    <span>"这次的改动"</span>
                    <small>"还没提交的部分"</small>
                </button>
                <button
                    type="button"
                    role="menuitemradio"
                    aria-checked=move || (p().base == "target").to_string()
                    on:click=set_base("target")
                >
                    <span>
                        {move || {
                            target()
                                .map_or_else(
                                    || "这条分支的全部改动".to_owned(),
                                    |t| format!("这条分支的全部改动（对比 {t}）"),
                                )
                        }}
                    </span>
                    <small>"包括已经提交的部分"</small>
                </button>
            </BarMenu>
            <span class="sum">
                {move || {
                    let (n, a, r) = sums();
                    view! {
                        {format!("{n} 个文件 ")}
                        <b class="ok">{format!("+{a}")}</b>
                        " "
                        <b class="bad">{format!("−{r}")}</b>
                    }
                }}
            </span>
            {move || {
                let n = d.comments.with(Vec::len);
                (n > 0)
                    .then(|| {
                        view! {
                            <span class="pill-note" title="随下一条消息一起发给智能体">
                                {format!("{n} 条意见待发")}
                            </span>
                        }
                    })
            }}
            <span class="grow"></span>
            <BarMenu label="显示" aria="显示" class="" open=show_open>
                <button
                    type="button"
                    role="menuitemradio"
                    aria-checked=move || (p().view != "split").to_string()
                    on:click=pick("diffview")
                >
                    <span>"统一"</span>
                    <kbd>{hint("diffview")}</kbd>
                </button>
                <button
                    type="button"
                    role="menuitemradio"
                    aria-checked=move || (p().view == "split").to_string()
                    on:click=pick("diffview")
                >
                    <span>"并排"</span>
                    <kbd>{hint("diffview")}</kbd>
                </button>
                <div class="menu-sep"></div>
                <button
                    type="button"
                    role="menuitemcheckbox"
                    aria-checked=move || p().w.to_string()
                    on:click=pick("diffws")
                >
                    <span>"忽略空白"</span>
                    <kbd>{hint("diffws")}</kbd>
                </button>
                <button
                    type="button"
                    role="menuitemcheckbox"
                    aria-checked=move || p().wrap.to_string()
                    on:click=pick("diffwrap")
                >
                    <span>"自动换行"</span>
                    <kbd>{hint("diffwrap")}</kbd>
                </button>
                <button
                    type="button"
                    role="menuitemcheckbox"
                    aria-checked=move || p().tree.to_string()
                    on:click=pick("difflist")
                >
                    <span>"改动文件列表"</span>
                    <kbd>{hint("difflist")}</kbd>
                </button>
                <div class="menu-sep"></div>
                <button type="button" role="menuitem" disabled=empty on:click=pick("difffold")>
                    <span>
                        {move || { if all_folded() { "全部展开" } else { "全部折叠" } }}
                    </span>
                    <kbd>{hint("difffold")}</kbd>
                </button>
            </BarMenu>
            <BarMenu
                label="⋯"
                aria="更多操作"
                more=true
                title="更多"
                class="dv-more"
                open=more_open
            >
                <button
                    type="button"
                    role="menuitem"
                    disabled=empty
                    title="开一个新对话，把这些改动交给智能体只审阅、不改代码"
                    on:click=review
                >
                    <span>"让智能体审阅…"</span>
                </button>
                <GitMenuItems git close=close_more />
            </BarMenu>
            <NextButton git />
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

fn new_side_len(h: &diff::Hunk) -> u32 {
    h.lines
        .iter()
        .filter(|l| matches!(l.kind, Kind::Ctx | Kind::Add))
        .count() as u32
}

fn expand_up(ws: String, d: DiffState, path: String, hunk: usize) {
    spawn_local(async move {
        let r = match api::read_file(&ws, &path).await {
            Ok(r) => r,
            Err(e) => {
                toast(format!("没能读取文件：{e}"));
                return;
            }
        };
        if r.binary || r.too_large {
            toast("文件太大或是二进制，展不开");
            return;
        }
        let text: Vec<&str> = r.content.lines().collect();
        d.files.update(|files| {
            let Some(f) = files.iter_mut().find(|f| f.path == path) else {
                return;
            };
            let prev_end = match hunk.checked_sub(1).and_then(|i| f.hunks.get(i)) {
                Some(p) => p.new + new_side_len(p),
                None => 1,
            };
            let Some(h) = f.hunks.get_mut(hunk) else {
                return;
            };
            let n = h.new.saturating_sub(prev_end).min(h.old.saturating_sub(1));
            if n == 0 {
                return;
            }
            let ctx: Vec<Line> = (0..n)
                .map_while(|k| {
                    let new = h.new - n + k;
                    text.get(new as usize - 1).map(|t| Line {
                        kind: Kind::Ctx,
                        old: Some(h.old - n + k),
                        new: Some(new),
                        text: (*t).to_owned(),
                    })
                })
                .collect();
            let got = ctx.len() as u32;
            h.old -= got;
            h.new -= got;
            h.lines.splice(0..0, ctx);
        });
    });
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
    Effect::new(move |_| {
        let Some((path, line, asked)) = d.focus.get() else {
            return;
        };
        if d.loaded.get() <= asked || d.loading.get() {
            return;
        }
        let at = d.files.with(|fs| fs.iter().position(|f| f.path == path));
        d.focus.set(None);
        let Some(i) = at else {
            toast("「更改」里没有这个文件的改动");
            return;
        };
        collapsed.update(|c| {
            c.remove(&path);
        });
        expanded.update(|e| {
            e.insert(path.clone());
        });
        gloo_timers::callback::Timeout::new(80, move || reveal(d, i, path, line)).forget();
    });

    let body = move || {
        if d.loading.get() && d.files.with(Vec::is_empty) {
            return view! { <LoadingState text="读取差异…" /> }.into_any();
        }
        if let Some(error) = d.error.get() {
            return view! {
                <InlineError
                    message=error
                    class="empty"
                    retry=Callback::new(move |_| d.reload.update(|n| *n += 1))
                />
            }
            .into_any();
        }
        let list = d.files.get();
        if list.is_empty() {
            let note = d.note.get();
            let msg = if !note.is_empty() {
                note
            } else if p().base == "target" {
                let b = d.base.get();
                if b.is_empty() {
                    "先在上面选这条分支要合到哪里，才能看它的全部改动".to_owned()
                } else {
                    format!("和 {} 比没有改动", b.strip_prefix("origin/").unwrap_or(&b))
                }
            } else {
                "这次还没有改动".to_owned()
            };
            return view! { <Blank icon=icon::CHECK title=msg inset=true /> }.into_any();
        }
        let split = p().view == "split";
        // 只有和 HEAD 比时丢弃才是“丢掉还没提交的改动”；忽略空白时看到的那一处和文件里的对不上。
        let head = d.base.get().is_empty();
        let drop = Discardable {
            file: head,
            hunk: head && !p().w,
        };
        view! {
            {move || {
                d.truncated
                    .get()
                    .then(|| {
                        view! { <div class="df-note warn">"差异较大，仅显示前 3MB"</div> }
                    })
            }}
            {list
                .into_iter()
                .enumerate()
                .map(|(i, f)| {
                    file_view(i, f, split, drop, collapsed, expanded, editing, d, files, ws_c)
                })
                .collect_view()}
        }
        .into_any()
    };

    let tree = move || {
        let q = filter.get().to_lowercase();
        d.files.with(|fs| {
            fs.iter()
                .enumerate()
                .filter(|(_, f)| q.is_empty() || f.path.to_lowercase().contains(&q))
                .map(|(i, f)| {
                    let (mk, cls) = f.status.mark();
                    let (name, dir) = f
                        .path
                        .rsplit_once('/')
                        .map_or((f.path.clone(), String::new()), |(d, n)| {
                            (n.to_owned(), d.to_owned())
                        });
                    let n = d
                        .comments
                        .with(|c| c.iter().filter(|c| c.file == f.path).count());
                    let path = f.path.clone();
                    view! {
                        <button
                            class="dt-row"
                            title=f.path.clone()
                            on:click=move |_| {
                                collapsed
                                    .update(|c| {
                                        c.remove(&path);
                                    });
                                if let Some(el) = document().get_element_by_id(&format!("df-{i}")) {
                                    el.scroll_into_view();
                                }
                            }
                        >
                            <span class="nm">{name}<span class="dir">{dir}</span></span>
                            {(n > 0).then(|| view! { <span class="cm">{n}</span> })}
                            <span class="st">{format!("+{} −{}", f.added, f.removed)}</span>
                            <span class=format!("ch {cls}")>{mk}</span>
                        </button>
                    }
                })
                .collect_view()
        })
    };

    view! {
        <div class="diffwrap">
            <Show when=move || p().tree && !d.files.with(Vec::is_empty)>
                <div class="difftree">
                    <div class="difftree-flow">
                        <input
                            class="tree-filter"
                            aria-label="筛选改动文件"
                            placeholder="筛选文件…"
                            prop:value=move || filter.get()
                            on:input=move |e| filter.set(event_target_value(&e))
                        />
                        {tree}
                    </div>
                </div>
            </Show>
            <div
                class="diffview"
                data-wrap=move || (p().wrap || p().view == "split").to_string()
                data-view=move || p().view
            >
                <div class="df-flow">{body}</div>
            </div>
        </div>
    }
}

fn reveal(d: DiffState, i: usize, path: String, line: u32) {
    let Some(section) = document().get_element_by_id(&format!("df-{i}")) else {
        return;
    };
    let row = section
        .query_selector_all("[data-ln]")
        .ok()
        .and_then(|rows| {
            (0..rows.length()).find_map(|k| {
                let el = rows.item(k)?.dyn_into::<web_sys::Element>().ok()?;
                let n: u32 = el.get_attribute("data-ln")?.parse().ok()?;
                (n >= line).then_some((el, n))
            })
        });
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_block(if row.is_some() {
        web_sys::ScrollLogicalPosition::Center
    } else {
        web_sys::ScrollLogicalPosition::Start
    });
    let target = row.as_ref().map_or(section, |(el, _)| el.clone());
    target.scroll_into_view_with_scroll_into_view_options(&opts);
    if let Some((row, n)) = row {
        let _ = row.set_attribute("data-focus", "true");
        d.mark.set(Some((path, n)));
        gloo_timers::callback::Timeout::new(1600, move || {
            let _ = row.remove_attribute("data-focus");
            d.mark.set(None);
        })
        .forget();
    }
}

fn marked(d: DiffState, path: &str) -> Option<u32> {
    d.mark
        .get_untracked()
        .filter(|(p, _)| p == path)
        .map(|(_, n)| n)
}

#[derive(Clone, Copy)]
struct Discardable {
    file: bool,
    hunk: bool,
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn hunk_lines(h: &diff::Hunk) -> Vec<String> {
    h.lines
        .iter()
        .map(|l| match l.kind {
            Kind::Add => format!("+{}", l.text),
            Kind::Del => format!("-{}", l.text),
            Kind::Ctx => format!(" {}", l.text),
            Kind::Note => format!("\\ {}", l.text),
        })
        .collect()
}

fn discard(d: DiffState, body: Value, what: String) {
    let ws = d.ws.get_value();
    spawn_local(async move {
        match api::send::<Value>("POST", &format!("/api/workspaces/{ws}/discard"), &body).await {
            Ok(r) => {
                let _ = d.reload.try_update(|n| *n += 1);
                match r["token"].as_str() {
                    Some(token) => {
                        let token = token.to_owned();
                        toast::offer(
                            format!("已丢弃{what}"),
                            "撤销",
                            undo::DELAY_MS,
                            move || restore(d, ws.clone(), token.clone()),
                        );
                    }
                    None => toast(format!("已丢弃{what}，这次没能留下撤销")),
                }
            }
            Err(e) => toast(format!("没丢掉：{e}")),
        }
    });
}

fn restore(d: DiffState, ws: String, token: String) {
    spawn_local(async move {
        let path = format!("/api/workspaces/{ws}/discard/{}/restore", api::enc(&token));
        match api::send::<Value>("POST", &path, &json!({})).await {
            Ok(_) => {
                let _ = d.reload.try_update(|n| *n += 1);
            }
            Err(e) => toast(format!("没撤销：{e}")),
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn file_view(
    i: usize,
    f: File,
    split: bool,
    drop: Discardable,
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
    let drop_body = json!({
        "path": f.path,
        "old_path": (f.status == diff::Status::Renamed).then(|| f.old_path.clone()),
    });
    let drop_what = format!("「{}」的改动", file_name(&f.path));
    let title = if f.status == diff::Status::Renamed {
        format!("{} → {}", f.old_path, f.path)
    } else {
        f.path.clone()
    };
    let f = StoredValue::new(f);
    view! {
        <section class="df" id=format!("df-{i}") data-folded=move || folded.get().to_string()>
            <header
                class="df-head"
                on:click=move |_| {
                    collapsed
                        .update(|c| {
                            if !c.remove(&p_toggle) {
                                c.insert(p_toggle.clone());
                            }
                        })
                }
            >
                <button
                    type="button"
                    class="df-toggle"
                    aria-expanded=move || (!folded.get()).to_string()
                    aria-label=title.clone()
                >
                    <span class="caret" aria-hidden="true"></span>
                    <b class=format!("mk {cls}")>{mk}</b>
                    <span class="path">{title.clone()}</span>
                    <span class="st">
                        {f.with_value(|f| format!("+{} −{}", f.added, f.removed))}
                    </span>
                </button>
                {(!deleted)
                    .then(|| {
                        view! {
                            <button
                                class="linkbtn"
                                title="在编辑器里打开"
                                on:click=move |e| {
                                    e.stop_propagation();
                                    files.open(p_open.clone(), 0);
                                }
                            >
                                "打开"
                            </button>
                        }
                    })}
                {drop
                    .file
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="linkbtn df-drop"
                                title="丢弃这个文件里还没提交的改动，5 秒内可以撤销"
                                on:click=move |e| {
                                    e.stop_propagation();
                                    discard(d, drop_body.clone(), drop_what.clone());
                                }
                            >
                                "丢弃"
                            </button>
                        }
                    })}
            </header>
            {move || {
                (!folded.get())
                    .then(|| {
                        let big = f.with_value(|f| f.lines > BIG)
                            && !expanded.with(|e| e.contains(&p_exp));
                        if big {
                            let p = p_exp.clone();
                            let n = f.with_value(|f| f.lines);
                            return view! {
                                <div class="df-body">
                                    <button
                                        class="df-more"
                                        on:click=move |_| {
                                            expanded
                                                .update(|e| {
                                                    e.insert(p.clone());
                                                })
                                        }
                                    >
                                        {format!("展开 {n} 行改动")}
                                    </button>
                                </div>
                            }
                                .into_any();
                        }
                        view! {
                            <div class="df-body">
                                {f.with_value(|f| file_body(f, split, drop.hunk, editing, d, ws))}
                            </div>
                        }
                            .into_any()
                    })
            }}
        </section>
    }
}

fn file_body(
    f: &File,
    split: bool,
    drop_hunk: bool,
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
    let deleted = f.status == diff::Status::Deleted;
    // 新建和删掉的文件只有一处，丢弃整个文件就是了。
    let drop_hunk = drop_hunk && matches!(f.status, diff::Status::Modified | diff::Status::Renamed);
    let mark = marked(d, &path);
    let mut out: Vec<AnyView> = Vec::new();
    let mut prev_end = 1u32;
    for (hi, h) in f.hunks.iter().enumerate() {
        let (line, above) = if deleted {
            (h.old, 0)
        } else {
            (h.new, h.new.saturating_sub(prev_end))
        };
        prev_end = h.new + new_side_len(h);
        let up = (above > 0).then(|| {
            let p = path.clone();
            view! {
                <button
                    type="button"
                    class="dh-up"
                    on:click=move |_| expand_up(ws.get_value(), d, p.clone(), hi)
                >
                    {format!("展开上方 {above} 行")}
                </button>
            }
        });
        let drop = drop_hunk.then(|| {
            let body = json!({
                "path": path,
                "hunk": { "old": h.old, "new": h.new, "lines": hunk_lines(h) },
            });
            let what = format!("「{}」里的一处改动", file_name(&path));
            view! {
                <button
                    type="button"
                    class="linkbtn df-drop"
                    title="丢弃这一处改动，5 秒内可以撤销"
                    on:click=move |_| discard(d, body.clone(), what.clone())
                >
                    "丢弃"
                </button>
            }
        });
        out.push(
            view! {
                <div class="dh">
                    <span class="dh-ln" title=h.ctx.clone()>
                        {format!("第 {line} 行")}
                    </span>
                    {up}
                    <span class="dh-rule" aria-hidden="true"></span>
                    {drop}
                </div>
            }
            .into_any(),
        );
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
                        out.push(
                            view! {
                                <div class="dr2">
                                    {half(Some(l), "old", &path, editing, mark)}
                                    {half(Some(l), "new", &path, editing, mark)}
                                </div>
                            }
                            .into_any(),
                        );
                        out.push(
                            comments_at(&path, "new", l.new.unwrap_or(0), editing, d).into_any(),
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
                            out.push(
                                view! {
                                    <div class="dr2">
                                        {half(dl, "old", &path, editing, mark)}
                                        {half(al, "new", &path, editing, mark)}
                                    </div>
                                }
                                .into_any(),
                            );
                            if let Some(l) = dl {
                                out.push(
                                    comments_at(&path, "old", l.old.unwrap_or(0), editing, d)
                                        .into_any(),
                                );
                            }
                            if let Some(l) = al {
                                out.push(
                                    comments_at(&path, "new", l.new.unwrap_or(0), editing, d)
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
                    out.push(
                        view! {
                            <div class="dr" data-t="n">
                                <span class="ln"></span>
                                <span class="ln"></span>
                                <span class="dg"></span>
                                <span class="dc muted">{l.text.clone()}</span>
                            </div>
                        }
                        .into_any(),
                    );
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
                    <div
                        class="dr"
                        data-t=t
                        data-ln=l.new.map(|n| n.to_string())
                        data-focus=(mark.is_some() && l.new == mark).then_some("true")
                    >
                        <span class="ln">{l.old.map(|n| n.to_string()).unwrap_or_default()}</span>
                        <span class="ln">
                            {l.new.map(|n| n.to_string()).unwrap_or_default()}
                            {add_btn(&path, side, line, &code, editing)}
                        </span>
                        <span class="dg">{g}</span>
                        <span class="dc">{code.clone()}</span>
                    </div>
                }.into_any());
                out.push(comments_at(&path, side, line, editing, d).into_any());
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
    view! {
        <button
            class="dcm"
            aria-label="添加行级意见"
            title="点行号对这一行写意见"
            on:click=move |_| editing.set(Some(e.clone()))
        ></button>
    }
}

fn half(
    l: Option<&Line>,
    side: &'static str,
    path: &str,
    editing: RwSignal<Option<Editing>>,
    mark: Option<u32>,
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
    let hit = side == "new" && mark == Some(n);
    view! {
        <div
            class="half"
            data-t=t
            data-ln=(side == "new").then(|| n.to_string())
            data-focus=hit.then_some("true")
        >
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
        let boxes = mine
            .into_iter()
            .map(|c| {
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
                            <span class="muted">"随下一条消息发送"</span>
                            <span class="grow"></span>
                            <button
                                class="linkbtn"
                                on:click=move |_| {
                                    editing
                                        .set(
                                            Some(Editing {
                                                file: c2.file.clone(),
                                                side: c2.side.clone(),
                                                line: c2.line,
                                                code: c2.code.clone(),
                                                id: Some(id_edit.clone()),
                                            }),
                                        )
                                }
                            >
                                "编辑"
                            </button>
                            <button class="linkbtn" on:click=move |_| d.delete_comment(&id_del)>
                                "删除"
                            </button>
                        </div>
                    </div>
                }
                .into_any()
            })
            .collect_view();
        let editor = ed.map(|e| view! { <CommentEditor e d editing /> });
        view! {
            {boxes}
            {editor}
        }
    }
}

#[component]
fn CommentEditor(e: Editing, d: DiffState, editing: RwSignal<Option<Editing>>) -> impl IntoView {
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
            let id =
                e.id.clone()
                    .unwrap_or_else(|| format!("{:x}", (js_sys::Math::random() * 1e12) as u64));
            d.comments
                .update(|l| match l.iter_mut().find(|c| c.id == id) {
                    Some(c) => c.text = t,
                    None => l.push(Comment {
                        id: id.clone(),
                        file: e.file,
                        side: e.side,
                        line: e.line,
                        code: e.code,
                        text: t,
                    }),
                });
            if let Some(c) = d
                .comments
                .with_untracked(|l| l.iter().find(|c| c.id == id).cloned())
            {
                d.save_comment(c);
            }
        }
        editing.set(None);
    };
    let done2 = done.clone();
    let done3 = done.clone();
    view! {
        <div class="dcmt-edit">
            <textarea
                node_ref=ta
                rows="3"
                aria-label="行级审阅意见"
                title="⌘↩ 保存"
                placeholder="写下意见…"
                prop:value=move || text.get()
                on:input=move |ev| text.set(event_target_value(&ev))
                on:keydown=move |ev| {
                    if ev.key() == "Escape" {
                        ev.prevent_default();
                        ev.stop_propagation();
                        done(false);
                    }
                    if ev.key() == "Enter" && (ev.meta_key() || ev.ctrl_key()) {
                        ev.prevent_default();
                        done(true);
                    }
                }
            ></textarea>
            <div class="row">
                <span class="muted">"随下一条消息发送给智能体"</span>
                <span class="grow"></span>
                <button class="btn small" on:click=move |_| done2(false)>
                    "取消"
                </button>
                <button class="btn small primary" on:click=move |_| done3(true)>
                    {if is_edit { "保存" } else { "添加意见" }}
                </button>
            </div>
        </div>
    }
}
