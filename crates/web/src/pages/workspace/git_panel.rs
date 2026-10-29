//! 「更改」视图里和提交、推送、PR 有关的部分：右上角的主按钮、⋯ 菜单里的 Git 操作、
//! 差异栏上的分支下拉，以及冲突、提交列表这些按需出现在差异上方的条。看改了什么的列表就是差异本身，这里不再列一遍。

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, GitStatus, PrDetail};
use crate::components::dialog::{self, Choice};
use crate::components::status::InlineError;
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
    /// 提交列表在差异上方展开着没有；开关在分支下拉里。
    pub commits_open: RwSignal<bool>,
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
/// 同步、改分支名、要合到哪里在分支下拉里，这里不重复。
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

/// 差异栏上分支按钮的图标。
pub const ICON_BRANCH: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 5-12 3-12 7"/></svg>"#;

/// 分支按钮上的名字：没在分支上时写提交号。
pub fn branch_label(git: Git) -> String {
    git.status.with(|g| {
        g.as_ref().map_or_else(String::new, |g| {
            if g.detached {
                format!("{}（没在分支上）", g.head)
            } else {
                g.branch.clone()
            }
        })
    })
}

/// 分支按钮旁的小圆点：和远端分叉、PR 检查失败或要求修改时亮，落后于要合到的分支时提醒。
pub fn branch_tone(git: Git) -> Option<&'static str> {
    let pr = git.pr.with(|d| {
        d.as_ref().and_then(|d| {
            if d.checks.failed > 0 || d.mergeable == "CONFLICTING" {
                Some("failed")
            } else if d.review == "CHANGES_REQUESTED" {
                Some("attention")
            } else {
                None
            }
        })
    });
    git.status.with(|g| {
        let g = g.as_ref().filter(|g| g.repo)?;
        if !g.remote.is_empty() && !g.upstream.is_empty() && g.up_behind > 0 {
            Some("failed")
        } else if pr.is_some() {
            pr
        } else if g.has_target() && g.behind > 0 {
            Some("attention")
        } else {
            None
        }
    })
}

/// 差异栏上的分支下拉：在哪条分支、推没推、要合到哪里、提交列表、PR 状态、同步、改名。
#[component]
pub fn BranchItems(git: Git, close: Callback<()>) -> impl IntoView {
    let picking = RwSignal::new(false);
    let targets = RwSignal::new(None::<Vec<String>>);
    let pick_target = move || {
        if picking.get_untracked() {
            picking.set(false);
            return;
        }
        picking.set(true);
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
                    let _ = targets.try_set(Some(list));
                }
                Err(e) => toast(e.to_string()),
            }
        });
    };
    let set_target = move |b: String| {
        close.run(());
        picking.set(false);
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
        let tgt = g.has_target();
        let target = short_target(&g);
        let sync: Option<(Option<&'static str>, String)> = if g.remote.is_empty() {
            Some((None, "没有远端".to_owned()))
        } else if g.detached {
            None
        } else if g.upstream.is_empty() {
            Some((None, "还没推送过".to_owned()))
        } else if g.up_behind > 0 {
            Some((
                Some("failed"),
                if g.up_ahead > 0 {
                    "和远端各有对方没有的提交".to_owned()
                } else {
                    format!("远端有 {} 个本地没有的提交", g.up_behind)
                },
            ))
        } else if g.up_ahead > 0 {
            Some((None, format!("{} 个提交还没推送", g.up_ahead)))
        } else {
            Some((None, "已推送".to_owned()))
        };
        let note_row = |tone: Option<&'static str>, text: String| {
            view! {
                <span class="dv-note-row">
                    {tone
                        .map(|t| {
                            view! { <span class="dot" data-tone=t aria-hidden="true"></span> }
                        })} {text}
                </span>
            }
        };
        let info = view! {
            <div class="menu-note dv-branch-note">
                {sync.map(|(t, s)| note_row(t, s))}
                {(tgt && g.behind > 0)
                    .then(|| note_row(
                        Some("attention"),
                        format!("{target} 上有 {} 个新提交", g.behind),
                    ))}
                {(g.running > 0).then(|| note_row(Some("run"), "智能体运行中".to_owned()))}
            </div>
        };

        // 已经在要合到的那条分支上（比如就在 main 上），没有「合到哪里」可选。
        let target_pick = (!g.detached && !g.on_target()).then(|| {
            let target_label = if g.target.is_empty() {
                "选一个分支".to_owned()
            } else {
                target.clone()
            };
            let guess = g.target_guess && !g.target.is_empty();
            let cur = g.target.clone();
            view! {
                <button
                    type="button"
                    role="menuitem"
                    class="menu-sub"
                    aria-expanded=move || picking.get().to_string()
                    title="这条分支最终要合到哪里。「这条分支的全部改动」、合并和 PR 都以它为准"
                    on:click=move |_| pick_target()
                >
                    <span>{format!("合到 {target_label}")}</span>
                    {guess.then(|| view! { <small>"自动"</small> })}
                    <span class="chev" aria-hidden="true">
                        {move || if picking.get() { "▾" } else { "▸" }}
                    </span>
                </button>
                <Show when=move || {
                    picking.get()
                }>
                    {
                        let cur = cur.clone();
                        move || match targets.get() {
                            None => {
                                view! { <div class="menu-note">"读取分支…"</div> }.into_any()
                            }
                            Some(list) => {
                                let cur = cur.clone();
                                view! {
                                    <button
                                        type="button"
                                        role="menuitemradio"
                                        class="menu-adv"
                                        aria-checked=guess.to_string()
                                        title="main、master 或远端的默认分支"
                                        on:click=move |_| set_target(String::new())
                                    >
                                        <span>"自动选择"</span>
                                    </button>
                                    {list
                                        .into_iter()
                                        .map(|b| {
                                            let checked = !guess && b == cur;
                                            let label = b
                                                .strip_prefix("origin/")
                                                .map_or_else(|| b.clone(), |s| format!("{s}（远端）"));
                                            view! {
                                                <button
                                                    type="button"
                                                    role="menuitemradio"
                                                    class="menu-adv"
                                                    aria-checked=checked.to_string()
                                                    on:click=move |_| set_target(b.clone())
                                                >
                                                    <span>{label}</span>
                                                </button>
                                            }
                                        })
                                        .collect_view()}
                                }
                                    .into_any()
                            }
                        }
                    }
                </Show>
            }
        });

        let n_commits = g.commits.len();
        let commits = (n_commits > 0).then(|| {
            let label = if tgt {
                format!("{} 个提交", g.ahead)
            } else {
                format!(
                    "{n_commits}{} 个提交",
                    if n_commits >= 40 { "+" } else { "" }
                )
            };
            let title = if tgt {
                format!("这条分支有、{target} 没有的提交，列在差异上方")
            } else {
                "这条分支上的提交，列在差异上方".to_owned()
            };
            view! {
                <button
                    type="button"
                    role="menuitemcheckbox"
                    aria-checked=move || git.commits_open.get().to_string()
                    title=title
                    on:click=move |_| {
                        close.run(());
                        git.commits_open.update(|o| *o = !*o);
                    }
                >
                    <span>{label}</span>
                </button>
            }
        });

        let pr = g.pr.clone().map(|pr| {
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
                let mut notes: Vec<String> = Vec::new();
                let mut tone = None;
                if let Some(d) = &d {
                    if d.draft {
                        notes.push("草稿".into());
                    }
                    if d.checks.failed > 0 {
                        tone = Some("failed");
                        notes.push(format!("{} 项检查失败", d.checks.failed));
                    } else if d.checks.pending > 0 {
                        notes.push(format!("{} 项检查进行中", d.checks.pending));
                    } else if d.checks.pass > 0 {
                        notes.push(format!("{} 项检查通过", d.checks.pass));
                    }
                    match d.review.as_str() {
                        "APPROVED" => notes.push("已批准".into()),
                        "CHANGES_REQUESTED" => {
                            tone = tone.or(Some("attention"));
                            notes.push("要求修改".into());
                        }
                        _ => {}
                    }
                    if d.mergeable == "CONFLICTING" {
                        tone = Some("failed");
                        notes.push("和要合到的分支有冲突".into());
                    }
                }
                let title = d.as_ref().map(|d| d.title.clone()).unwrap_or_default();
                let number = pr.number.map(|n| n.to_string()).unwrap_or_default();
                view! {
                    <div class="menu-sep"></div>
                    <a
                        role="menuitem"
                        class="dv-pr"
                        href=pr.url.clone()
                        target="_blank"
                        rel="noopener"
                        title=title
                        data-s=state.to_lowercase()
                        on:click=move |_| close.run(())
                    >
                        <span class="dv-note-row">
                            {tone
                                .map(|t| {
                                    view! {
                                        <span class="dot" data-tone=t aria-hidden="true"></span>
                                    }
                                })} {format!("PR #{number} · {state_label}")}
                        </span>
                        {(!notes.is_empty()).then(|| view! { <small>{notes.join(" · ")}</small> })}
                    </a>
                    <button type="button" role="menuitem" on:click=move |_| git.run("pr-refresh")>
                        <span>
                            {move || {
                                if git.busy_with("pr-refresh") {
                                    "查询中…"
                                } else {
                                    "刷新 PR 状态"
                                }
                            }}
                        </span>
                    </button>
                }
            }
        });

        let mut tail: Vec<AnyView> = Vec::new();
        if c.fetch {
            tail.push(
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
            tail.push(
                item(
                    "改分支名…".into(),
                    "",
                    Callback::new(move |()| git.form.set(Some(Form::Rename))),
                )
                .into_any(),
            );
        }
        if !g.detached {
            let name = g.branch.clone();
            tail.push(
                item(
                    "复制分支名".into(),
                    "",
                    Callback::new(move |()| {
                        let name = name.clone();
                        spawn_local(async move {
                            let p = window().navigator().clipboard().write_text(&name);
                            if wasm_bindgen_futures::JsFuture::from(p).await.is_ok() {
                                toast("已复制分支名");
                            }
                        });
                    }),
                )
                .into_any(),
            );
        }
        let first = target_pick.is_some() || commits.is_some();
        Some(view! {
            {info}
            {first.then(|| view! { <div class="menu-sep"></div> })}
            {target_pick}
            {commits}
            {pr}
            {(!tail.is_empty()).then(|| view! { <div class="menu-sep"></div> })}
            {tail}
        })
    }
}

/// 差异栏下面按需出现的几条：读不到分支状态、不是 Git 仓库、进行到一半的合并或变基、
/// 展开的提交列表、操作输出。平时一条都没有，分支的其余信息都在差异栏的分支下拉里。
#[component]
pub fn GitLine(git: Git, draft: RwSignal<Option<String>>) -> impl IntoView {
    let any_busy = move || git.any_busy();
    let busy = move |op: &str| git.busy_with(op);
    let commits_open = git.commits_open;

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
            let n = g.commits.len();
            view! {
                <div class="gp-commits">
                    <div class="gp-commits-head">
                        <span>{format!("{n} 个提交")}</span>
                        <span class="grow"></span>
                        <button class="linkbtn" on:click=move |_| commits_open.set(false)>
                            "收起"
                        </button>
                    </div>
                    {rows}
                </div>
            }
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

    view! {
        <div class="gitline">
            {move || match git.status.get() {
                None => ().into_any(),
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
                        {commits(&g)}
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
