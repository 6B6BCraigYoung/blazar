//! 「更改」视图里和提交、推送、PR 有关的部分：右上角的主按钮、⋯ 菜单里的 Git 操作，
//! 以及差异上方那一行分支状态。看改了什么的列表就是差异本身，这里不再列一遍。

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::api::{self, GitStatus, PrDetail};
use crate::components::dialog::{self, Choice};
use crate::components::menu::menu_keydown;
use crate::components::rows::{MenuAt, RowAction, RowMenu};
use crate::components::status::{InlineError, Status, Tone};
use crate::components::toast::toast;
use crate::fmt;

mod forms;
mod presentation;
mod state;

use forms::GitForm;
use presentation::{op_text, resolve_text};

#[derive(Clone, Copy)]
pub struct Git {
    pub status: RwSignal<Option<GitStatus>>,
    pub load_error: RwSignal<Option<String>>,
    pub busy: RwSignal<Option<&'static str>>,
    pub out: RwSignal<Option<(String, bool, String)>>,
    pub pr: RwSignal<Option<PrDetail>>,
    pub reload: RwSignal<u32>,
    pub changed: RwSignal<u32>,
    form: RwSignal<Option<Form>>,
    ws: StoredValue<String>,
}

#[derive(Clone, PartialEq)]
enum Form {
    Commit,
    Pr,
    Rename,
}

/// 现在最该做的下一步，决定右上角主按钮写什么。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    Commit,
    Push,
    Pr,
    Merge,
}

/// 目标分支给人看的名字：`origin/main` 写成 `main`。
pub fn short_target(g: &GitStatus) -> String {
    g.target
        .strip_prefix("origin/")
        .unwrap_or(&g.target)
        .to_owned()
}

#[derive(Clone, Copy)]
struct Can {
    commit: bool,
    push: bool,
    pr: bool,
    merge: bool,
    rebase: bool,
    force: bool,
    fetch: bool,
    rename: bool,
}

fn can(g: &GitStatus) -> Can {
    let idle = g.repo && g.op.is_none();
    let tgt = g.has_target();
    let has_remote = !g.remote.is_empty();
    let has_up = !g.upstream.is_empty();
    let on_branch = !g.detached;
    Can {
        commit: idle && g.uncommitted > 0,
        push: idle && has_remote && on_branch && (!has_up || g.up_ahead > 0),
        pr: idle && has_remote && tgt && on_branch && g.ahead > 0 && g.pr.is_none(),
        merge: idle && tgt && on_branch && g.ahead > 0 && g.target_local,
        rebase: idle && tgt && on_branch && g.behind > 0,
        force: idle && has_remote && on_branch && has_up && g.up_behind > 0,
        fetch: g.repo && has_remote,
        rename: idle && on_branch,
    }
}

fn next(g: &GitStatus) -> Option<Next> {
    let c = can(g);
    let has_up = !g.upstream.is_empty();
    // 有本地提交还没上远端就先推；推过、还没有 PR 就开 PR。
    let unpushed = c.push && ((has_up && g.up_ahead > 0) || (!has_up && g.ahead > 0));
    if c.commit {
        Some(Next::Commit)
    } else if unpushed {
        Some(Next::Push)
    } else if c.pr {
        Some(Next::Pr)
    } else if c.merge {
        Some(Next::Merge)
    } else {
        None
    }
}

impl Git {
    fn snapshot(self) -> Option<GitStatus> {
        self.status.get_untracked()
    }

    fn any_busy(self) -> bool {
        self.busy.with(Option::is_some)
    }

    fn busy_with(self, op: &str) -> bool {
        self.busy.with(|b| *b == Some(op))
    }

    fn start(self, n: Next) {
        match n {
            Next::Commit => self.form.set(Some(Form::Commit)),
            Next::Pr => self.form.set(Some(Form::Pr)),
            Next::Push => self.run("push"),
            Next::Merge => self.run("merge"),
        }
    }

    async fn force_push(self, g: &GitStatus) {
        let n = self
            .status
            .with_untracked(|s| s.as_ref().map_or(0, |s| s.up_behind));
        let body = format!(
            "远端的 {} 上有本地没有的提交，通常是因为本地刚改写过这条分支的历史。\n\n\
             用本地覆盖远端之后，远端那 {} 个提交会从分支上消失；别人已经拉过旧的历史，需要重新对齐。\n\n\
             如果在你上次同步之后别人又推过新提交，这次会被拒绝，不会悄悄盖掉别人的活。",
            g.branch,
            if n > 0 {
                n.to_string()
            } else {
                "几".to_owned()
            }
        );
        if dialog::ask(
            "用本地覆盖远端？",
            &body,
            vec![Choice::plain("取消"), Choice::danger("覆盖远端")],
        )
        .await
            == Some(1)
        {
            let f = self.op("push", serde_json::json!({ "force": true })).await;
            toast(if f.is_some_and(|r| r.ok) {
                "已用本地覆盖远端"
            } else {
                "远端还是拒绝了，详情见「操作失败」"
            });
        }
    }

    fn run(self, op: &'static str) {
        spawn_local(async move {
            let Some(g) = self.snapshot() else {
                return;
            };
            let target = short_target(&g);
            match op {
                "fetch" => {
                    if self
                        .op("fetch", serde_json::json!({}))
                        .await
                        .is_some_and(|r| r.ok)
                    {
                        toast("已同步到最新");
                    }
                }
                "continue" => match self.op("continue", serde_json::json!({})).await {
                    Some(r)
                        if r.ok
                            && self
                                .status
                                .with_untracked(|s| s.as_ref().is_some_and(|s| s.op.is_none())) =>
                    {
                        toast("已完成")
                    }
                    Some(r) if !r.ok => toast("还不能继续，详情见「操作失败」"),
                    _ => {}
                },
                "abort" => {
                    let what = op_text(g.op.as_deref().unwrap_or("")).to_owned();
                    let ok = dialog::ask(
                        &format!("放弃{what}"),
                        &format!(
                            "放弃这次{what}？分支会回到开始之前的样子，已经解决的冲突也不保留。"
                        ),
                        vec![Choice::plain("取消"), Choice::danger("放弃")],
                    )
                    .await;
                    if ok == Some(1)
                        && self
                            .op("abort", serde_json::json!({}))
                            .await
                            .is_some_and(|r| r.ok)
                    {
                        toast("已放弃，分支回到之前的状态");
                    }
                }
                "rebase" => match self.op("rebase", serde_json::json!({})).await {
                    Some(r) if r.ok => toast(format!("已把这条分支挪到 {target} 最新之后")),
                    Some(_) => {
                        let n = self.status.with_untracked(|s| {
                            s.as_ref()
                                .filter(|s| s.op.as_deref() == Some("rebase"))
                                .map(|s| s.conflicts.len())
                        });
                        toast(n.map_or_else(
                            || "没成功，详情见「操作失败」".to_owned(),
                            |n| format!("遇到冲突：{n} 个文件要解决"),
                        ));
                    }
                    None => {}
                },
                "merge" => {
                    let how = if g.behind > 0 {
                        format!(
                            "{target} 上有 {} 个这条分支没有的提交，会多出一个合并提交（有冲突就不合并）。",
                            g.behind
                        )
                    } else {
                        "可以直接合进去，不会多出合并提交。".to_owned()
                    };
                    let body = format!(
                        "把 {} 合并进 {target}？\n\n{how}\n挂在这个工作区上、处于「待审阅」的任务会自动标成完成。",
                        g.branch
                    );
                    if dialog::ask(
                        "合并",
                        &body,
                        vec![Choice::plain("取消"), Choice::plain("合并")],
                    )
                    .await
                        != Some(1)
                    {
                        return;
                    }
                    match self.op("merge", serde_json::json!({})).await {
                        Some(r) if r.ok => toast(if r.tasks_done > 0 {
                            format!("已合并进 {target}，{} 个任务已完成", r.tasks_done)
                        } else {
                            format!("已合并进 {target}")
                        }),
                        Some(_) => toast("没有合并，详情见「操作失败」"),
                        None => {}
                    }
                }
                "push" => match self.op("push", serde_json::json!({})).await {
                    Some(r) if r.ok => toast("已推送"),
                    Some(r) if r.needs_force => self.force_push(&g).await,
                    Some(_) => toast("推送失败，详情见「操作失败」"),
                    None => {}
                },
                "force-push" => self.force_push(&g).await,
                "pr-refresh" => {
                    if self.busy.try_get_untracked().is_none_or(|b| b.is_some()) {
                        return;
                    }
                    self.busy.set(Some("pr-refresh"));
                    let ws = self.ws.get_value();
                    let r =
                        api::get::<serde_json::Value>(&format!("/api/workspaces/{ws}/pr")).await;
                    let _ = self.busy.try_set(None);
                    match r {
                        Ok(v) if v["pr"].is_object() => {
                            let _ = self
                                .pr
                                .try_set(serde_json::from_value(v["pr"].clone()).ok());
                        }
                        Ok(v) => toast(
                            v["reason"]
                                .as_str()
                                .unwrap_or("这条分支上没找到 PR")
                                .to_owned(),
                        ),
                        Err(e) => toast(e.to_string()),
                    }
                }
                _ => {}
            }
        });
    }
}

/// 「更改」右上角唯一的主按钮：提交… / 推送 / 创建 PR… / 合并进 X，按现在的状态只出一个。
#[component]
pub fn NextButton(git: Git) -> impl IntoView {
    move || {
        let (n, target, unpushed) = git.status.with(|g| {
            g.as_ref()
                .and_then(|g| next(g).map(|n| (n, short_target(g), g.up_ahead)))
        })?;
        let label = move || match n {
            Next::Commit => "提交…".to_owned(),
            Next::Push if git.busy_with("push") => "推送中…".to_owned(),
            Next::Push => "推送".to_owned(),
            Next::Pr => "创建 PR…".to_owned(),
            Next::Merge if git.busy_with("merge") => "合并中…".to_owned(),
            Next::Merge => format!("合并进 {target}"),
        };
        let title = match n {
            Next::Commit => "把这次的改动存成一个提交".to_owned(),
            Next::Push if unpushed > 0 => format!("把 {unpushed} 个提交推到远端"),
            Next::Push => "把这条分支推到远端".to_owned(),
            Next::Pr => "请别人审阅并合并这条分支".to_owned(),
            Next::Merge => "在本地把这条分支合进去".to_owned(),
        };
        Some(view! {
            <button
                type="button"
                class="btn small primary"
                title=title
                disabled=move || git.any_busy()
                on:click=move |_| git.start(n)
            >
                {label}
            </button>
        })
    }
}

/// 「更改」⋯ 菜单里的 Git 部分。主按钮已经露出来的那一项不再重复；变基和强制推送收在「高级」里。
#[component]
pub fn GitMenuItems(git: Git, close: Callback<()>) -> impl IntoView {
    let advanced = RwSignal::new(false);
    let item = move |label: String, title: &'static str, act: Callback<()>| {
        view! {
            <button
                type="button"
                role="menuitem"
                title=title
                disabled=move || git.any_busy()
                on:click=move |_| {
                    close.run(());
                    act.run(());
                }
            >
                <span>{label}</span>
            </button>
        }
    };
    move || {
        let g = git.status.get()?;
        if !g.repo {
            return None;
        }
        let c = can(&g);
        let primary = next(&g);
        let target = short_target(&g);
        let mut items: Vec<AnyView> = Vec::new();
        let off = |n: Next| primary != Some(n);
        if c.commit && off(Next::Commit) {
            items.push(
                item(
                    "提交…".into(),
                    "把这次的改动存成一个提交",
                    Callback::new(move |()| git.start(Next::Commit)),
                )
                .into_any(),
            );
        }
        if c.push && off(Next::Push) {
            items.push(
                item(
                    "推送".into(),
                    "把这条分支推到远端",
                    Callback::new(move |()| git.run("push")),
                )
                .into_any(),
            );
        }
        if c.pr && off(Next::Pr) {
            items.push(
                item(
                    "创建 PR…".into(),
                    "请别人审阅并合并这条分支",
                    Callback::new(move |()| git.start(Next::Pr)),
                )
                .into_any(),
            );
        }
        if c.merge && off(Next::Merge) {
            items.push(
                item(
                    format!("合并进 {target}"),
                    "在本地把这条分支合进去",
                    Callback::new(move |()| git.run("merge")),
                )
                .into_any(),
            );
        }
        if c.fetch {
            items.push(
                item(
                    if git.busy_with("fetch") {
                        "同步中…".into()
                    } else {
                        "同步到最新".into()
                    },
                    "从远端取回最新的状态，不改你的文件",
                    Callback::new(move |()| git.run("fetch")),
                )
                .into_any(),
            );
        }
        if c.rename {
            items.push(
                item(
                    "改分支名…".into(),
                    "",
                    Callback::new(move |()| git.form.set(Some(Form::Rename))),
                )
                .into_any(),
            );
        }
        items.push(
            item(
                "刷新状态".into(),
                "",
                Callback::new(move |()| git.reload.update(|n| *n += 1)),
            )
            .into_any(),
        );
        let adv = (c.rebase || c.force).then(|| {
            let t2 = target.clone();
            view! {
                <button
                    type="button"
                    role="menuitem"
                    class="menu-sub"
                    aria-expanded=move || advanced.get().to_string()
                    on:click=move |_| advanced.update(|a| *a = !*a)
                >
                    <span>"高级"</span>
                    <span class="chev" aria-hidden="true">
                        {move || if advanced.get() { "▾" } else { "▸" }}
                    </span>
                </button>
                <Show when=move || {
                    advanced.get()
                }>
                    {c
                        .rebase
                        .then(|| {
                            let t = t2.clone();
                            view! {
                                <button
                                    type="button"
                                    role="menuitem"
                                    class="menu-adv"
                                    disabled=move || git.any_busy()
                                    on:click=move |_| {
                                        close.run(());
                                        git.run("rebase");
                                    }
                                >
                                    <span>{format!("变基到 {t}")}</span>
                                    <small>
                                        {format!(
                                            "把这条分支的提交挪到 {t} 最新之后，可能要解决冲突",
                                        )}
                                    </small>
                                </button>
                            }
                        })}
                    {c
                        .force
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    role="menuitem"
                                    class="menu-adv"
                                    disabled=move || git.any_busy()
                                    on:click=move |_| {
                                        close.run(());
                                        git.run("force-push");
                                    }
                                >
                                    <span>"强制推送"</span>
                                    <small>
                                        "用本地覆盖远端的这条分支，远端多出的提交会丢掉"
                                    </small>
                                </button>
                            }
                        })}
                </Show>
            }
        });
        Some(view! {
            <div class="menu-sep"></div>
            {items}
            {adv}
        })
    }
}

/// 差异上方的一行：在哪条分支、要合到哪里、推没推、PR 怎样；进行到一半的合并或变基也在这里。
#[component]
pub fn GitLine(git: Git, draft: RwSignal<Option<String>>) -> impl IntoView {
    let menu = RwSignal::new(None::<Vec<String>>);
    let menu_trigger = NodeRef::<html::Button>::new();
    let branch_menu = RwSignal::new(None::<MenuAt>);
    let commits_open = RwSignal::new(false);
    let any_busy = move || git.any_busy();
    let busy = move |op: &str| git.busy_with(op);

    let pick_target = move || {
        let Some(g) = git.snapshot() else {
            return;
        };
        let ws = git.ws.get_value();
        spawn_local(async move {
            match api::get::<serde_json::Value>(&format!("/api/workspaces/{ws}/git/branches")).await
            {
                Ok(v) => {
                    let list = v["branches"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .filter(|b| *b != g.branch)
                                .take(40)
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = menu.try_set(Some(list));
                }
                Err(e) => toast(e.to_string()),
            }
        });
    };

    let set_target = move |b: String| {
        menu.set(None);
        spawn_local(async move {
            if git
                .op("set-target", serde_json::json!({ "branch": b }))
                .await
                .is_some_and(|r| r.ok)
            {
                toast(if b.is_empty() {
                    "要合到哪里改回自动判断".to_owned()
                } else {
                    format!("这条分支要合到 {}", b.strip_prefix("origin/").unwrap_or(&b))
                });
            }
        });
    };

    let line = move |g: &GitStatus| {
        let tgt = g.has_target();
        let target = short_target(g);
        let branch = if g.detached {
            format!("没在任何分支上 · {}", g.head)
        } else {
            g.branch.clone()
        };
        let can_rename = !g.detached && g.op.is_none();
        let target_label = if g.target.is_empty() {
            "选一个分支".to_owned()
        } else {
            target.clone()
        };
        let guess = g.target_guess && !g.target.is_empty();
        let branch_name = g.branch.clone();
        let n_commits = g.commits.len();
        let commits_label = if tgt {
            format!("{} 个提交", g.ahead)
        } else {
            format!(
                "{n_commits}{} 个提交",
                if n_commits >= 40 { "+" } else { "" }
            )
        };
        let behind = (tgt && g.behind > 0).then(|| {
            view! {
                <Status
                    tone=Tone::Attention
                    label=format!("{target} 上有 {} 个新提交", g.behind)
                    title="这条分支还没有这些提交，合并或创建 PR 时会一起处理"
                        .to_owned()
                />
            }
        });
        let sync = if g.remote.is_empty() {
            Some(view! { <span>"没有远端"</span> }.into_any())
        } else if g.detached {
            None
        } else if g.upstream.is_empty() {
            Some(view! { <span>"还没推送过"</span> }.into_any())
        } else if g.up_behind > 0 {
            let label = if g.up_ahead > 0 {
                "和远端各有对方没有的提交".to_owned()
            } else {
                format!("远端有 {} 个本地没有的提交", g.up_behind)
            };
            Some(view! { <Status tone=Tone::Failed label /> }.into_any())
        } else if g.up_ahead > 0 {
            Some(view! { <span>{format!("{} 个提交还没推送", g.up_ahead)}</span> }.into_any())
        } else {
            Some(view! { <span>"已推送"</span> }.into_any())
        };
        let running =
            (g.running > 0).then(|| view! { <Status tone=Tone::Run label="智能体运行中" /> });
        view! {
            <div class="gp-line">
                <span
                    class="gp-br"
                    title=if can_rename { "当前分支（右键改名）" } else { "当前分支" }
                    on:contextmenu=move |e: leptos::ev::MouseEvent| {
                        if can_rename {
                            e.prevent_default();
                            branch_menu.set(Some(MenuAt::from(&e, 0)));
                        }
                    }
                >
                    <svg
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.7"
                        stroke-linecap="round"
                    >
                        <circle cx="6" cy="5" r="2" />
                        <circle cx="6" cy="19" r="2" />
                        <circle cx="18" cy="8" r="2" />
                        <path d="M6 7v10M18 10c0 5-12 3-12 7" />
                    </svg>
                    <b>{branch}</b>
                </span>
                {move || {
                    branch_menu
                        .get()
                        .map(|at| {
                            let name = branch_name.clone();
                            view! {
                                <RowMenu
                                    at
                                    label="分支"
                                    groups=vec![
                                        vec![
                                            RowAction::new(
                                                "改名…",
                                                move || git.form.set(Some(Form::Rename)),
                                            ),
                                            RowAction::new(
                                                "复制分支名",
                                                move || {
                                                    let name = name.clone();
                                                    spawn_local(async move {
                                                        let p = window().navigator().clipboard().write_text(&name);
                                                        if wasm_bindgen_futures::JsFuture::from(p).await.is_ok() {
                                                            toast("已复制分支名");
                                                        }
                                                    });
                                                },
                                            ),
                                        ],
                                    ]
                                    on_close=Callback::new(move |()| branch_menu.set(None))
                                />
                            }
                        })
                }}
                <span class="more">
                    <button
                        class="gp-target"
                        node_ref=menu_trigger
                        aria-haspopup="menu"
                        aria-expanded=move || menu.get().is_some().to_string()
                        title="这条分支最终要合到哪里。「这条分支的全部改动」、合并和 PR 都以它为准"
                        on:click=move |_| pick_target()
                    >
                        "合到 "
                        <b>{target_label}</b>
                        {guess.then(|| view! { <i>" 自动"</i> })}
                        " ▾"
                    </button>
                    {move || {
                        menu.get()
                            .map(|list| {
                                let cur = git
                                    .status
                                    .with_untracked(|s| {
                                        s.as_ref().map(|s| s.target.clone()).unwrap_or_default()
                                    });
                                let menu_root = NodeRef::<html::Div>::new();
                                view! {
                                    <div
                                        class="menu"
                                        node_ref=menu_root
                                        role="menu"
                                        aria-label="要合到哪里"
                                        on:keydown=move |event| {
                                            if let Some(root) = menu_root.get_untracked() {
                                                menu_keydown(
                                                    &event,
                                                    root.unchecked_ref(),
                                                    Callback::new(move |_| {
                                                        menu.set(None);
                                                        if let Some(trigger) = menu_trigger.get_untracked() {
                                                            let _ = trigger.focus();
                                                        }
                                                    }),
                                                );
                                            }
                                        }
                                        on:mouseleave=move |_| menu.set(None)
                                    >
                                        <button
                                            role="menuitem"
                                            title="main、master 或远端的默认分支"
                                            on:click=move |_| set_target(String::new())
                                        >
                                            "自动选择"
                                        </button>
                                        <div class="menu-sep"></div>
                                        {list
                                            .into_iter()
                                            .map(|b| {
                                                let label = if b == cur {
                                                    format!("✓ {b}")
                                                } else {
                                                    b.clone()
                                                };
                                                view! {
                                                    <button
                                                        role="menuitem"
                                                        on:click=move |_| set_target(b.clone())
                                                    >
                                                        {label}
                                                    </button>
                                                }
                                            })
                                            .collect_view()}
                                    </div>
                                }
                            })
                    }}
                </span>
                {(n_commits > 0)
                    .then(|| {
                        view! {
                            <button
                                type="button"
                                class="gp-link"
                                aria-expanded=move || commits_open.get().to_string()
                                title=if tgt {
                                    format!("这条分支有、{target} 没有的提交")
                                } else {
                                    "这条分支上的提交".to_owned()
                                }
                                on:click=move |_| commits_open.update(|o| *o = !*o)
                            >
                                {commits_label}
                                " ▾"
                            </button>
                        }
                    })}
                {behind}
                {sync}
                {running}
            </div>
        }
    };

    let commits = move |g: &GitStatus| {
        (commits_open.get() && !g.commits.is_empty()).then(|| {
            let rows = g
                .commits
                .iter()
                .map(|c| {
                    let when =
                        js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(c.at as f64 * 1000.0))
                            .to_iso_string();
                    view! {
                        <div class="gp-commit">
                            <code>{c.sha.clone()}</code>
                            <span class="sj" title=c.subject.clone()>
                                {c.subject.clone()}
                            </span>
                            <span class="muted small">
                                {format!("{} · {}", c.author, fmt::ago(&String::from(when)))}
                            </span>
                        </div>
                    }
                })
                .collect_view();
            view! { <div class="gp-commits">{rows}</div> }
        })
    };

    let op_bar = move |g: &GitStatus| {
        g.op.clone().map(|op| {
            let what = op_text(&op).to_owned();
            let n = g.conflicts.len();
            view! {
                <div class="gp-op">
                    <span class="dot" data-tone="attention" aria-hidden="true"></span>
                    <b>{format!("{what}进行到一半")}</b>
                    <span class="muted">
                        {if n > 0 {
                            format!("{n} 个文件有冲突，解决后点「继续」")
                        } else {
                            "冲突都解决了，可以继续".to_owned()
                        }}
                    </span>
                    <span class="grow"></span>
                    {(n > 0)
                        .then(|| {
                            let g2 = g.clone();
                            view! {
                                <button
                                    class="btn small primary"
                                    on:click=move |_| draft.set(Some(resolve_text(&g2)))
                                >
                                    "协助解决"
                                </button>
                            }
                        })}
                    <button
                        class="btn small"
                        disabled=any_busy
                        on:click=move |_| git.run("continue")
                    >
                        {move || if busy("continue") { "继续中…" } else { "继续" }}
                    </button>
                    <button
                        class="btn small danger"
                        disabled=any_busy
                        on:click=move |_| git.run("abort")
                    >
                        {format!("放弃{what}")}
                    </button>
                </div>
            }
        })
    };

    let pr_line = move |g: &GitStatus| {
        g.pr.clone().map(|pr| {
            move || {
                let d = git.pr.get();
                let state = d
                    .as_ref()
                    .map(|d| d.state.clone())
                    .or_else(|| pr.state.clone())
                    .unwrap_or_default();
                let state_label = match state.as_str() {
                    "OPEN" => "待合并",
                    "MERGED" => "已合并",
                    "CLOSED" => "已关闭",
                    s => s,
                }
                .to_owned();
                let extra = d.map(|d| {
                    let checks = if d.checks.failed > 0 {
                        Some((Tone::Failed, format!("{} 项检查失败", d.checks.failed)))
                    } else if d.checks.pending > 0 {
                        Some((Tone::Run, format!("{} 项检查进行中", d.checks.pending)))
                    } else if d.checks.pass > 0 {
                        Some((Tone::Other, format!("{} 项检查通过", d.checks.pass)))
                    } else {
                        None
                    };
                    let review = match d.review.as_str() {
                        "APPROVED" => Some((Tone::Other, "已批准")),
                        "CHANGES_REQUESTED" => Some((Tone::Attention, "要求修改")),
                        _ => None,
                    };
                    view! {
                        {d.draft.then(|| view! { <span>"草稿"</span> })}
                        {checks.map(|(tone, t)| view! { <Status tone label=t /> })}
                        {review.map(|(tone, t)| view! { <Status tone label=t /> })}
                        {(d.mergeable == "CONFLICTING")
                            .then(|| {
                                view! {
                                    <Status
                                        tone=Tone::Failed
                                        label="和要合到的分支有冲突"
                                    />
                                }
                            })}
                        <span class="gp-prt">{d.title}</span>
                    }
                });
                view! {
                    <div class="gp-line gp-pr" data-s=state.to_lowercase()>
                        <b>
                            {format!(
                                "PR #{}",
                                pr.number.map(|n| n.to_string()).unwrap_or_default(),
                            )}
                        </b>
                        <span>{state_label}</span>
                        {extra}
                        <span class="grow"></span>
                        <a class="linkbtn" href=pr.url.clone() target="_blank" rel="noopener">
                            "在网页上打开"
                        </a>
                        <button class="linkbtn" on:click=move |_| git.run("pr-refresh")>
                            {move || {
                                if busy("pr-refresh") { "查询中…" } else { "刷新状态" }
                            }}
                        </button>
                    </div>
                }
            }
        })
    };

    view! {
        <div class="gitline">
            {move || match git.status.get() {
                None => {
                    view! { <div class="gp-line muted">"读取分支状态…"</div> }.into_any()
                }
                Some(_) if git.load_error.with(Option::is_some) => {
                    view! {
                        <InlineError
                            message=git.load_error.get().unwrap_or_default()
                            class="gp-line"
                            retry=Callback::new(move |_| git.reload.update(|n| *n += 1))
                        />
                    }
                        .into_any()
                }
                Some(g) if !g.repo => {
                    let why = if g.reason.is_empty() {
                        "这个文件夹没有用 Git 管理，提交和推送用不了".to_owned()
                    } else {
                        g.reason.clone()
                    };
                    view! { <div class="gp-line muted">{why}</div> }.into_any()
                }
                Some(g) => {
                    view! {
                        {op_bar(&g)}
                        {line(&g)}
                        {commits(&g)}
                        {pr_line(&g)}
                    }
                        .into_any()
                }
            }}
            {move || {
                git.out
                    .get()
                    .filter(|o| !o.2.is_empty())
                    .map(|(op, ok, text)| {
                        view! {
                            <details class="gp-out" open=!ok>
                                <summary>
                                    {format!(
                                        "{} · {}",
                                        if ok { "操作输出" } else { "操作失败" },
                                        op_label(&op),
                                    )}
                                </summary>
                                <pre>{text}</pre>
                            </details>
                        }
                    })
            }} {move || git.form.get().map(|f| view! { <GitForm f git form=git.form /> })}
        </div>
    }
}

fn op_label(op: &str) -> &str {
    match op {
        "commit" => "提交",
        "push" => "推送",
        "fetch" => "同步",
        "pr-create" => "创建 PR",
        "rename-branch" => "改分支名",
        "set-target" => "选要合到哪里",
        "continue" => "继续",
        "abort" => "放弃",
        other => op_text(other),
    }
}
