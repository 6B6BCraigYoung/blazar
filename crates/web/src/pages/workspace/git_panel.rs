use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::api::{self, GitStatus, PrDetail};
use crate::components::dialog::{self, Choice};
use crate::components::menu::menu_keydown;
use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::components::toast::toast;
use crate::fmt;

use super::files::Files;

mod forms;
mod presentation;
mod state;

use forms::GitForm;
use presentation::{code_mark, op_text, resolve_text};

#[derive(Clone, Copy)]
pub struct Git {
    pub status: RwSignal<Option<GitStatus>>,
    pub load_error: RwSignal<Option<String>>,
    pub busy: RwSignal<Option<&'static str>>,
    pub out: RwSignal<Option<(String, bool, String)>>,
    pub pr: RwSignal<Option<PrDetail>>,
    pub reload: RwSignal<u32>,
    pub changed: RwSignal<u32>,
    ws: StoredValue<String>,
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
    let menu_trigger = NodeRef::<html::Button>::new();
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
        chips.push(view! { <span class="gchip" class:warn={g.uncommitted > 0}>{if g.uncommitted > 0 { format!("{} 个未提交", g.uncommitted) } else { "无未提交改动".to_owned() }}</span> }.into_any());
        chips.push(if g.remote.is_empty() {
            view! { <span class="gchip">"未设置远端"</span> }.into_any()
        } else if g.upstream.is_empty() {
            view! { <span class="gchip warn">"未推送"</span> }.into_any()
        } else {
            let t = if g.up_ahead > 0 || g.up_behind > 0 { format!("远端 ↑{} ↓{}", g.up_ahead, g.up_behind) } else { "已同步".to_owned() };
            view! { <span class="gchip" class:bad={g.up_behind > 0} class:warn={g.up_ahead > 0 && g.up_behind == 0} title=format!("相对 {}", g.upstream)>{t}</span> }.into_any()
        });
        if g.running > 0 {
            chips.push(view! { <span class="gchip info">"智能体运行中"</span> }.into_any());
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
                    <button class="gp-target" node_ref=menu_trigger aria-haspopup="menu" aria-expanded=move || menu.get().is_some().to_string() title="这条分支最终要合到哪里。领先 / 落后、变基、合并和 PR 都以它为准" on:click=move |_| run("target")>
                        {target_label}{guess.then(|| view! { <i class="muted">" 自动"</i> })}" ▾"
                    </button>
                    {move || menu.get().map(|list| {
                        let cur = git.status.with_untracked(|s| s.as_ref().map(|s| s.target.clone()).unwrap_or_default());
                        let menu_root = NodeRef::<html::Div>::new();
                        view! {
                            <div class="menu" node_ref=menu_root role="menu" aria-label="目标分支" on:keydown=move |event| {
                                if let Some(root) = menu_root.get_untracked() { menu_keydown(&event, root.unchecked_ref(), Callback::new(move |_| { menu.set(None); if let Some(trigger) = menu_trigger.get_untracked() { let _ = trigger.focus(); } })); }
                            } on:mouseleave=move |_| menu.set(None)>
                                <button role="menuitem" title="main / master / origin 默认分支" on:click=move |_| set_target(String::new())>"自动选择"</button>
                                <div class="menu-sep"></div>
                                {list.into_iter().map(|b| {
                                    let label = if b == cur { format!("✓ {b}") } else { b.clone() };
                                    view! { <button role="menuitem" on:click=move |_| set_target(b.clone())>{label}</button> }
                                }).collect_view()}
                            </div>
                        }
                    })}
                </span>
                {chips}
                <span class="grow"></span>
                <button class="btn small" disabled=move || no_remote || any_busy() title=if no_remote { "未设置远端" } else { "" }
                    on:click=move |_| run("fetch")>{move || if busy("fetch") { "拉取中…" } else { "Fetch" }}</button>
                <button class="laybtn" aria-label="刷新 Git 状态" title="刷新" inner_html=super::ICON_REFRESH on:click=move |_| git.reload.update(|n| *n += 1)></button>
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
            "未设置远端"
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
            "未设置远端"
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
                    <button class="btn primary" disabled=move || pr_off || any_busy() title=pr_why on:click=move |_| form.set(Some(Form::Pr))>"创建 PR…"</button>
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
                        view! { <button class="btn small primary" on:click=move |_| draft.set(Some(resolve_text(&g2)))>"协助解决"</button> }
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
                let state_label = match state.as_str() { "OPEN" => "待合并", "MERGED" => "已合并", "CLOSED" => "已关闭", s => s }.to_owned();
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
            view! { <EmptyState title="没有未提交改动" class="muted small"/> }.into_any()
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
            view! { <EmptyState title=if tgt { "没有领先提交" } else { "选择目标分支查看提交" } class="muted small"/> }.into_any()
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
                None => view! { <LoadingState text="读取 Git 状态…"/> }.into_any(),
                Some(_) if git.load_error.with(Option::is_some) => view! { <InlineError message=git.load_error.get().unwrap_or_default() class="empty" retry=Callback::new(move |_| git.reload.update(|n| *n += 1))/> }.into_any(),
                Some(g) if !g.repo => view! { <EmptyState title=if g.reason.is_empty() { "当前目录不是 Git 仓库".to_owned() } else { g.reason.clone() }/> }.into_any(),
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
                    <summary>{format!("{} · {}", if ok { "操作输出" } else { "操作失败" }, op_text(&op))}</summary>
                    <pre>{text}</pre>
                </details>
            })}
            {move || form.get().map(|f| view! { <GitForm f git form/> })}
        </div>
    }
}
