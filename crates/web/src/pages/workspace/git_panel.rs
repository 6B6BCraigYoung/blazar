use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, GitOpResult, GitStatus, PrDetail};
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::fmt;
use crate::realtime::use_bus;

use super::files::Files;

#[derive(Clone, Copy)]
pub struct Git {
    pub status: RwSignal<Option<GitStatus>>,
    pub busy: RwSignal<Option<&'static str>>,
    pub out: RwSignal<Option<(String, bool, String)>>,
    pub pr: RwSignal<Option<PrDetail>>,
    pub reload: RwSignal<u32>,
    pub changed: RwSignal<u32>,
    ws: StoredValue<String>,
}

impl Git {
    pub fn new(ws: &str) -> Self {
        Self {
            status: RwSignal::new(None),
            busy: RwSignal::new(None),
            out: RwSignal::new(None),
            pr: RwSignal::new(None),
            reload: RwSignal::new(0),
            changed: RwSignal::new(0),
            ws: StoredValue::new(ws.to_owned()),
        }
    }

    pub fn keep_loaded(self) {
        let bus = use_bus();
        Effect::new(move |_| {
            bus.workspaces.track();
            self.reload.track();
            let ws = self.ws.get_value();
            spawn_local(async move {
                let s = match api::get::<GitStatus>(&format!("/api/workspaces/{ws}/git")).await {
                    Ok(s) => s,
                    Err(e) => GitStatus {
                        reason: e.to_string(),
                        ..GitStatus::default()
                    },
                };
                let _ = self.status.try_set(Some(s));
            });
        });
    }

    async fn op(self, op: &'static str, body: serde_json::Value) -> Option<GitOpResult> {
        if self.busy.get_untracked().is_some() {
            return None;
        }
        self.busy.set(Some(op));
        let ws = self.ws.get_value();
        let r = api::send::<GitOpResult>("POST", &format!("/api/workspaces/{ws}/git/{op}"), &body)
            .await;
        let _ = self.busy.try_set(None);
        if matches!(op, "rebase" | "continue" | "abort" | "commit" | "merge") {
            let _ = self.changed.try_update(|n| *n = n.wrapping_add(1));
        }
        match r {
            Ok(r) => {
                if let Some(s) = r.status.clone() {
                    let _ = self.status.try_set(Some(s));
                }
                let _ = self
                    .out
                    .try_set(Some((op.to_owned(), r.ok, r.output.clone())));
                Some(r)
            }
            Err(e) => {
                let _ = self
                    .out
                    .try_set(Some((op.to_owned(), false, e.to_string())));
                toast(e.to_string());
                None
            }
        }
    }
}

fn resolve_text(g: &GitStatus) -> String {
    let op = g.op.as_deref().unwrap_or("");
    let what = match op {
        "rebase" => format!("变基（rebase）到 {}", g.target),
        "merge" => "合并（merge）".to_owned(),
        o => o.to_owned(),
    };
    let cont = match op {
        "rebase" => "git rebase --continue",
        "merge" => "git commit --no-edit",
        _ => "git cherry-pick --continue",
    };
    let files: Vec<String> = g.conflicts.iter().map(|f| format!("- {f}")).collect();
    format!(
        "这个工作区正在{what}，进行到一半，下面这些文件有冲突：\n{}\n\n请逐个解决：先弄清两边各自想做什么，把两边的意图都保留下来，不要整段只选一边。解决完 git add，再执行 {cont}（设置 GIT_EDITOR=true 免得卡在编辑器上）；后面的提交又冲突就接着解决，直到整个过程结束。\n不要 abort，不要动和冲突无关的代码。完成后用几句话说明每处冲突是怎么取舍的。",
        files.join("\n")
    )
}

fn op_text(op: &str) -> &str {
    match op {
        "rebase" => "变基",
        "merge" => "合并",
        other => other,
    }
}

fn code_mark(code: &str) -> (&'static str, &'static str) {
    match code {
        "??" => ("U", "added"),
        "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD" => ("!", "deleted"),
        c if c.starts_with('A') => ("A", "added"),
        c if c.starts_with('D') => ("D", "deleted"),
        c if c.starts_with('R') => ("R", "renamed"),
        _ => ("M", "modified"),
    }
}

#[derive(Clone, PartialEq)]
enum Form {
    Commit,
    Pr,
    Rename,
}

#[component]
pub fn GitView(git: Git, files: Files, draft: RwSignal<Option<String>>) -> impl IntoView {
    let form = RwSignal::new(None::<Form>);
    let menu = RwSignal::new(None::<Vec<String>>);
    let busy = move |op: &str| git.busy.with(|b| *b == Some(op));
    let any_busy = move || git.busy.with(Option::is_some);

    let run = move |op: &'static str| {
        spawn_local(async move {
            let Some(g) = git.status.get_untracked() else {
                return;
            };
            match op {
                "fetch" => {
                    if git
                        .op("fetch", serde_json::json!({}))
                        .await
                        .is_some_and(|r| r.ok)
                    {
                        toast("已拉取远端的最新状态");
                    }
                }
                "continue" => match git.op("continue", serde_json::json!({})).await {
                    Some(r)
                        if r.ok
                            && git
                                .status
                                .with_untracked(|s| s.as_ref().is_some_and(|s| s.op.is_none())) =>
                    {
                        toast("已完成")
                    }
                    Some(r) if !r.ok => toast("还不能继续，看下面的输出"),
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
                        && git
                            .op("abort", serde_json::json!({}))
                            .await
                            .is_some_and(|r| r.ok)
                    {
                        toast("已放弃，分支回到之前的状态");
                    }
                }
                "rebase" => match git.op("rebase", serde_json::json!({})).await {
                    Some(r) if r.ok => toast(format!("已变基到 {}", g.target)),
                    Some(_) => {
                        let n = git.status.with_untracked(|s| {
                            s.as_ref()
                                .filter(|s| s.op.as_deref() == Some("rebase"))
                                .map(|s| s.conflicts.len())
                        });
                        toast(n.map_or_else(
                            || "变基没成功，看下面的输出".to_owned(),
                            |n| format!("变基遇到冲突：{n} 个文件要解决"),
                        ));
                    }
                    None => {}
                },
                "merge" => {
                    let how = if g.behind > 0 {
                        format!(
                            "{} 上有 {} 个这条分支没有的提交，会产生一个合并提交（有冲突则不合并，得先变基）。",
                            g.target, g.behind
                        )
                    } else {
                        "可以直接快进，不会产生合并提交。".to_owned()
                    };
                    let body = format!(
                        "把 {} 合并进 {}？\n\n{how}\n挂在这个工作区上、处于「待审阅」的任务会自动标成完成。",
                        g.branch, g.target
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
                    match git.op("merge", serde_json::json!({})).await {
                        Some(r) if r.ok => toast(if r.tasks_done > 0 {
                            format!("已合并进 {}，{} 个任务已完成", g.target, r.tasks_done)
                        } else {
                            format!("已合并进 {}", g.target)
                        }),
                        Some(_) => toast("没有合并，看下面的输出"),
                        None => {}
                    }
                }
                "push" => match git.op("push", serde_json::json!({})).await {
                    Some(r) if r.ok => toast("已推送"),
                    Some(r) if r.needs_force => {
                        let n = git
                            .status
                            .with_untracked(|s| s.as_ref().map_or(0, |s| s.up_behind));
                        let body = format!(
                            "远端的 {} 上有本地没有的提交 —— 通常是因为刚变基过，本地历史被改写了。\n\n\
                             强制推送会用本地的历史覆盖远端：远端那 {} 个提交会从分支上消失；已经基于旧历史拉过代码的人需要重新对齐。\n\n\
                             这里用的是 --force-with-lease：如果在你上次拉取之后别人又推过新提交，这次强推会被拒绝，不会悄悄盖掉别人的活。",
                            g.branch,
                            if n > 0 {
                                n.to_string()
                            } else {
                                "几".to_owned()
                            }
                        );
                        if dialog::ask(
                            "强制推送",
                            &body,
                            vec![Choice::plain("取消"), Choice::danger("强制推送")],
                        )
                        .await
                            == Some(1)
                        {
                            let f = git.op("push", serde_json::json!({ "force": true })).await;
                            toast(if f.is_some_and(|r| r.ok) {
                                "已强制推送"
                            } else {
                                "强制推送也被拒绝了，看下面的输出"
                            });
                        }
                    }
                    Some(_) => toast("推送失败，看下面的输出"),
                    None => {}
                },
                "target" => {
                    let ws = git.ws.get_value();
                    match api::get::<serde_json::Value>(&format!(
                        "/api/workspaces/{ws}/git/branches"
                    ))
                    .await
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
                            menu.set(Some(list));
                        }
                        Err(e) => toast(e.to_string()),
                    }
                }
                "pr-refresh" => {
                    if git.busy.get_untracked().is_some() {
                        return;
                    }
                    git.busy.set(Some("pr-refresh"));
                    let ws = git.ws.get_value();
                    let r =
                        api::get::<serde_json::Value>(&format!("/api/workspaces/{ws}/pr")).await;
                    let _ = git.busy.try_set(None);
                    match r {
                        Ok(v) if v["pr"].is_object() => {
                            let _ = git.pr.try_set(serde_json::from_value(v["pr"].clone()).ok());
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
                    "目标分支改回自动判断".to_owned()
                } else {
                    format!("目标分支：{b}")
                });
            }
        });
    };

    let head = move |g: &GitStatus| {
        let tgt = g.has_target();
        let mut chips: Vec<AnyView> = Vec::new();
        if tgt {
            chips.push(view! { <span class="gchip" title=format!("这条分支有、{} 没有的提交", g.target)>{format!("↑ {} 领先", g.ahead)}</span> }.into_any());
            chips.push(view! { <span class="gchip" class:warn={g.behind > 0} title=format!("{} 有、这条分支没有的提交", g.target)>{format!("↓ {} 落后", g.behind)}</span> }.into_any());
        }
        chips.push(view! { <span class="gchip" class:warn={g.uncommitted > 0}>{if g.uncommitted > 0 { format!("{} 个未提交", g.uncommitted) } else { "工作区干净".to_owned() }}</span> }.into_any());
        chips.push(if g.remote.is_empty() {
            view! { <span class="gchip">"没有 origin 远端"</span> }.into_any()
        } else if g.upstream.is_empty() {
            view! { <span class="gchip warn">"还没推送过"</span> }.into_any()
        } else {
            let t = if g.up_ahead > 0 || g.up_behind > 0 { format!("远端 ↑{} ↓{}", g.up_ahead, g.up_behind) } else { "已与远端同步".to_owned() };
            view! { <span class="gchip" class:bad={g.up_behind > 0} class:warn={g.up_ahead > 0 && g.up_behind == 0} title=format!("相对 {}", g.upstream)>{t}</span> }.into_any()
        });
        if g.running > 0 {
            chips.push(view! { <span class="gchip info">"agent 运行中"</span> }.into_any());
        }
        let branch = if g.detached {
            format!("游离 HEAD · {}", g.head)
        } else {
            g.branch.clone()
        };
        let can_rename = !g.detached && g.op.is_none();
        let target_label = if g.target.is_empty() {
            "选择目标分支".to_owned()
        } else {
            g.target.clone()
        };
        let guess = g.target_guess && !g.target.is_empty();
        let no_remote = g.remote.is_empty();
        view! {
            <div class="gp-head">
                <span class="gp-br" title="当前分支">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 5-12 3-12 7"/></svg>
                    <b>{branch}</b>
                    {can_rename.then(|| view! { <button class="linkbtn" disabled=any_busy on:click=move |_| form.set(Some(Form::Rename))>"改名"</button> })}
                </span>
                <span class="muted">"→"</span>
                <span class="more">
                    <button class="gp-target" title="这条分支最终要合到哪里。领先 / 落后、变基、合并和 PR 都以它为准" on:click=move |_| run("target")>
                        {target_label}{guess.then(|| view! { <i class="muted">" 自动"</i> })}" ▾"
                    </button>
                    {move || menu.get().map(|list| {
                        let cur = git.status.with_untracked(|s| s.as_ref().map(|s| s.target.clone()).unwrap_or_default());
                        view! {
                            <div class="menu" on:mouseleave=move |_| menu.set(None)>
                                <button on:click=move |_| set_target(String::new())>"自动判断（main / master / origin 默认分支）"</button>
                                <div class="menu-sep"></div>
                                {list.into_iter().map(|b| {
                                    let label = if b == cur { format!("✓ {b}") } else { b.clone() };
                                    view! { <button on:click=move |_| set_target(b.clone())>{label}</button> }
                                }).collect_view()}
                            </div>
                        }
                    })}
                </span>
                {chips}
                <span class="grow"></span>
                <button class="btn small" disabled=move || no_remote || any_busy() title=if no_remote { "没有 origin 远端" } else { "" }
                    on:click=move |_| run("fetch")>{move || if busy("fetch") { "拉取中…" } else { "Fetch" }}</button>
                <button class="laybtn" title="刷新" inner_html=super::ICON_REFRESH on:click=move |_| git.reload.update(|n| *n += 1)></button>
            </div>
        }
    };

    let actions = move |g: &GitStatus| {
        let tgt = g.has_target();
        let in_op = g.op.is_some();
        let has_up = !g.upstream.is_empty();
        let target = if g.target.is_empty() {
            "…".to_owned()
        } else {
            g.target.clone()
        };
        let commit_why = if in_op {
            "先把进行到一半的操作收尾"
        } else if g.uncommitted == 0 {
            "没有改动可提交"
        } else {
            ""
        };
        let rebase_why = if !tgt {
            "先选一个目标分支".to_owned()
        } else if in_op || g.detached {
            String::new()
        } else if g.behind == 0 {
            format!("没有落后 {}，不用变基", g.target)
        } else {
            String::new()
        };
        let rebase_off = !tgt || in_op || g.detached || g.behind == 0;
        let merge_off = !tgt || in_op || g.detached || g.ahead == 0 || !g.target_local;
        let merge_why = if !tgt {
            "先选一个目标分支"
        } else if !g.target_local {
            "目标是远端分支：推送后走 PR"
        } else if g.ahead == 0 {
            "没有可合并的提交"
        } else {
            ""
        };
        let push_off = g.remote.is_empty()
            || g.detached
            || in_op
            || (has_up && g.up_ahead == 0 && g.up_behind == 0);
        let push_why = if g.remote.is_empty() {
            "没有 origin 远端"
        } else if push_off {
            "远端已经是最新的"
        } else {
            ""
        };
        let push_label = if g.up_ahead > 0 && has_up {
            format!("推送 ↑{}", g.up_ahead)
        } else {
            "推送".to_owned()
        };
        let pr_off = g.remote.is_empty() || !tgt || g.detached || in_op || g.ahead == 0;
        let pr_why = if g.remote.is_empty() {
            "没有 origin 远端"
        } else if g.ahead == 0 {
            "没有领先目标分支的提交"
        } else {
            ""
        };
        let has_pr = g.pr.is_some();
        let commit_off = g.uncommitted == 0 || in_op;
        let t2 = target.clone();
        view! {
            <div class="gp-acts">
                <button class="btn" disabled=move || commit_off || any_busy() title=commit_why on:click=move |_| form.set(Some(Form::Commit))>"提交…"</button>
                <button class="btn" disabled=move || rebase_off || any_busy() title=rebase_why on:click=move |_| run("rebase")>
                    {move || if busy("rebase") { "变基中…".to_owned() } else { format!("变基到 {target}") }}
                </button>
                <button class="btn" disabled=move || merge_off || any_busy() title=merge_why on:click=move |_| run("merge")>
                    {move || if busy("merge") { "合并中…".to_owned() } else { format!("合并进 {t2}") }}
                </button>
                <button class="btn" disabled=move || push_off || any_busy() title=push_why on:click=move |_| run("push")>
                    {move || if busy("push") { "推送中…".to_owned() } else { push_label.clone() }}
                </button>
                {(!has_pr).then(|| view! {
                    <button class="btn primary" disabled=move || pr_off || any_busy() title=pr_why on:click=move |_| form.set(Some(Form::Pr))>"开 PR…"</button>
                })}
            </div>
        }
    };

    let op_bar = move |g: &GitStatus| {
        g.op.clone().map(|op| {
            let what = op_text(&op).to_owned();
            let n = g.conflicts.len();
            view! {
                <div class="gp-op">
                    <b>{format!("{what}进行到一半")}</b>
                    <span>{if n > 0 { format!("{n} 个文件有冲突，解决后点「继续」") } else { "冲突都解决了，可以继续".to_owned() }}</span>
                    <span class="grow"></span>
                    {(n > 0).then(|| {
                        let g2 = g.clone();
                        view! { <button class="btn small primary" on:click=move |_| draft.set(Some(resolve_text(&g2)))>"让 agent 解决"</button> }
                    })}
                    <button class="btn small" disabled=any_busy on:click=move |_| run("continue")>{move || if busy("continue") { "继续中…" } else { "继续" }}</button>
                    <button class="btn small danger" disabled=any_busy on:click=move |_| run("abort")>{format!("放弃{what}")}</button>
                </div>
            }
        })
    };

    let pr_bar = move |g: &GitStatus| {
        g.pr.clone().map(|pr| {
            move || {
                let d = git.pr.get();
                let state = d.as_ref().map(|d| d.state.clone()).or_else(|| pr.state.clone()).unwrap_or_default();
                let state_label = match state.as_str() { "OPEN" => "开着", "MERGED" => "已合并", "CLOSED" => "已关闭", s => s }.to_owned();
                let extra = d.map(|d| {
                    let checks = if d.checks.failed > 0 {
                        Some(("bad", format!("{} 项检查失败", d.checks.failed)))
                    } else if d.checks.pending > 0 {
                        Some(("warn", format!("{} 项检查进行中", d.checks.pending)))
                    } else if d.checks.pass > 0 {
                        Some(("ok", format!("{} 项检查通过", d.checks.pass)))
                    } else {
                        None
                    };
                    let review = match d.review.as_str() { "APPROVED" => Some(("ok", "已批准")), "CHANGES_REQUESTED" => Some(("bad", "要求修改")), _ => None };
                    view! {
                        {d.draft.then(|| view! { <span class="gchip">"草稿"</span> })}
                        {checks.map(|(c, t)| view! { <span class=format!("gchip {c}")>{t}</span> })}
                        {review.map(|(c, t)| view! { <span class=format!("gchip {c}")>{t}</span> })}
                        {(d.mergeable == "CONFLICTING").then(|| view! { <span class="gchip bad">"与目标分支冲突"</span> })}
                        <span class="gp-prt">{d.title}</span>
                    }
                });
                view! {
                    <div class="gp-pr" data-s=state.to_lowercase()>
                        <b>{format!("PR #{}", pr.number.map(|n| n.to_string()).unwrap_or_default())}</b>
                        <span class="gchip">{state_label}</span>
                        {extra}
                        <span class="grow"></span>
                        <a class="linkbtn" href=pr.url.clone() target="_blank" rel="noopener">"在网页上打开"</a>
                        <button class="linkbtn" on:click=move |_| run("pr-refresh")>{move || if busy("pr-refresh") { "查询中…" } else { "刷新状态" }}</button>
                    </div>
                }
            }
        })
    };

    let cols = move |g: &GitStatus| {
        let tgt = g.has_target();
        let files_list = if g.files.is_empty() {
            view! { <div class="muted small">"没有"</div> }.into_any()
        } else {
            g.files.iter().map(|f| {
                let (mk, cls) = code_mark(&f.code);
                let p = f.path.clone();
                view! {
                    <button class="gp-file" title=f.path.clone() on:click=move |_| files.open(p.clone(), 0)>
                        <b class=format!("mk {cls}")>{mk}</b><span>{f.path.clone()}</span>
                    </button>
                }
            }).collect_view().into_any()
        };
        let commits = if g.commits.is_empty() {
            view! { <div class="muted small">{if tgt { "还没有" } else { "选了目标分支才知道哪些提交是这条分支的" }}</div> }.into_any()
        } else {
            g.commits.iter().map(|c| {
                let when = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(c.at as f64 * 1000.0)).to_iso_string();
                view! {
                    <div class="gp-commit">
                        <code>{c.sha.clone()}</code>
                        <span class="sj" title=c.subject.clone()>{c.subject.clone()}</span>
                        <span class="muted small">{format!("{} · {}", c.author, fmt::ago(&String::from(when)))}</span>
                    </div>
                }
            }).collect_view().into_any()
        };
        let n_commits = format!(
            "{}{}",
            g.commits.len(),
            if g.commits.len() >= 40 { "+" } else { "" }
        );
        view! {
            <div class="gp-cols">
                <div class="gp-col"><h5>"未提交的改动 "<span class="muted">{g.uncommitted}</span></h5>{files_list}</div>
                <div class="gp-col"><h5>{if tgt { format!("领先 {} 的提交 ", g.target) } else { "提交 ".to_owned() }}<span class="muted">{n_commits}</span></h5>{commits}</div>
            </div>
        }
    };

    view! {
        <div class="gitview">
            {move || match git.status.get() {
                None => view! { <div class="empty">"加载中…"</div> }.into_any(),
                Some(g) if !g.repo => view! { <div class="empty">{if g.reason.is_empty() { "这个目录不是 git 仓库".to_owned() } else { g.reason.clone() }}</div> }.into_any(),
                Some(g) => view! {
                    {head(&g)}
                    {op_bar(&g)}
                    {actions(&g)}
                    {pr_bar(&g)}
                    {cols(&g)}
                }.into_any(),
            }}
            {move || git.out.get().filter(|o| !o.2.is_empty()).map(|(op, ok, text)| view! {
                <details class="gp-out" open=!ok>
                    <summary>{format!("{} · {op}", if ok { "上一次操作的输出" } else { "上一次操作没成功" })}</summary>
                    <pre>{text}</pre>
                </details>
            })}
            {move || form.get().map(|f| view! { <GitForm f git form/> })}
        </div>
    }
}

#[component]
fn GitForm(f: Form, git: Git, form: RwSignal<Option<Form>>) -> impl IntoView {
    let g = git.status.get_untracked().unwrap_or_default();
    let title = RwSignal::new(match f {
        Form::Pr if g.commits.len() == 1 => g.commits[0].subject.clone(),
        Form::Rename => g.branch.clone(),
        _ => String::new(),
    });
    let body = RwSignal::new(String::new());
    let draft = RwSignal::new(false);
    let drafting = RwSignal::new(false);
    let close = move || form.set(None);

    let ai = {
        let f = f.clone();
        move |_| {
            let kind = if f == Form::Pr { "pr" } else { "commit" };
            drafting.set(true);
            let ws = git.ws.get_value();
            spawn_local(async move {
                match api::send::<serde_json::Value>(
                    "POST",
                    &format!("/api/workspaces/{ws}/git/describe"),
                    &serde_json::json!({ "kind": kind }),
                )
                .await
                {
                    Ok(v) if kind == "pr" => {
                        title.set(v["title"].as_str().unwrap_or_default().to_owned());
                        body.set(v["body"].as_str().unwrap_or_default().to_owned());
                    }
                    Ok(v) => body.set(v["text"].as_str().unwrap_or_default().to_owned()),
                    Err(e) => toast(e.to_string()),
                }
                let _ = drafting.try_set(false);
            });
        }
    };

    let submit = {
        let f = f.clone();
        let g = g.clone();
        move || {
            let f = f.clone();
            let g = g.clone();
            match f {
                Form::Commit => {
                    let message = body.get_untracked().trim().to_owned();
                    if message.is_empty() {
                        toast("写一句提交信息");
                        return;
                    }
                    close();
                    spawn_local(async move {
                        match git
                            .op("commit", serde_json::json!({ "message": message }))
                            .await
                        {
                            Some(r) if r.ok => toast(if r.changed {
                                format!("已提交 {}", r.commit.unwrap_or_default())
                            } else {
                                "没有改动可提交".to_owned()
                            }),
                            Some(_) => toast("提交失败，看下面的输出"),
                            None => {}
                        }
                    });
                }
                Form::Rename => {
                    let name = title.get_untracked().trim().to_owned();
                    close();
                    if name.is_empty() || name == g.branch {
                        return;
                    }
                    spawn_local(async move {
                        let r = git
                            .op("rename-branch", serde_json::json!({ "name": name }))
                            .await;
                        toast(if r.is_some_and(|r| r.ok) {
                            "已改名（远端的旧分支不会自动删除）"
                        } else {
                            "改名失败"
                        });
                    });
                }
                Form::Pr => {
                    let t = title.get_untracked().trim().to_owned();
                    if !crate::git_policy::valid_pr_title(g.gh, &t) {
                        toast("写一个标题");
                        return;
                    }
                    let b = body.get_untracked();
                    let dr = draft.get_untracked();
                    close();
                    spawn_local(async move {
                        if !g.gh {
                            let r = git.op("push", serde_json::json!({})).await;
                            let url = git.status.with_untracked(|s| {
                                s.as_ref()
                                    .and_then(|s| s.web.as_ref())
                                    .and_then(|w| w.new_pr.clone())
                            });
                            match (r.is_some_and(|r| r.ok), url) {
                                (true, Some(u)) => {
                                    let _ = window().open_with_url_and_target(&u, "_blank");
                                }
                                (true, None) => toast("认不出托管平台，没法给网页链接"),
                                _ => toast("推送失败，看下面的输出"),
                            }
                            return;
                        }
                        match git
                            .op(
                                "pr-create",
                                serde_json::json!({ "title": t, "body": b, "draft": dr }),
                            )
                            .await
                        {
                            Some(r) if r.ok && r.url.is_some() => {
                                toast(if r.existing {
                                    "这条分支已经有 PR 了"
                                } else {
                                    "PR 已创建"
                                });
                                if let Some(u) = r.url {
                                    let _ = window().open_with_url_and_target(&u, "_blank");
                                }
                            }
                            Some(_) => toast("PR 没开成，看下面的输出"),
                            None => {}
                        }
                    });
                }
            }
        }
    };
    let submit2 = submit.clone();

    let base = g.target.trim_start_matches("origin/").to_owned();
    let (head_title, ok_label) = match f {
        Form::Commit => ("提交改动", "提交"),
        Form::Pr => (
            if g.gh { "开 Pull Request" } else { "开 PR" },
            if g.gh {
                "创建 PR"
            } else {
                "推送并打开网页"
            },
        ),
        Form::Rename => ("新的分支名", "改名"),
    };
    let fields = match f {
        Form::Commit => view! {
            <div class="dlg-body small">{format!("{} 个文件的改动会全部提交到 {}（含未跟踪的新文件）", g.uncommitted, if g.branch.is_empty() { &g.head } else { &g.branch })}</div>
            <label class="field">"提交信息"
                <textarea rows="5" placeholder="改了什么、为什么" prop:value=move || body.get() on:input=move |e| body.set(event_target_value(&e))
                    on:keydown=move |e| if e.key() == "Enter" && (e.meta_key() || e.ctrl_key()) { e.prevent_default(); submit2(); }></textarea>
            </label>
        }.into_any(),
        Form::Pr if !g.gh => view! {
            <div class="dlg-body">"工作区所在的机器上没有装 GitHub CLI（gh），没法直接创建 PR。可以先把分支推上去，再到网页上开。"</div>
        }.into_any(),
        Form::Pr => view! {
            <div class="dlg-body small">
                {format!("{} → {} · {} 个提交", g.branch, base, g.ahead)}
                {(g.uncommitted > 0).then(|| view! { <span class="warn-tx">{format!(" · 还有 {} 个没提交的改动不会进 PR", g.uncommitted)}</span> })}
            </div>
            <label class="field">"标题"<input prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e)) placeholder="这个 PR 做了什么"/></label>
            <label class="field">"描述（Markdown）"<textarea rows="9" prop:value=move || body.get() on:input=move |e| body.set(event_target_value(&e)) placeholder="改了什么、为什么、怎么验证"></textarea></label>
            <label class="chk"><input type="checkbox" prop:checked=move || draft.get() on:change=move |_| draft.update(|d| *d = !*d)/>"作为草稿创建"</label>
            <div class="muted small">"会先把分支推到 origin，再用那台机器上 gh 的登录态创建 PR。"</div>
        }.into_any(),
        Form::Rename => view! {
            <label class="field"><input prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e))/></label>
        }.into_any(),
    };
    let show_ai = match f {
        Form::Commit => true,
        Form::Pr => g.gh,
        Form::Rename => false,
    };
    view! {
        <div class="dlg-mask" on:click=move |_| close()>
            <div class="dlg wide" role="dialog" on:click=|e| e.stop_propagation()>
                <h3>{head_title}</h3>
                {fields}
                <div class="dlg-foot">
                    {show_ai.then(|| view! {
                        <button class="btn" disabled=move || drafting.get() title="让本机的 Claude（Haiku）看一眼改动，替你起草" on:click=ai.clone()>
                            {move || if drafting.get() { "起草中…" } else { "AI 起草" }}
                        </button>
                    })}
                    <span class="grow"></span>
                    <button class="btn" on:click=move |_| close()>"取消"</button>
                    <button class="btn primary" on:click=move |_| submit()>{ok_label}</button>
                </div>
            </div>
        </div>
    }
}
