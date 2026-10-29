use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api;
use crate::app_state::use_app;
use crate::components::toast::{toast, toast_error};
use crate::fmt;

use super::dialog::{self, Choice};
use super::icon;
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
    home: Option<String>,
}

fn remembered_dir(node: &str) -> String {
    crate::storage::load_raw(&format!("blazar.new-ws.dir.{node}")).unwrap_or_default()
}

fn remember_dir(node: &str, chosen: &str) {
    let parent = match chosen.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => return,
    };
    crate::storage::save_raw(&format!("blazar.new-ws.dir.{node}"), parent);
}

/// 家目录写成 ~，其余照原样。
fn tilde(path: &str, home: Option<&str>) -> String {
    match home.and_then(|h| path.strip_prefix(h.trim_end_matches('/'))) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

async fn confirm_broad(path: &str) -> bool {
    let (title, body) = if path == "/" {
        (
            "用磁盘根目录建工作区？",
            "根目录包含整块磁盘，读取文件列表会很慢，还可能弹出系统授权。建议选具体的项目文件夹。"
                .to_owned(),
        )
    } else {
        (
            "用整个家目录建工作区？",
            format!(
                "「{path}」是你的家目录，读取文件列表会很慢，还可能弹出系统授权。建议选具体的项目文件夹。"
            ),
        )
    };
    dialog::ask(
        title,
        &body,
        vec![Choice::plain("换一个文件夹"), Choice::danger("仍然创建")],
    )
    .await
        == Some(1)
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
            <Dialog />
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
    // 在线的在前，离线的标灰字「离线」放到最后。
    let nodes: Vec<(String, bool)> = {
        let mut v = vec![("local".to_owned(), false)];
        let mut rest = app.state.with_untracked(|s| {
            s.as_ref()
                .map(|s| {
                    s.nodes
                        .iter()
                        .filter(|n| n.is_usable())
                        .filter(|n| n.name != "local")
                        .map(|n| (n.name.clone(), n.status != "online"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        rest.sort_by_key(|(_, off)| *off);
        v.extend(rest);
        if !v.iter().any(|(n, _)| *n == preset) {
            v.push((preset.clone(), false));
        }
        v
    };
    let node = RwSignal::new(preset);
    let parent = app.new_ws_parent.get_untracked();
    let is_child = parent.is_some();
    let parent_name = parent
        .as_ref()
        .map(|p| {
            app.workspace(&p.0)
                .map_or_else(|| p.1.clone(), |w| crate::fmt::ws_label(&w.name, &w.node))
        })
        .unwrap_or_default();
    let copy = RwSignal::new(false);
    on_cleanup(move || {
        app.new_ws_node.set(None);
        app.new_ws_parent.set(None);
    });
    let path = RwSignal::new(String::new());
    let touched = RwSignal::new(false);
    let typed = RwSignal::new(String::new());
    let listing = RwSignal::new(None::<Result<Listing, (String, Option<String>)>>);
    let iso = RwSignal::new(false);
    let branches = RwSignal::new(None::<Result<Vec<Branch>, (String, Option<String>)>>);
    let pick = RwSignal::new(String::new());
    let bq = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let project = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let creating = RwSignal::new(false);
    let folder = RwSignal::new(String::new());
    let folder_busy = RwSignal::new(false);
    let folder_input = NodeRef::<html::Input>::new();
    let menu = RwSignal::new(None::<(i32, i32)>);
    let menu_item = NodeRef::<html::Button>::new();
    // 列表里单击选中的子目录；选中了它就是要建工作区的文件夹，不用先进去。
    let sel = RwSignal::new(None::<Entry>);
    // 「更多选项」：机器、副本、起始分支、名称、项目。从别的机器打开时默认展开，好让人看见选的是哪台。
    let more = RwSignal::new(node.get_untracked() != "local");
    // 桌面端连着内嵌 hub 时，本机用系统的选择文件夹面板；None 表示还在问桌面端。
    let native = RwSignal::new(if !is_child && crate::desktop::available() {
        None
    } else {
        Some(false)
    });
    // 系统面板选完、没开「更多选项」时，等目录读完就直接创建。
    let auto = RwSignal::new(false);
    let use_native = move || native.get() != Some(false) && node.get() == "local";
    let close = move || {
        if !busy.get_untracked() {
            app.new_ws.set(false);
        }
    };

    let target = move || {
        sel.with(|s| s.as_ref().map(|e| e.path.clone()))
            .or_else(|| touched.get().then(|| path.get()))
            .filter(|p| !p.is_empty())
    };
    let bseq = StoredValue::new(0u32);
    let load_branches = move || {
        branches.set(None);
        let (n, p) = (
            node.get_untracked(),
            untrack(target).unwrap_or_else(|| path.get_untracked()),
        );
        bseq.update_value(|s| *s += 1);
        let mine = bseq.get_value();
        spawn_local(async move {
            let r = api::get::<Vec<Branch>>(&format!(
                "/api/nodes/{}/branches?repo={}",
                api::enc(&n),
                api::enc(&p)
            ))
            .await
            .map_err(|e| e.explained());
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
        sel.set(None);
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
            .map_err(|e| e.explained());
            if seq.try_get_value() != Some(mine) {
                return;
            }
            if let Ok(l) = &r {
                path.set(l.path.clone());
                typed.set(l.path.clone());
                if !l.is_repo {
                    iso.set(false);
                }
                if iso.try_get_untracked() == Some(true) {
                    load_branches();
                }
            }
            let _ = listing.try_set(Some(r));
        });
    };
    let enter = move |p: String| {
        path.set(p);
        touched.set(true);
        browse();
    };
    let choose = move |e: Entry| {
        let repo = e.is_repo;
        sel.set(Some(e));
        bseq.update_value(|n| *n = n.wrapping_add(1));
        branches.set(None);
        pick.set(String::new());
        if !repo {
            iso.set(false);
        } else if iso.get_untracked() {
            load_branches();
        }
    };
    let pick_native = move || {
        if busy.get_untracked() || auto.get_untracked() {
            return;
        }
        let from = path.get_untracked();
        spawn_local(async move {
            let dir = Some(from).filter(|d| !d.is_empty());
            match crate::desktop::invoke("pick_folder", &json!({ "directory": dir })).await {
                Ok(Value::String(p)) => {
                    path.set(p);
                    touched.set(true);
                    browse();
                    let _ = auto.try_set(!more.get_untracked());
                }
                Ok(_) => {}
                Err(e) => {
                    // The driven dev instance refuses on purpose; only a real failure is news.
                    if !e.contains("界面驱动") {
                        toast("打不开系统的选择框，改用下面的目录列表");
                    }
                    let _ = native.try_set(Some(false));
                    browse();
                }
            }
        });
    };
    path.set(remembered_dir(&node.get_untracked()));
    if native.get_untracked().is_some() {
        browse();
    } else {
        spawn_local(async move {
            let here = !crate::desktop::remote_hub().await;
            if native.try_set(Some(here)).is_some() {
                return;
            }
            if here && node.get_untracked() == "local" {
                pick_native();
            } else {
                browse();
            }
        });
    }
    let is_repo = move || {
        !is_child
            && match sel.get() {
                Some(e) => e.is_repo,
                None => listing.with(|l| matches!(l, Some(Ok(l)) if l.is_repo)),
            }
    };
    let listed = move || listing.with(|l| matches!(l, Some(Ok(_))));

    let start_folder = move || {
        menu.set(None);
        if busy.get_untracked() || !listed() {
            return;
        }
        folder.set(String::new());
        creating.set(true);
        request_animation_frame(move || {
            if let Some(input) = folder_input.try_get_untracked().flatten() {
                let _ = input.focus();
            }
        });
    };
    let make_folder = move || {
        if folder_busy.get_untracked() {
            return;
        }
        let nm = folder.get_untracked().trim().to_owned();
        if nm.is_empty() {
            toast("请输入文件夹名称");
            return;
        }
        folder_busy.set(true);
        let (n, p) = (node.get_untracked(), path.get_untracked());
        spawn_local(async move {
            let r = api::send::<Value>(
                "POST",
                &format!("/api/nodes/{}/mkdir", api::enc(&n)),
                &json!({ "path": p, "name": nm }),
            )
            .await;
            let _ = folder_busy.try_set(false);
            match r {
                Ok(v) => {
                    let _ = creating.try_set(false);
                    if let Some(created) = v["path"].as_str() {
                        path.set(created.to_owned());
                        touched.set(true);
                        browse();
                    }
                }
                Err(e) => toast_error("新建文件夹失败", &e),
            }
        });
    };
    let open_menu = move |e: leptos::ev::MouseEvent| {
        e.prevent_default();
        if !listed() {
            return;
        }
        menu.set(Some((e.client_x(), e.client_y())));
        request_animation_frame(move || {
            if let Some(item) = menu_item.try_get_untracked().flatten() {
                let _ = item.focus();
            }
        });
    };
    let close_menu_down = window_event_listener(leptos::ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.closest(".ctx-menu").ok().flatten())
            .is_some();
        if !inside {
            menu.set(None);
        }
    });
    let close_menu_blur = window_event_listener(leptos::ev::blur, move |_| menu.set(None));
    on_cleanup(move || {
        close_menu_down.remove();
        close_menu_blur.remove();
    });

    let create = move || {
        if busy.get_untracked() {
            return;
        }
        if !matches!(listing.get_untracked(), Some(Ok(_))) {
            toast("请先等待目录读取完成");
            return;
        }
        let Some(p) = untrack(target) else {
            toast("请先选一个文件夹");
            return;
        };
        let broad = p == "/"
            || listing.with_untracked(
                |l| matches!(l, Some(Ok(l)) if l.home.as_deref() == Some(p.as_str())),
            );
        let leaving = parent.is_none() && crate::files_js::leave_blocked();
        if !broad && !leaving {
            busy.set(true);
        }
        let n = node.get_untracked();
        let nm = Some(name.get_untracked().trim().to_owned()).filter(|x| !x.is_empty());
        let pj = Some(project.get_untracked().trim().to_owned()).filter(|x| !x.is_empty());
        let isolated = iso.get_untracked();
        let from = Some(pick.get_untracked()).filter(|x| !x.is_empty());
        let navigate = navigate.clone();
        if let Some((pid, _)) = parent.clone() {
            let want_copy = copy.get_untracked();
            let chosen = (n.clone(), p.clone());
            spawn_local(async move {
                if broad {
                    if !confirm_broad(&p).await {
                        return;
                    }
                    let _ = busy.try_set(true);
                }
                let r = api::send::<Value>(
                    "POST",
                    &format!("/api/workspaces/{}/children", api::enc(&pid)),
                    &json!({ "node": n, "path": p, "name": nm, "copy": want_copy }),
                )
                .await;
                let _ = busy.try_set(false);
                match r {
                    Ok(v) => {
                        remember_dir(&chosen.0, &chosen.1);
                        toast(format!(
                            "已添加子工作区 {}",
                            crate::fmt::ws_label(
                                v["name"].as_str().unwrap_or(""),
                                v["node"].as_str().unwrap_or(""),
                            )
                        ));
                        app.new_ws.set(false);
                        app.load_state();
                    }
                    Err(e) => toast_error("创建失败", &e),
                }
            });
            return;
        }
        let chosen = (n.clone(), p.clone());
        spawn_local(async move {
            if leaving && !crate::files_js::confirm_leave().await {
                return;
            }
            if broad && !confirm_broad(&p).await {
                return;
            }
            let _ = busy.try_set(true);
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
                    remember_dir(&chosen.0, &chosen.1);
                    app.new_ws.set(false);
                    app.load_state();
                    if let Some(b) = v["branch"].as_str() {
                        toast(format!("副本建好了，在分支 {b} 上"));
                    }
                    if let Some(id) = v["id"].as_str() {
                        navigate(&format!("/w/{id}"), Default::default());
                    }
                }
                Err(e) => toast_error("创建失败", &e),
            }
        });
    };
    let create2 = create.clone();
    let create_auto = create.clone();
    Effect::new(move |_| {
        if !auto.get() {
            return;
        }
        match listing.get() {
            None => {}
            Some(Ok(_)) => {
                auto.set(false);
                create_auto();
            }
            Some(Err(_)) => auto.set(false),
        }
    });
    let directories = NodeRef::<html::Div>::new();
    let branch_list = NodeRef::<html::Div>::new();
    let machine = view! {
        <label class="field">
            "机器"
            <select
                prop:value=move || node.get()
                disabled=move || busy.get()
                on:change=move |e| {
                    let picked = event_target_value(&e);
                    path.set(remembered_dir(&picked));
                    touched.set(false);
                    auto.set(false);
                    node.set(picked);
                    typed.set(String::new());
                    if use_native() {
                        sel.set(None);
                        iso.set(false);
                        listing.set(None);
                    } else {
                        browse();
                    }
                }
            >
                {nodes
                    .into_iter()
                    .map(|(n, off)| {
                        let selected = n.clone();
                        let label = if off {
                            format!("{} · 离线", crate::fmt::machine(&n))
                        } else {
                            crate::fmt::machine(&n)
                        };
                        view! {
                            <option
                                value=n.clone()
                                class:off=off
                                selected=move || node.get() == selected
                            >
                                {label}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>
        </label>
    };
    let (machine_top, machine_more) = if is_child {
        (Some(machine), None)
    } else {
        (None, Some(machine))
    };
    let name_field = view! {
        <label class="field">
            "名称（留空取目录名）"
            <input
                prop:value=move || name.get()
                on:input=move |e| name.set(event_target_value(&e))
            />
        </label>
    };
    let (name_top, name_more) = if is_child {
        (Some(name_field), None)
    } else {
        (None, Some(name_field))
    };
    let shown = move || {
        target().map(|p| {
            let home = listing.with(|l| match l {
                Some(Ok(l)) => l.home.clone(),
                _ => None,
            });
            tilde(&p, home.as_deref())
        })
    };

    view! {
        <Modal label="新建工作区" class="dlg wide" on_close=Callback::new(move |_| close())>
            <h3>{if is_child { "新建子工作区" } else { "新建工作区" }}</h3>
            {is_child
                .then(|| {
                    view! {
                        <p class="muted">
                            {format!(
                                "挂在「{parent_name}」名下。智能体在那个工作区里开新一轮时会知道这台机器可用，可以把活派过去。",
                            )}
                        </p>
                    }
                })}
            {machine_top}
            <div class="field" hidden=move || !use_native()>
                <span class="muted">
                    {move || {
                        if more.get() {
                            "用系统的选择框选一个项目文件夹。"
                        } else {
                            "用系统的选择框选一个项目文件夹，选好就建工作区并打开。"
                        }
                    }}
                </span>
                {move || {
                    touched
                        .get()
                        .then(|| {
                            view! {
                                <div class="new-ws-picked">
                                    <span class="mono grow">{shown}</span>
                                    {move || match listing.get() {
                                        None => {
                                            view! { <span class="muted small">"读取中…"</span> }
                                                .into_any()
                                        }
                                        Some(Err((e, detail))) => {
                                            view! {
                                                <InlineError
                                                    message=e
                                                    detail=detail
                                                    retry=Callback::new(move |_| browse())
                                                />
                                            }
                                                .into_any()
                                        }
                                        Some(Ok(_)) => ().into_any(),
                                    }}
                                </div>
                            }
                        })
                }}
            </div>
            <div class="field" hidden=use_native>
                "目录"
                <div class="browse-bar">
                    <button
                        type="button"
                        class="btn small"
                        aria-label="上一级目录"
                        title="上一级"
                        disabled=move || {
                            listing.with(|l| !matches!(l, Some(Ok(l)) if l.parent.is_some()))
                        }
                        on:click=move |_| {
                            if let Some(Some(Ok(l))) = listing.try_get_untracked()
                                && let Some(p) = l.parent
                            {
                                path.set(p);
                                touched.set(true);
                                browse();
                            }
                        }
                    >
                        "↑"
                    </button>
                    <input
                        class="mono"
                        aria-label="目录路径"
                        placeholder="~"
                        prop:value=move || typed.get()
                        on:input=move |e| typed.set(event_target_value(&e))
                        on:keydown=move |e| {
                            if e.key() == "Enter" && !e.is_composing() {
                                path.set(typed.get_untracked().trim().to_owned());
                                touched.set(true);
                                browse();
                            }
                        }
                    />
                    <button
                        type="button"
                        class="btn small"
                        on:click=move |_| {
                            path.set(typed.get_untracked().trim().to_owned());
                            touched.set(true);
                            browse();
                        }
                    >
                        "前往"
                    </button>
                    <button
                        type="button"
                        class="btn small"
                        title="在当前目录下新建文件夹"
                        disabled=move || busy.get() || !listed()
                        on:click=move |_| start_folder()
                    >
                        "新建文件夹"
                    </button>
                </div>
                <div
                    class="browser"
                    node_ref=directories
                    role="group"
                    aria-label="子目录"
                    data-keyboard-list=""
                    on:contextmenu=open_menu
                    on:keydown=move |event| {
                        if let Some(root) = directories.get_untracked() {
                            menu_keydown(
                                &event,
                                root.unchecked_ref(),
                                Callback::new(move |_| close()),
                            );
                        }
                    }
                >
                    {move || {
                        creating
                            .get()
                            .then(|| {
                                view! {
                                    <div class="brow newdir">
                                        <span class="mk" aria-hidden="true">
                                            "▸"
                                        </span>
                                        <input
                                            node_ref=folder_input
                                            aria-label="文件夹名称"
                                            placeholder="文件夹名称"
                                            autocomplete="off"
                                            spellcheck="false"
                                            prop:value=move || folder.get()
                                            on:input=move |e| folder.set(event_target_value(&e))
                                            on:keydown=move |e| match e.key().as_str() {
                                                "Enter" if !e.is_composing() => {
                                                    e.prevent_default();
                                                    make_folder()
                                                }
                                                "Escape" => {
                                                    e.prevent_default();
                                                    e.stop_propagation();
                                                    creating.set(false)
                                                }
                                                _ => {}
                                            }
                                        />
                                        <button
                                            type="button"
                                            class="btn small primary"
                                            disabled=move || folder_busy.get()
                                            on:click=move |_| make_folder()
                                        >
                                            {move || {
                                                if folder_busy.get() { "创建中…" } else { "创建" }
                                            }}
                                        </button>
                                        <button
                                            type="button"
                                            class="btn small ghost"
                                            disabled=move || folder_busy.get()
                                            on:click=move |_| creating.set(false)
                                        >
                                            "取消"
                                        </button>
                                    </div>
                                }
                            })
                    }}
                    {move || match listing.get() {
                        None => view! { <LoadingState text="读取中…" /> }.into_any(),
                        Some(Err((e, detail))) => {
                            view! {
                                <InlineError
                                    message=e
                                    detail=detail
                                    class="empty"
                                    retry=Callback::new(move |_| browse())
                                />
                            }
                                .into_any()
                        }
                        Some(Ok(l)) if l.entries.is_empty() => {
                            view! {
                                <EmptyState
                                    title="这个目录下没有子目录"
                                    detail="右键或点「新建文件夹」可以在这里建一个。"
                                />
                            }
                                .into_any()
                        }
                        Some(Ok(l)) => {
                            l.entries
                                .into_iter()
                                .map(|e| {
                                    let p = e.path.clone();
                                    let p2 = e.path.clone();
                                    let mine = e.path.clone();
                                    let selected = Memo::new(move |_| {
                                        sel.with(|s| s.as_ref().is_some_and(|s| s.path == mine))
                                    });
                                    let entry = e.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="brow"
                                            title=format!("{} · 双击打开", e.path)
                                            aria-label=if e.is_repo {
                                                format!("{}，Git 仓库", e.name)
                                            } else {
                                                e.name.clone()
                                            }
                                            aria-pressed=move || selected.get().to_string()
                                            data-sel=move || selected.get().to_string()
                                            on:click=move |_| choose(entry.clone())
                                            on:dblclick=move |_| enter(p.clone())
                                            on:keydown=move |ev| {
                                                if ev.key() == "Enter" && !ev.is_composing() {
                                                    ev.prevent_default();
                                                    enter(p2.clone());
                                                }
                                            }
                                        >
                                            <span
                                                class="mk"
                                                aria-hidden="true"
                                                inner_html=icon::svg(icon::FOLDER)
                                            ></span>
                                            <span class="nm">
                                                {e.name.clone()}
                                                {e
                                                    .is_repo
                                                    .then(|| view! { <span class="tag">"Git"</span> })}
                                            </span>
                                            {e
                                                .children
                                                .map(|c| view! { <span class="muted small">{c}</span> })}
                                        </button>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }
                    }}
                </div>
                {move || {
                    menu.get()
                        .map(|(x, y)| {
                            let w = window()
                                .inner_width()
                                .ok()
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0) as i32;
                            let h = window()
                                .inner_height()
                                .ok()
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0) as i32;
                            let (x, y) = (x.min(w - 180).max(0), y.min(h - 44).max(0));
                            view! {
                                <div
                                    class="ctx-menu"
                                    role="menu"
                                    style=format!("left:{x}px;top:{y}px")
                                    on:keydown=move |e| {
                                        if e.key() == "Escape" {
                                            e.prevent_default();
                                            e.stop_propagation();
                                            menu.set(None);
                                        }
                                    }
                                >
                                    <button
                                        type="button"
                                        role="menuitem"
                                        class="ctx-item"
                                        node_ref=menu_item
                                        on:click=move |_| start_folder()
                                    >
                                        "新建文件夹"
                                    </button>
                                </div>
                            }
                        })
                }}
            </div>
            {is_child
                .then(|| {
                    view! {
                        <label
                            class="chk block"
                            title="按 git 规则跳过被忽略的内容；目录里已有的同名文件会被覆盖"
                        >
                            <input
                                type="checkbox"
                                prop:checked=move || copy.get()
                                on:change=move |_| copy.update(|c| *c = !*c)
                            />
                            <span>
                                <b>"把父工作区的文件拷过去"</b>
                                <span class="muted small">
                                    " 跳过 git 忽略的内容，最多 128MB。"
                                </span>
                            </span>
                        </label>
                    }
                })}
            {name_top}
            <details
                class="new-ws-more"
                hidden=is_child
                open=more.get_untracked()
                on:toggle=move |e| {
                    let open = e
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlDetailsElement>().ok())
                        .is_some_and(|d| d.open());
                    more.set(open);
                }
            >
                <summary>"更多选项"</summary>
                {machine_more}
                <Show when=is_repo>
                    <label
                        class="chk block"
                        title="先取远端最新的分支，再在单独的目录里放一份副本。"
                    >
                        <input
                            type="checkbox"
                            aria-label="在副本里改"
                            aria-describedby="new-ws-iso-desc"
                            prop:checked=move || iso.get()
                            on:change=move |_| {
                                iso.update(|i| *i = !*i);
                                if iso.get_untracked() && branches.with_untracked(Option::is_none) {
                                    load_branches();
                                }
                            }
                        />
                        <span>
                            <b>"在副本里改"</b>
                            <span class="muted small" id="new-ws-iso-desc">
                                " 另放一份代码副本、开一个新分支，原目录不受影响。"
                            </span>
                        </span>
                    </label>
                    <Show when=move || iso.get()>
                        <div
                            class="field"
                            title="按最近提交排序，会先取远端最新的分支。"
                        >
                            "从哪里开始"
                            <input
                                aria-label="筛选分支"
                                placeholder="筛选分支…"
                                prop:value=move || bq.get()
                                on:input=move |e| bq.set(event_target_value(&e))
                            />
                            <div
                                class="browser short"
                                node_ref=branch_list
                                role="group"
                                aria-label="可选分支"
                                data-keyboard-list=""
                                on:keydown=move |event| {
                                    if let Some(root) = branch_list.get_untracked() {
                                        menu_keydown(
                                            &event,
                                            root.unchecked_ref(),
                                            Callback::new(move |_| close()),
                                        );
                                    }
                                }
                            >
                                <button
                                    type="button"
                                    class="brow br"
                                    aria-pressed=move || pick.get().is_empty().to_string()
                                    data-sel=move || pick.get().is_empty().to_string()
                                    on:click=move |_| pick.set(String::new())
                                >
                                    <span class="nm">"从现在的代码开始"</span>
                                    <span class="muted small">"新分支用下面的名称"</span>
                                </button>
                                {move || match branches.get() {
                                    None => {
                                        view! { <LoadingState text="读取分支…" /> }.into_any()
                                    }
                                    Some(Err((e, detail))) => {
                                        view! {
                                            <InlineError
                                                message=e
                                                detail=detail
                                                class="empty"
                                                retry=Callback::new(move |_| load_branches())
                                            />
                                        }
                                            .into_any()
                                    }
                                    Some(Ok(list)) => {
                                        let k = bq.get().to_lowercase();
                                        list.into_iter()
                                            .filter(|b| {
                                                k.is_empty()
                                                    || format!("{} {} {}", b.name, b.author, b.subject)
                                                        .to_lowercase()
                                                        .contains(&k)
                                            })
                                            .take(200)
                                            .map(|b| {
                                                let nm = b.name.clone();
                                                let cur = b.current;
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="brow br"
                                                        aria-pressed={
                                                            let selected = nm.clone();
                                                            move || (pick.get() == selected).to_string()
                                                        }
                                                        data-sel=move || (pick.get() == nm).to_string()
                                                        data-dis=cur.to_string()
                                                        on:click={
                                                            let n = b.name.clone();
                                                            move |_| {
                                                                if cur {
                                                                    toast(
                                                                        "原目录正在用这个分支，选别的分支，或者新开一个",
                                                                    )
                                                                } else {
                                                                    pick.set(n.clone())
                                                                }
                                                            }
                                                        }
                                                    >
                                                        <span class="nm mono">
                                                            {b.name.clone()}
                                                            {b
                                                                .remote_only
                                                                .then(|| view! { <span class="gchip">"仅远端"</span> })}
                                                            {cur
                                                                .then(|| {
                                                                    view! { <span class="gchip warn">"原目录在用"</span> }
                                                                })}
                                                        </span>
                                                        <span class="muted small">
                                                            {format!(
                                                                "{} · {} · {}",
                                                                b.author,
                                                                fmt::ago(&b.last_commit_at),
                                                                b.subject,
                                                            )}
                                                        </span>
                                                    </button>
                                                }
                                            })
                                            .collect_view()
                                            .into_any()
                                    }
                                }}
                            </div>
                        </div>
                    </Show>
                </Show>
                {name_more}
                {(!is_child)
                    .then(|| {
                        view! {
                            <label class="field">
                                "项目"
                                <input
                                    placeholder="可留空；同名项目归为一组"
                                    prop:value=move || project.get()
                                    on:input=move |e| project.set(event_target_value(&e))
                                    on:keydown=move |e| {
                                        if e.key() == "Enter" && !e.is_composing() {
                                            create2()
                                        }
                                    }
                                />
                            </label>
                        }
                    })}
            </details>
            <div class="dlg-foot">
                <span class="muted small grow">
                    {move || {
                        if use_native() {
                            None
                        } else {
                            Some(
                                shown()
                                    .map_or_else(
                                        || {
                                            "单击选中一个文件夹，双击打开它".to_owned()
                                        },
                                        |p| format!("用「{p}」"),
                                    ),
                            )
                        }
                    }}
                </span>
                <button
                    type="button"
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| close()
                >
                    "取消"
                </button>
                {move || {
                    use_native()
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class=move || {
                                        if touched.get() { "btn" } else { "btn primary" }
                                    }
                                    disabled=move || {
                                        busy.get() || auto.get() || native.get().is_none()
                                    }
                                    on:click=move |_| pick_native()
                                >
                                    {move || {
                                        if touched.get() {
                                            "换一个文件夹…"
                                        } else {
                                            "选择文件夹…"
                                        }
                                    }}
                                </button>
                            }
                        })
                }}
                {move || {
                    let create = create.clone();
                    (!use_native() || touched.get() && (!auto.get() || busy.get()))
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="btn primary"
                                    disabled=move || busy.get() || target().is_none() || !listed()
                                    on:click=move |_| create()
                                >
                                    {move || {
                                        if busy.get() {
                                            "创建中…"
                                        } else if is_child {
                                            "添加子工作区"
                                        } else if iso.get() {
                                            "创建副本"
                                        } else {
                                            "创建工作区"
                                        }
                                    }}
                                </button>
                            }
                        })
                }}
            </div>
        </Modal>
    }
}
