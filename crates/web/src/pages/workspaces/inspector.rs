use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use crate::api;
use crate::app_state::{AppData, use_app};
use crate::components::dialog::{self, Choice};
use crate::components::modal::Modal;
use crate::components::prefs::{PrefGroup, PrefRow};
use crate::components::status::{InlineError, LoadingState, Status};
use crate::components::toast::toast;
use crate::fmt;
use crate::vocab::{self, Tone};

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

async fn confirm_remove(label: &str, isolated: bool, kids: usize, busy: bool) -> bool {
    let mut msg = if isolated {
        format!("销毁工作区「{label}」？会删除远端 worktree、分支与私有目录，不可撤销。")
    } else {
        format!("从 Blazar 移除「{label}」？只删记录，不会动机器上的目录。")
    };
    if busy {
        msg.push_str("正在跑的会话会被停掉。");
    }
    if kids > 0 {
        msg.push_str(&format!("它的 {kids} 个子工作区会变成独立的工作区。"));
    }
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

pub fn retry_workspace(app: AppData, id: String, session: String) {
    spawn_local(async move {
        let ok = dialog::ask(
            "重试这一轮？",
            "· 文件会还原到这条消息之前（当前状态会先备份）\n· 用同一条消息重新开始",
            vec![Choice::plain("取消"), Choice::danger("重试")],
        )
        .await;
        if ok != Some(1) {
            return;
        }
        match api::send::<Value>(
            "POST",
            &format!("/api/workspaces/{id}/retry"),
            &json!({ "session_id": session, "options": {} }),
        )
        .await
        {
            Ok(r) if r["admitted"] == json!(false) => toast(
                r["reason"]
                    .as_str()
                    .unwrap_or("工作区正忙着别的操作")
                    .to_owned(),
            ),
            Ok(_) => {
                toast("已重新开始");
                app.load_state();
            }
            Err(e) => toast(format!("没能重试：{e}")),
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
        let kids = app.children_of(&id).len();
        let busy = matches!(
            d["activity"].as_str().unwrap_or(""),
            "running" | "starting" | "awaiting_approval"
        );
        let path = format!("/api/workspaces/{id}/destroy");
        if !confirm_remove(&label, isolated, kids, busy).await {
            return;
        }
        match api::send::<Value>("POST", &path, &json!({})).await {
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

fn on_control(e: &ev::KeyboardEvent) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|t| {
            t.closest("button, a, input, textarea, select")
                .ok()
                .flatten()
                .is_some()
        })
}

#[component]
pub fn Inspector(id: String, on_close: impl Fn() + Copy + Send + Sync + 'static) -> impl IntoView {
    let app = use_app();
    let navigate = use_navigate();
    let rev = RwSignal::new(0u32);
    let wid = StoredValue::new(id.clone());
    let detail = LocalResource::new(move || {
        rev.track();
        let id = wid.get_value();
        async move { api::get::<Value>(&format!("/api/workspaces/{id}/detail")).await }
    });
    let busy = RwSignal::new(false);
    let close = move || {
        if !busy.get_untracked() {
            on_close();
        }
    };
    let href = format!("/w/{id}");
    let open = {
        let href = href.clone();
        move || {
            on_close();
            navigate(&href, Default::default());
        }
    };
    let toggle_frozen = move |paused: bool| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let id = wid.get_value();
            let a = if paused { "resume" } else { "pause" };
            let r =
                api::send::<Value>("POST", &format!("/api/workspaces/{id}/{a}"), &json!({})).await;
            let _ = busy.try_set(false);
            match r {
                Ok(r) => {
                    toast(outcome_text(if paused { "已解冻" } else { "已冻结" }, &r));
                    app.load_state();
                    rev.update(|n| *n += 1);
                }
                Err(e) => toast(format!("操作失败：{e}")),
            }
        });
    };
    let state = move |paused: bool| -> (String, Tone) {
        if paused {
            return ("已冻结".to_owned(), Tone::Other);
        }
        app.workspace(&wid.get_value())
            .map(|w| crate::home_rows::status(&app.shown_activity(&w), w.step.as_ref()))
            .unwrap_or_else(|| crate::home_rows::status("", None))
    };
    view! {
        <Modal label="工作区简介" class="dlg wide ws-info" on_close=Callback::new(move |_| close())>
            <div
                class="ws-info-body"
                on:keydown=move |e: ev::KeyboardEvent| {
                    if e.key() == "Enter" && !e.is_composing() && !on_control(&e) {
                        e.prevent_default();
                        open();
                    }
                }
            >
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
                                    "完成"
                                </button>
                            </div>
                        }
                            .into_any()
                    }
                    Some(Ok(d)) => {
                        let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
                        let isolated = d["isolated"].as_bool().unwrap_or(false);
                        let paused = s("status") == "paused";
                        let (state_label, tone) = state(paused);
                        let (name, node, path) = (s("name"), s("node"), s("path"));
                        let project = d["project"].as_str().unwrap_or("—").to_owned();
                        let branch = s("branch");
                        let base: String = s("base_commit").chars().take(10).collect();
                        let origin = if s("origin") == "task" { "批量派发" } else { "手动创建" };
                        let created = fmt::ago(&s("created_at"));
                        let sessions: Vec<Value> = d["sessions"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        let count = sessions.len();
                        let href = href.clone();
                        view! {
                            <h3>{fmt::ws_title(&name, &node)}</h3>
                            <PrefGroup title="工作区">
                                <PrefRow label="机器">
                                    <span class="pref-text">{fmt::machine(&node)}</span>
                                </PrefRow>
                                <PrefRow label="路径">
                                    <span class="pref-text">
                                        <code>{path}</code>
                                    </span>
                                </PrefRow>
                                <PrefRow label="项目">
                                    <span class="pref-text">{project}</span>
                                </PrefRow>
                                {isolated
                                    .then(|| {
                                        view! {
                                            <PrefRow label="分支">
                                                <span class="pref-text">
                                                    <code>{branch}</code>
                                                </span>
                                            </PrefRow>
                                            <PrefRow label="基线">
                                                <span class="pref-text">
                                                    <code>{base}</code>
                                                </span>
                                            </PrefRow>
                                        }
                                    })}
                                <PrefRow label="来源">
                                    <span class="pref-text">{origin}</span>
                                </PrefRow>
                                <PrefRow label="状态">
                                    <Status tone label=state_label />
                                </PrefRow>
                                {isolated
                                    .then(|| {
                                        view! {
                                            <PrefRow
                                                label="工作目录"
                                                detail=if paused {
                                                    "已冻结：worktree 已删除，分支保留。"
                                                } else {
                                                    "冻结会先提交，再删除 worktree，保留分支。"
                                                }
                                            >
                                                <button
                                                    type="button"
                                                    class="btn"
                                                    disabled=move || busy.get()
                                                    on:click=move |_| toggle_frozen(paused)
                                                >
                                                    {if paused { "解冻" } else { "冻结" }}
                                                </button>
                                            </PrefRow>
                                        }
                                    })}
                                <PrefRow label="创建">
                                    <span class="pref-text">{created}</span>
                                </PrefRow>
                            </PrefGroup>
                            <PrefGroup title=if count == 0 {
                                "会话".to_owned()
                            } else {
                                format!("会话 {count}")
                            }>
                                {if sessions.is_empty() {
                                    view! { <PrefRow label="还没有会话" /> }.into_any()
                                } else {
                                    sessions
                                        .into_iter()
                                        .take(8)
                                        .map(|x| {
                                            let runtime = vocab::term(x["runtime"].as_str().unwrap_or(""))
                                                .to_owned();
                                            let title = x["title"]
                                                .as_str()
                                                .filter(|t| !t.trim().is_empty())
                                                .unwrap_or("新对话")
                                                .to_owned();
                                            let (status, _) = vocab::state(x["status"].as_str().unwrap_or(""));
                                            view! {
                                                <PrefRow
                                                    label=format!("{runtime} · {title}")
                                                    detail=format!(
                                                        "{status} · {} 条事件 · {}",
                                                        x["events"].as_i64().unwrap_or(0),
                                                        fmt::ago(x["created_at"].as_str().unwrap_or("")),
                                                    )
                                                />
                                            }
                                        })
                                        .collect_view()
                                        .into_any()
                                }}
                            </PrefGroup>
                            <div class="dlg-foot">
                                <button
                                    type="button"
                                    class="btn"
                                    disabled=move || busy.get()
                                    on:click=move |_| close()
                                >
                                    "完成"
                                </button>
                                <a
                                    class="btn primary"
                                    href=href
                                    data-modal-initial-focus=""
                                    on:click=move |_| on_close()
                                >
                                    "打开"
                                    <kbd>"↩"</kbd>
                                </a>
                            </div>
                        }
                            .into_any()
                    }
                }}
            </div>
        </Modal>
    }
}
