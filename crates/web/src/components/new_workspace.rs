use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api;
use crate::app_state::use_app;
use crate::components::toast::toast;
use crate::fmt;

use super::menu::menu_keydown;
use super::modal::Modal;
use super::status::{EmptyState, InlineError, LoadingState};
use leptos::html;
use wasm_bindgen::JsCast;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
struct Listing {
    path: String,
    parent: Option<String>,
    is_repo: bool,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
struct Entry {
    name: String,
    path: String,
    is_repo: bool,
    children: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
struct Branch {
    name: String,
    author: String,
    last_commit_at: String,
    subject: String,
    remote_only: bool,
    current: bool,
}

#[component]
pub fn NewWorkspace() -> impl IntoView {
    let app = use_app();
    view! {
        <Show when=move || app.new_ws.get()>
            <Dialog/>
        </Show>
    }
}

#[component]
fn Dialog() -> impl IntoView {
    let app = use_app();
    let navigate = use_navigate();
    let preset = app
        .new_ws_node
        .get_untracked()
        .unwrap_or_else(|| "local".to_owned());
    let nodes: Vec<String> = {
        let mut v = vec!["local".to_owned()];
        v.extend(app.state.with_untracked(|s| {
            s.as_ref()
                .map(|s| {
                    s.nodes
                        .iter()
                        .filter(|n| n.is_usable())
                        .map(|n| n.name.clone())
                        .filter(|n| n != "local")
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        }));
        if !v.contains(&preset) {
            v.push(preset.clone());
        }
        v
    };
    let node = RwSignal::new(preset);
    on_cleanup(move || app.new_ws_node.set(None));
    let path = RwSignal::new(String::new());
    let typed = RwSignal::new(String::new());
    let listing = RwSignal::new(None::<Result<Listing, String>>);
    let iso = RwSignal::new(false);
    let branches = RwSignal::new(None::<Result<Vec<Branch>, String>>);
    let pick = RwSignal::new(String::new());
    let bq = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let project = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let close = move || {
        if !busy.get_untracked() {
            app.new_ws.set(false);
        }
    };

    let bseq = StoredValue::new(0u32);
    let load_branches = move || {
        branches.set(None);
        let (n, p) = (node.get_untracked(), path.get_untracked());
        bseq.update_value(|s| *s += 1);
        let mine = bseq.get_value();
        spawn_local(async move {
            let r = api::get::<Vec<Branch>>(&format!(
                "/api/nodes/{}/branches?repo={}",
                api::enc(&n),
                api::enc(&p)
            ))
            .await
            .map_err(|e| e.to_string());
            if bseq.try_get_value() != Some(mine) {
                return;
            }
            pick.set(String::new());
            let _ = branches.try_set(Some(r));
        });
    };
    let seq = StoredValue::new(0u32);
    let browse = move || {
        if busy.get_untracked() {
            return;
        }
        bseq.update_value(|n| *n = n.wrapping_add(1));
        branches.set(None);
        pick.set(String::new());
        bq.set(String::new());
        listing.set(None);
        let (n, p) = (node.get_untracked(), path.get_untracked());
        seq.update_value(|s| *s += 1);
        let mine = seq.get_value();
        spawn_local(async move {
            let r = api::get::<Listing>(&format!(
                "/api/nodes/{}/browse?path={}",
                api::enc(&n),
                api::enc(&p)
            ))
            .await
            .map_err(|e| e.to_string());
            if seq.try_get_value() != Some(mine) {
                return;
            }
            if let Ok(l) = &r {
                path.set(l.path.clone());
                typed.set(l.path.clone());
                if !l.is_repo {
                    iso.set(false);
                }
                if iso.get_untracked() {
                    load_branches();
                }
            }
            let _ = listing.try_set(Some(r));
        });
    };
    browse();
    let is_repo = move || listing.with(|l| matches!(l, Some(Ok(l)) if l.is_repo));

    let create = move || {
        if busy.get_untracked() {
            return;
        }
        if !matches!(listing.get_untracked(), Some(Ok(_))) {
            toast("请先等待目录读取完成");
            return;
        }
        let p = path.get_untracked();
        if p.is_empty() {
            toast("请先选一个目录");
            return;
        }
        if !crate::files_js::confirm_navigation() {
            return;
        }
        busy.set(true);
        let n = node.get_untracked();
        let nm = Some(name.get_untracked().trim().to_owned()).filter(|x| !x.is_empty());
        let pj = Some(project.get_untracked().trim().to_owned()).filter(|x| !x.is_empty());
        let isolated = iso.get_untracked();
        let from = Some(pick.get_untracked()).filter(|x| !x.is_empty());
        let navigate = navigate.clone();
        spawn_local(async move {
            let r = if isolated {
                api::send::<Value>("POST", "/api/workspaces/isolated", &json!({ "node": n, "repo": p, "from_branch": from, "name": nm, "project": pj })).await
            } else {
                api::send::<Value>(
                    "POST",
                    "/api/workspaces",
                    &json!({ "node": n, "path": p, "name": nm, "project": pj }),
                )
                .await
            };
            let _ = busy.try_set(false);
            match r {
                Ok(v) => {
                    app.new_ws.set(false);
                    app.load_state();
                    if let Some(b) = v["branch"].as_str() {
                        toast(format!("已在分支 {b} 上建好隔离工作区"));
                    }
                    if let Some(id) = v["id"].as_str() {
                        navigate(&format!("/w/{id}"), Default::default());
                    }
                }
                Err(e) => toast(format!("创建失败：{e}")),
            }
        });
    };
    let create2 = create.clone();
    let directories = NodeRef::<html::Div>::new();
    let branch_list = NodeRef::<html::Div>::new();

    view! {
        <Modal label="新建工作区" class="dlg wide" on_close=Callback::new(move |_| close())>
                <h3>"新建工作区"</h3>
                <label class="field">"机器"
                    <select prop:value=move ||node.get() disabled=move || busy.get() on:change=move |e| { node.set(event_target_value(&e)); path.set(String::new()); typed.set(String::new()); browse(); }>
                        {nodes.into_iter().map(|n| { let selected = n.clone(); view! { <option value=n.clone() selected=move ||node.get()==selected>{n.clone()}</option> } }).collect_view()}
                    </select>
                </label>
                <div class="field">"目录"
                    <div class="browse-bar">
                        <button type="button" class="btn small" aria-label="上一级目录" title="上一级" disabled=move || listing.with(|l| !matches!(l, Some(Ok(l)) if l.parent.is_some()))
                            on:click=move |_| { if let Some(Some(Ok(l))) = listing.try_get_untracked() && let Some(p) = l.parent { path.set(p); browse(); } }>"↑"</button>
                        <input class="mono" aria-label="目录路径" placeholder="~" prop:value=move || typed.get() on:input=move |e| typed.set(event_target_value(&e))
                            on:keydown=move |e| if e.key() == "Enter" && !e.is_composing() { path.set(typed.get_untracked().trim().to_owned()); browse(); }/>
                        <button type="button" class="btn small" on:click=move |_| { path.set(typed.get_untracked().trim().to_owned()); browse(); }>"前往"</button>
                    </div>
                    <div class="browser" node_ref=directories role="group" aria-label="子目录" data-keyboard-list="" on:keydown=move |event| { if let Some(root) = directories.get_untracked() { menu_keydown(&event, root.unchecked_ref(), Callback::new(move |_| close())); } }>
                        {move || match listing.get() {
                            None => view! { <LoadingState text="读取中…"/> }.into_any(),
                            Some(Err(e)) => view! { <InlineError message=e class="empty" retry=Callback::new(move |_| browse())/> }.into_any(),
                            Some(Ok(l)) if l.entries.is_empty() => view! { <EmptyState title="这个目录下没有子目录"/> }.into_any(),
                            Some(Ok(l)) => l.entries.into_iter().map(|e| {
                                let p = e.path.clone();
                                view! {
                                    <button type="button" class="brow" class:repo=e.is_repo title=e.path.clone() on:click=move |_| { path.set(p.clone()); browse(); }>
                                        <span class="mk" title=if e.is_repo { "Git 仓库" } else { "目录" }>{if e.is_repo { "◆" } else { "▸" }}</span>
                                        <span class="nm">{e.name.clone()}</span>
                                        {e.children.map(|c| view! { <span class="muted small">{c}</span> })}
                                    </button>
                                }
                            }).collect_view().into_any(),
                        }}
                    </div>
                </div>
                <Show when=is_repo>
                    <label class="chk block" title="启用后会从 origin 获取最新分支，并在独立目录创建工作区。">
                        <input type="checkbox" prop:checked=move || iso.get() on:change=move |_| { iso.update(|i| *i = !*i); if iso.get_untracked() && branches.with_untracked(Option::is_none) { load_branches(); } }/>
                        <span><b>"独立分支"</b><span class="muted small">" 在独立目录中工作，保留当前工作区。"</span></span>
                    </label>
                    <Show when=move || iso.get()>
                        <div class="field" title="按最近提交排序；读取时会从 origin 获取最新分支。">"起始分支"
                            <input aria-label="筛选分支" placeholder="筛选分支…" prop:value=move || bq.get() on:input=move |e| bq.set(event_target_value(&e))/>
                            <div class="browser short" node_ref=branch_list role="group" aria-label="可选分支" data-keyboard-list="" on:keydown=move |event| { if let Some(root) = branch_list.get_untracked() { menu_keydown(&event, root.unchecked_ref(), Callback::new(move |_| close())); } }>
                                <button type="button" class="brow br" aria-pressed=move || pick.get().is_empty().to_string() data-sel=move || pick.get().is_empty().to_string() on:click=move |_| pick.set(String::new())>
                                    <span class="nm">"＋ 从当前版本创建"</span><span class="muted small">"使用下方名称命名分支"</span>
                                </button>
                                {move || match branches.get() {
                                    None => view! { <LoadingState text="读取分支…"/> }.into_any(),
                                    Some(Err(e)) => view! { <InlineError message=e class="empty" retry=Callback::new(move |_| load_branches())/> }.into_any(),
                                    Some(Ok(list)) => {
                                        let k = bq.get().to_lowercase();
                                        list.into_iter().filter(|b| k.is_empty() || format!("{} {} {}", b.name, b.author, b.subject).to_lowercase().contains(&k)).take(200).map(|b| {
                                            let nm = b.name.clone();
                                            let cur = b.current;
                                            view! {
                                                <button type="button" class="brow br" aria-pressed={ let selected = nm.clone(); move || (pick.get() == selected).to_string() } data-sel=move || (pick.get() == nm).to_string() data-dis=cur.to_string()
                                                    on:click={ let n = b.name.clone(); move |_| if cur { toast("主仓正 checkout 着这个分支，不能再挂到别的工作树上") } else { pick.set(n.clone()) } }>
                                                    <span class="nm mono">{b.name.clone()}
                                                        {b.remote_only.then(|| view! { <span class="gchip">"仅远端"</span> })}
                                                        {cur.then(|| view! { <span class="gchip warn">"主仓在用"</span> })}
                                                    </span>
                                                    <span class="muted small">{format!("{} · {} · {}", b.author, fmt::ago(&b.last_commit_at), b.subject)}</span>
                                                </button>
                                            }
                                        }).collect_view().into_any()
                                    }
                                }}
                            </div>
                        </div>
                    </Show>
                </Show>
                <label class="field">"名称（留空取目录名）"<input prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
                <label class="field">"项目"
                    <input placeholder="可留空；同名项目归为一组" prop:value=move || project.get() on:input=move |e| project.set(event_target_value(&e))
                        on:keydown=move |e| if e.key() == "Enter" && !e.is_composing() { create2() }/>
                </label>
                <div class="dlg-foot">
                    <button type="button" class="btn" disabled=move || busy.get() on:click=move |_| close()>"取消"</button>
                    <button type="button" class="btn primary" disabled=move || busy.get() || !matches!(listing.get(), Some(Ok(_))) on:click=move |_| create()>
                        {move || if busy.get() { "创建中…" } else if iso.get() { "创建独立工作区" } else { "创建工作区" }}
                    </button>
                </div>
        </Modal>
    }
}
