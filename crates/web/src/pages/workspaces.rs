//! 工作区列表：全部 / 运行中 / 等我审批 / 某个项目；以及工作区的「属性」对话框（冻结、提交、推送、销毁）。

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::{Value, json};

use crate::api::{self, WorkspaceView};
use crate::app_state::use_app;
use crate::chat_model;
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::fmt;

use super::workspace::activity_label;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    All,
    Running,
    Waiting,
    Project,
}

#[component]
pub fn WorkspacesPage() -> impl IntoView {
    view! { <List kind=Kind::All/> }
}
#[component]
pub fn RunningPage() -> impl IntoView {
    view! { <List kind=Kind::Running/> }
}
#[component]
pub fn WaitingPage() -> impl IntoView {
    view! { <List kind=Kind::Waiting/> }
}
#[component]
pub fn ProjectPage() -> impl IntoView {
    view! { <List kind=Kind::Project/> }
}

#[component]
fn List(kind: Kind) -> impl IntoView {
    let app = use_app();
    let params = use_params_map();
    let project = move || {
        params
            .read()
            .get("name")
            .map(|n| {
                js_sys::decode_uri_component(&n)
                    .map(String::from)
                    .unwrap_or(n)
            })
            .unwrap_or_default()
    };
    let q = RwSignal::new(String::new());
    let insp = RwSignal::new(None::<String>);
    let title = move || match kind {
        Kind::All => "全部工作区".to_owned(),
        Kind::Running => "运行中".to_owned(),
        Kind::Waiting => "等我审批".to_owned(),
        Kind::Project => project(),
    };
    let list = move || {
        let p = project();
        app.workspaces()
            .into_iter()
            .filter(|w| match kind {
                Kind::All => true,
                Kind::Running => w.activity == "running",
                Kind::Waiting => w.activity == "awaiting_approval",
                Kind::Project => w.project.as_deref() == Some(p.as_str()),
            })
            .collect::<Vec<_>>()
    };
    let rows = move || {
        let k = q.get().to_lowercase();
        list()
            .into_iter()
            .filter(|w| {
                k.is_empty()
                    || format!(
                        "{} {} {} {}",
                        w.name,
                        w.node,
                        w.project.clone().unwrap_or_default(),
                        w.path
                    )
                    .to_lowercase()
                    .contains(&k)
            })
            .collect::<Vec<_>>()
    };
    view! {
        <div class="page">
            <div class="page-head">
                <h1>{title}</h1>
                <span class="gchip">{move || list().len()}</span>
                <span class="grow"></span>
                <input class="page-filter" placeholder="筛选…" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
                <button class="btn primary" on:click=move |_| app.new_ws.set(true)>"新建"</button>
            </div>
            {(kind == Kind::Waiting).then(|| view! { <PendingApprovals/> })}
            {move || {
                let r = rows();
                if app.state.with(Option::is_none) {
                    return view! { <div class="empty">"加载中…"</div> }.into_any();
                }
                if r.is_empty() {
                    let msg = if list().is_empty() { "还没有工作区 —— 点右上角「新建」" } else { "无匹配" };
                    return view! { <div class="empty">{msg}</div> }.into_any();
                }
                view! { <div class="ws-list">{r.into_iter().map(|w| card(w, insp)).collect_view()}</div> }.into_any()
            }}
            {move || insp.get().map(|id| view! { <Inspector id on_close=move || insp.set(None)/> })}
        </div>
    }
}

pub fn diff_badge(w: &WorkspaceView) -> AnyView {
    match w.diff {
        None => view! { <span class="muted">"—"</span> }.into_any(),
        Some(d) if d.files == 0 => view! { <span class="muted">"无改动"</span> }.into_any(),
        Some(d) => view! { <span class="diffst" title=format!("{} 个文件", d.files)><i class="ok">{format!("+{}", d.added)}</i>" "<i class="bad">{format!("−{}", d.removed)}</i></span> }.into_any(),
    }
}

fn card(w: WorkspaceView, insp: RwSignal<Option<String>>) -> impl IntoView {
    let app = use_app();
    let navigate = use_navigate();
    let shown = app.shown_activity(&w);
    let unseen = app.unseen(&w);
    let id = w.id.clone();
    let id2 = w.id.clone();
    view! {
        <div class="ws-card" on:click=move |_| navigate(&format!("/w/{id}"), Default::default())>
            <div class="top">
                <b class:strong=unseen title=w.name.clone()>{w.name.clone()}</b>
                <span class="state-pill" data-act=shown.clone()>{activity_label(&shown).to_owned()}</span>
            </div>
            <div class="path" title=w.path.clone()>{format!("{}:{}", w.node, w.path)}</div>
            <div class="meta">
                {w.project.clone().map(|p| view! { <span title="项目">{p}</span> })}
                {diff_badge(&w)}
                <span class="muted">{w.last_active_at.as_deref().map(fmt::ago).unwrap_or_default()}</span>
            </div>
            <div class="act">
                <button class="btn small" on:click=move |e| { e.stop_propagation(); insp.set(Some(id2.clone())); }>"属性"</button>
                <button class="btn small primary">"打开"</button>
            </div>
        </div>
    }
}

/// 「等我审批」页：所有工作区里在等你裁决的操作。
#[component]
fn PendingApprovals() -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0u32);
    let list = LocalResource::new(move || {
        rev.track();
        app.state.track();
        api::get::<Vec<Value>>("/api/approvals")
    });
    let decide = move |id: String, allow: bool| {
        spawn_local(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/approvals/{id}"),
                &json!({ "allow": allow, "message": "" }),
            )
            .await
            {
                Ok(r) if r["delivered"].as_bool() == Some(true) => {
                    toast(if allow { "已允许" } else { "已拒绝" })
                }
                Ok(r) => toast(r["reason"].as_str().unwrap_or("没能送达").to_owned()),
                Err(e) => toast(e.to_string()),
            }
            rev.update(|n| *n += 1);
        });
    };
    view! {
        <div class="appr-all">
            {move || match list.get() {
                None => view! { <div class="muted small">"读取中…"</div> }.into_any(),
                Some(Err(e)) => view! { <div class="err-line">{e.to_string()}</div> }.into_any(),
                Some(Ok(l)) if l.is_empty() => view! { <div class="muted small">"没有等你裁决的操作"</div> }.into_any(),
                Some(Ok(l)) => l.into_iter().map(|a| {
                    let id = a["id"].as_str().unwrap_or("").to_owned();
                    let wid = a["workspace_id"].as_str().unwrap_or("").to_owned();
                    let w = app.workspaces().into_iter().find(|w| w.id == wid);
                    let root = w.as_ref().map(|w| w.path.clone()).unwrap_or_default();
                    let ask = chat_model::tool_name(a["request"]["tool_name"].as_str().unwrap_or("")) == "AskUserQuestion";
                    let title = if ask { "agent 有问题要问你".to_owned() } else { chat_model::approval_title(&a["request"], &root, None) };
                    let what = if ask { String::new() } else { chat_model::approval_what(&a["request"], &root) };
                    let (i1, i2) = (id.clone(), id.clone());
                    view! {
                        <div class="cc-appr static">
                            <div class="muted small">
                                <a href=format!("/w/{wid}")>{w.as_ref().map_or(wid.clone(), |w| w.name.clone())}</a>
                                {format!(" · {} · {}", w.as_ref().map(|w| w.node.clone()).unwrap_or_default(), fmt::ago(a["created_at"].as_str().unwrap_or("")))}
                            </div>
                            <div class="ahd"><span class="ah">{title}</span></div>
                            {(!what.is_empty()).then(|| view! { <pre class="acmd">{what}</pre> })}
                            <div class="row-actions">
                                {if ask {
                                    view! { <a class="btn small primary" href=format!("/w/{wid}")>"去回答"</a> }.into_any()
                                } else {
                                    view! {
                                        <button class="btn small primary" on:click=move |_| decide(i1.clone(), true)>"允许"</button>
                                        <button class="btn small" on:click=move |_| decide(i2.clone(), false)>"拒绝"</button>
                                        <a class="linkbtn" href=format!("/w/{wid}")>"打开工作区"</a>
                                    }.into_any()
                                }}
                            </div>
                        </div>
                    }
                }).collect_view().into_any(),
            }}
        </div>
    }
}

/// 工作区属性和操作。
#[component]
pub fn Inspector(id: String, on_close: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let app = use_app();
    let navigate = use_navigate();
    let rev = RwSignal::new(0u32);
    let wid = id.clone();
    let detail = LocalResource::new(move || {
        rev.track();
        let id = wid.clone();
        async move { api::get::<Value>(&format!("/api/workspaces/{id}/detail")).await }
    });
    let busy = RwSignal::new(None::<&'static str>);
    let act = StoredValue::new_local(move |d: Value, a: &'static str| {
        let navigate = navigate.clone();
        spawn_local(async move {
            let name = d["name"].as_str().unwrap_or("").to_owned();
            let isolated = d["isolated"].as_bool().unwrap_or(false);
            let id = d["id"].as_str().unwrap_or("").to_owned();
            if a == "destroy" {
                let msg = if isolated {
                    format!("销毁工作区「{name}」？会删除远端 worktree、分支与私有目录，不可撤销。")
                } else {
                    format!("从 Blazar 移除「{name}」？只删记录，不会动机器上的目录。")
                };
                if dialog::ask(
                    if isolated { "销毁" } else { "移除" },
                    &msg,
                    vec![
                        Choice::plain("取消"),
                        Choice::danger(if isolated { "销毁" } else { "移除" }),
                    ],
                )
                .await
                    != Some(1)
                {
                    return;
                }
            }
            let mut body = json!({});
            if a == "commit" {
                let msg = window()
                    .prompt_with_message_and_default("提交信息", &format!("blazar: {name}"))
                    .ok()
                    .flatten();
                let Some(m) = msg.filter(|m| !m.trim().is_empty()) else {
                    return;
                };
                body = json!({ "message": m });
            }
            busy.set(Some(a));
            let r = api::send::<Value>("POST", &format!("/api/workspaces/{id}/{a}"), &body).await;
            let _ = busy.try_set(None);
            match r {
                Err(e) => toast(format!("操作失败：{e}")),
                Ok(r) if a == "push" => {
                    if let Some(e) = r["error"].as_str() {
                        toast(format!("推送失败：{e}"));
                        return;
                    }
                    let link = r["web"]["new_pr"].as_str().map(str::to_owned);
                    let body = format!(
                        "分支：{}\n远端：{}{}",
                        r["branch"].as_str().unwrap_or(""),
                        r["remote"].as_str().unwrap_or(""),
                        r["committed"]
                            .as_str()
                            .map(|c| format!(
                                "\n自动提交：{}",
                                c.chars().take(10).collect::<String>()
                            ))
                            .unwrap_or_default()
                    );
                    let mut ch = vec![Choice::plain("关闭")];
                    if link.is_some() {
                        ch.push(Choice::plain("开 PR / MR"));
                    }
                    if dialog::ask("已推送", &body, ch).await == Some(1)
                        && let Some(l) = link
                    {
                        let _ = window().open_with_url_and_target(&l, "_blank");
                    }
                }
                Ok(r) if a == "commit" => toast(if r["changed"].as_bool() == Some(true) {
                    format!(
                        "已提交 {}",
                        r["commit"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(8)
                            .collect::<String>()
                    )
                } else {
                    "没有改动可提交".to_owned()
                }),
                Ok(r) => {
                    let mut notes = Vec::new();
                    if let Some(n) = r["stopped_sessions"].as_u64().filter(|n| *n > 0) {
                        notes.push(format!("停掉了 {n} 个正在跑的会话"));
                    }
                    if r["worktree_was_missing"].as_bool() == Some(true) {
                        notes.push(
                            "工作目录冻结前就已经不在了，没提交的改动（如果有）已丢失".to_owned(),
                        );
                    }
                    if r["already_present"].as_bool() == Some(true) {
                        notes.push("工作目录本来就在，保持原样".to_owned());
                    }
                    if let Some(m) = r["moved_aside"].as_str() {
                        notes.push(format!("原目录已损坏，挪到了 {m}"));
                    }
                    if let Some(b) = r["branch_kept"].as_str() {
                        notes.push(format!("分支没删掉：{b}"));
                    }
                    let head = match a {
                        "pause" => "已冻结",
                        "resume" => "已解冻",
                        _ => {
                            if isolated {
                                "已销毁"
                            } else {
                                "已移除"
                            }
                        }
                    };
                    toast(if notes.is_empty() {
                        head.to_owned()
                    } else {
                        format!("{head}。{}", notes.join("；"))
                    });
                    app.load_state();
                    if a == "destroy" {
                        on_close();
                        navigate("/", Default::default());
                    } else {
                        rev.update(|n| *n += 1);
                    }
                }
            }
        });
    });
    let act = move |d: Value, a: &'static str| act.with_value(|f| f(d, a));
    view! {
        <div class="dlg-mask" on:click=move |_| on_close()>
            <div class="dlg wide" on:click=|e| e.stop_propagation()>
                {move || match detail.get() {
                    None => view! { <div class="empty">"读取中…"</div> }.into_any(),
                    Some(Err(e)) => view! { <div class="err-line">{e.to_string()}</div> }.into_any(),
                    Some(Ok(d)) => {
                        let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
                        let isolated = d["isolated"].as_bool().unwrap_or(false);
                        let paused = s("status") == "paused";
                        let kv = |k: &'static str, v: String, mono: bool| view! { <div class="kv"><span class="k">{k}</span><span class="v" class:mono=mono>{v}</span></div> };
                        let sessions: Vec<Value> = d["sessions"].as_array().cloned().unwrap_or_default();
                        let (d1, d2, d3, d4, d5) = (d.clone(), d.clone(), d.clone(), d.clone(), d.clone());
                        view! {
                            <h3>{s("name")}</h3>
                            <div class="insp">
                                <h4>"属性"</h4>
                                {kv("机器", s("node"), false)}
                                {kv("路径", s("path"), true)}
                                {kv("项目", d["project"].as_str().unwrap_or("—").to_owned(), false)}
                                {isolated.then(|| kv("分支", s("branch"), true))}
                                {isolated.then(|| kv("基线", s("base_commit").chars().take(10).collect(), true))}
                                {kv("来源", if s("origin") == "task" { "批量派发".into() } else { "手动创建".into() }, false)}
                                {kv("状态", if paused { "已冻结".into() } else { "活跃".into() }, false)}
                                {kv("创建", fmt::ago(&s("created_at")), false)}
                            </div>
                            <div class="insp">
                                <h4>"操作"</h4>
                                <div class="row-actions">
                                    {isolated.then(|| if paused {
                                        view! { <button class="btn small" disabled=move || busy.get().is_some() on:click=move |_| act(d1.clone(), "resume")>"解冻"</button> }.into_any()
                                    } else {
                                        view! { <button class="btn small" title="提交→删 worktree→保留分支" disabled=move || busy.get().is_some() on:click=move |_| act(d1.clone(), "pause")>"冻结"</button> }.into_any()
                                    })}
                                    {isolated.then(|| view! { <button class="btn small" disabled=move || busy.get().is_some() on:click=move |_| act(d2.clone(), "commit")>"提交"</button> })}
                                    {(isolated && !paused).then(|| view! {
                                        <button class="btn small primary" title="先提交没提交的改动，再推到 origin" disabled=move || busy.get().is_some() on:click=move |_| act(d3.clone(), "push")>
                                            {move || if busy.get() == Some("push") { "推送中…" } else { "推送分支" }}
                                        </button>
                                    })}
                                    <button class="btn small danger" disabled=move || busy.get().is_some() on:click=move |_| act(d4.clone(), "destroy")>{if isolated { "销毁" } else { "移除" }}</button>
                                </div>
                                {(!isolated).then(|| view! { <div class="muted small">"非隔离工作区：移除只删记录，不动你的目录"</div> })}
                            </div>
                            <div class="insp">
                                <h4>{format!("会话 {}", if sessions.is_empty() { String::new() } else { sessions.len().to_string() })}</h4>
                                {if sessions.is_empty() {
                                    view! { <div class="muted small">"还没有会话"</div> }.into_any()
                                } else {
                                    sessions.into_iter().take(8).map(|x| view! {
                                        <div class="kv"><span class="k">{x["runtime"].as_str().unwrap_or("").to_owned()}</span>
                                            <span class="v muted small">{format!("{} · {} 条事件 · {}", x["status"].as_str().unwrap_or(""), x["events"].as_i64().unwrap_or(0), fmt::ago(x["created_at"].as_str().unwrap_or("")))}</span></div>
                                    }).collect_view().into_any()
                                }}
                            </div>
                            <div class="dlg-foot">
                                <a class="btn" href=format!("/w/{}", d5["id"].as_str().unwrap_or(""))>"打开"</a>
                                <button class="btn" on:click=move |_| on_close()>"关闭"</button>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
