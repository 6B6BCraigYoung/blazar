use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};

use crate::api;
use crate::app_state::{AppData, use_app};
use crate::components::dialog::{self, Choice};
use crate::components::modal::Modal;
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::fmt;

fn removed_head(isolated: bool) -> &'static str {
    if isolated { "已销毁" } else { "已移除" }
}

fn outcome_text(head: &str, r: &Value) -> String {
    let mut notes = Vec::new();
    if let Some(n) = r["stopped_sessions"].as_u64().filter(|n| *n > 0) {
        notes.push(format!("停掉了 {n} 个正在跑的会话"));
    }
    if r["worktree_was_missing"].as_bool() == Some(true) {
        notes.push("工作目录冻结前就已经不在了，没提交的改动（如果有）已丢失".to_owned());
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
    if notes.is_empty() {
        head.to_owned()
    } else {
        format!("{head}。{}", notes.join("；"))
    }
}

async fn confirm_remove(label: &str, isolated: bool, kids: usize) -> bool {
    let orphans = if kids > 0 {
        format!("它的 {kids} 个子工作区会变成独立的工作区。")
    } else {
        String::new()
    };
    let msg = if isolated {
        format!("销毁工作区「{label}」？会删除远端 worktree、分支与私有目录，不可撤销。{orphans}")
    } else {
        format!("从 Blazar 移除「{label}」？只删记录，不会动机器上的目录。{orphans}")
    };
    let verb = if isolated { "销毁" } else { "移除" };
    dialog::ask(
        verb,
        &msg,
        vec![Choice::plain("取消"), Choice::danger(verb)],
    )
    .await
        == Some(1)
}

pub fn rename_workspace(app: AppData, id: String) {
    spawn_local(async move {
        let Some(w) = app.workspace(&id) else {
            return;
        };
        let current = fmt::ws_title(&w.name, &w.node);
        let Some(name) = dialog::prompt_with("改名", "名称", &current, "改名").await else {
            return;
        };
        if name == current {
            return;
        }
        match api::send::<Value>(
            "PATCH",
            &format!("/api/workspaces/{id}"),
            &json!({ "name": name }),
        )
        .await
        {
            Ok(_) => {
                toast("已改名");
                app.load_state();
            }
            Err(e) => toast(format!("改名失败：{e}")),
        }
    });
}

pub fn remove_workspace(app: AppData, id: String) {
    spawn_local(async move {
        let d = match api::get::<Value>(&format!("/api/workspaces/{id}/detail")).await {
            Ok(d) => d,
            Err(e) => {
                toast(format!("读取详情失败：{e}"));
                return;
            }
        };
        let isolated = d["isolated"].as_bool().unwrap_or(false);
        let label = fmt::ws_label(
            d["name"].as_str().unwrap_or(""),
            d["node"].as_str().unwrap_or(""),
        );
        if !confirm_remove(&label, isolated, app.children_of(&id).len()).await {
            return;
        }
        match api::send::<Value>("POST", &format!("/api/workspaces/{id}/destroy"), &json!({})).await
        {
            Ok(r) => {
                toast(outcome_text(removed_head(isolated), &r));
                app.load_state();
            }
            Err(e) => toast(format!("操作失败：{e}")),
        }
    });
}

#[component]
pub fn InspectorHost() -> impl IntoView {
    let app = use_app();
    view! {
        {move || {
            app.inspect_ws
                .get()
                .map(|id| view! { <Inspector id on_close=move || app.inspect_ws.set(None) /> })
        }}
    }
}

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
    let close = move || {
        if busy.get_untracked().is_none() {
            on_close();
        }
    };
    let act = StoredValue::new_local(move |d: Value, a: &'static str| {
        if busy.get_untracked().is_some() {
            return;
        }
        busy.set(Some(a));
        let navigate = navigate.clone();
        spawn_local(async move {
            let name = d["name"].as_str().unwrap_or("").to_owned();
            let isolated = d["isolated"].as_bool().unwrap_or(false);
            let id = d["id"].as_str().unwrap_or("").to_owned();
            if a == "destroy"
                && !confirm_remove(
                    &fmt::ws_label(&name, d["node"].as_str().unwrap_or("")),
                    isolated,
                    app.children_of(&id).len(),
                )
                .await
            {
                let _ = busy.try_set(None);
                return;
            }
            let mut body = json!({});
            if a == "commit" {
                let msg =
                    dialog::prompt_with("提交", "提交信息", &format!("blazar: {name}"), "提交")
                        .await;
                let Some(m) = msg else {
                    let _ = busy.try_set(None);
                    return;
                };
                body = json!({ "message": m });
            }
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
                    let head = match a {
                        "pause" => "已冻结",
                        "resume" => "已解冻",
                        _ => removed_head(isolated),
                    };
                    toast(outcome_text(head, &r));
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
        <Modal label="工作区详情" class="dlg wide" on_close=Callback::new(move |_| close())>
            {move || match detail.get() {
                None => view! { <LoadingState text="读取详情…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=e.to_string()
                            retry=Callback::new(move |_| rev.update(|n| *n += 1))
                        />
                        <div class="dlg-foot">
                            <button type="button" class="btn" on:click=move |_| close()>
                                "关闭"
                            </button>
                        </div>
                    }
                        .into_any()
                }
                Some(Ok(d)) => {
                    let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
                    let isolated = d["isolated"].as_bool().unwrap_or(false);
                    let paused = s("status") == "paused";
                    let kv = |k: &'static str, v: String, mono: bool| {
                        view! {
                            <div class="kv">
                                <span class="k">{k}</span>
                                <span class="v" class:mono=mono>
                                    {v}
                                </span>
                            </div>
                        }
                    };
                    let sessions: Vec<Value> = d["sessions"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let (d1, d2, d3, d4, d5) = (
                        d.clone(),
                        d.clone(),
                        d.clone(),
                        d.clone(),
                        d.clone(),
                    );
                    view! {
                        <h3>{s("name")}</h3>
                        <div class="insp">
                            <h4>"工作区信息"</h4>
                            {kv("机器", s("node"), false)}
                            {kv("路径", s("path"), true)}
                            {kv("项目", d["project"].as_str().unwrap_or("—").to_owned(), false)}
                            {isolated.then(|| kv("分支", s("branch"), true))}
                            {isolated
                                .then(|| kv(
                                    "基线",
                                    s("base_commit").chars().take(10).collect(),
                                    true,
                                ))}
                            {kv(
                                "来源",
                                if s("origin") == "task" {
                                    "批量派发".into()
                                } else {
                                    "手动创建".into()
                                },
                                false,
                            )}
                            {kv(
                                "状态",
                                if paused { "已冻结".into() } else { "活跃".into() },
                                false,
                            )}
                            {kv("创建", fmt::ago(&s("created_at")), false)}
                        </div>
                        <div class="insp">
                            <h4>"操作"</h4>
                            <div class="row-actions">
                                {isolated
                                    .then(|| {
                                        if paused {
                                            view! {
                                                <button
                                                    class="btn small"
                                                    disabled=move || busy.get().is_some()
                                                    on:click=move |_| act(d1.clone(), "resume")
                                                >
                                                    "解冻"
                                                </button>
                                            }
                                                .into_any()
                                        } else {
                                            view! {
                                                <button
                                                    class="btn small"
                                                    title="提交→删 worktree→保留分支"
                                                    disabled=move || busy.get().is_some()
                                                    on:click=move |_| act(d1.clone(), "pause")
                                                >
                                                    "冻结"
                                                </button>
                                            }
                                                .into_any()
                                        }
                                    })}
                                {isolated
                                    .then(|| {
                                        view! {
                                            <button
                                                class="btn small"
                                                disabled=move || busy.get().is_some()
                                                on:click=move |_| act(d2.clone(), "commit")
                                            >
                                                "提交"
                                            </button>
                                        }
                                    })}
                                {(isolated && !paused)
                                    .then(|| {
                                        view! {
                                            <button
                                                class="btn small primary"
                                                title="先提交没提交的改动，再推到 origin"
                                                disabled=move || busy.get().is_some()
                                                on:click=move |_| act(d3.clone(), "push")
                                            >
                                                {move || {
                                                    if busy.get() == Some("push") {
                                                        "推送中…"
                                                    } else {
                                                        "推送分支"
                                                    }
                                                }}
                                            </button>
                                        }
                                    })}
                                <button
                                    class="btn small danger"
                                    disabled=move || busy.get().is_some()
                                    on:click=move |_| act(d4.clone(), "destroy")
                                >
                                    {if isolated { "销毁" } else { "移除" }}
                                </button>
                            </div>
                            {(!isolated)
                                .then(|| {
                                    view! {
                                        <div class="muted small">
                                            "非隔离工作区：移除只删记录，不动你的目录"
                                        </div>
                                    }
                                })}
                        </div>
                        <div class="insp">
                            <h4>
                                {format!(
                                    "会话 {}",
                                    if sessions.is_empty() {
                                        String::new()
                                    } else {
                                        sessions.len().to_string()
                                    },
                                )}
                            </h4>
                            {if sessions.is_empty() {
                                view! { <div class="muted small">"还没有会话"</div> }
                                    .into_any()
                            } else {
                                sessions
                                    .into_iter()
                                    .take(8)
                                    .map(|x| {
                                        view! {
                                            <div class="kv">
                                                <span class="k">
                                                    {x["runtime"].as_str().unwrap_or("").to_owned()}
                                                </span>
                                                <span class="v muted small">
                                                    {format!(
                                                        "{} · {} 条事件 · {}",
                                                        x["status"].as_str().unwrap_or(""),
                                                        x["events"].as_i64().unwrap_or(0),
                                                        fmt::ago(x["created_at"].as_str().unwrap_or("")),
                                                    )}
                                                </span>
                                            </div>
                                        }
                                    })
                                    .collect_view()
                                    .into_any()
                            }}
                        </div>
                        <div class="dlg-foot">
                            <a class="btn" href=format!("/w/{}", d5["id"].as_str().unwrap_or(""))>
                                "打开"
                            </a>
                            <button
                                class="btn"
                                disabled=move || busy.get().is_some()
                                on:click=move |_| close()
                            >
                                "关闭"
                            </button>
                        </div>
                    }
                        .into_any()
                }
            }}
        </Modal>
    }
}
