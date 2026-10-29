use std::collections::HashSet;

use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, closure::Closure};

use crate::app_state::use_app;
use crate::components::dialog::{self, Choice};
use crate::components::empty::{self, Action, Blank};
use crate::components::group::ListGroup;
use crate::components::icon;
use crate::components::modal::Modal;
use crate::components::page_head::{MenuItem, MoreMenu, PageHead};
use crate::components::rows::{RowAction, RowKey, RowMenu, Rows, open_row};
use crate::components::status::{EmptyState, InlineError, LoadingState, StateOf};
use crate::components::toast::{toast, toast_error};
use crate::components::undo::{self, Request};
use crate::{api, fmt, storage};

pub(super) fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

fn list(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
fn n(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or_default()
}
fn archived(v: &Value) -> bool {
    v["archived_at"].as_str().is_some()
}
fn availability(a: &Value, runtimes: &[api::Runtime]) -> &'static str {
    if archived(a) {
        return "archived";
    }
    match runtimes.iter().find(|r| r.id == text(a, "runtime")) {
        None => "unbound",
        Some(r) if !r.installed => "unbound",
        Some(r) if r.authed == Some(false) => "offline",
        _ => "online",
    }
}
fn status_label(status: &str) -> &str {
    match status {
        "archived" => "已归档",
        "unbound" => "待配置",
        "offline" => "需登录",
        "online" => "可用",
        other => crate::vocab::state(other).0,
    }
}
pub(super) async fn confirm(title: &str, body: &str) -> bool {
    dialog::ask(
        title,
        body,
        vec![
            Choice::plain("取消"),
            Choice::danger(title.trim_end_matches('？').trim_end_matches('?')),
        ],
    )
    .await
        == Some(1)
}

pub(super) fn remove_profile(agent: &Value, forever: bool, done: impl Fn() + 'static) {
    let id = text(agent, "id");
    let name = text(agent, "name");
    let (text, request) = if forever {
        (
            format!("已删除「{name}」"),
            Request::delete(format!("/api/agent-profiles/{}", api::enc(&id))),
        )
    } else {
        (
            format!("已归档「{name}」，正在运行的任务会被取消"),
            Request::post(format!("/api/agent-profiles/{}/archive", api::enc(&id)))
                .failing("没归档"),
        )
    };
    undo::remove_row(text, id, request, done);
}

pub(super) fn guard_unsaved(dirty: RwSignal<bool>) {
    let listener = Closure::<dyn Fn(web_sys::Event)>::new(move |e: web_sys::Event| {
        if dirty.try_get_untracked().unwrap_or(false) {
            e.prevent_default();
        }
    });
    let _ = window().add_event_listener_with_callback(
        "blazar:before-navigate",
        listener.as_ref().unchecked_ref(),
    );
    let listener = StoredValue::new_local(listener);
    let unload = window_event_listener(ev::beforeunload, move |e| {
        if dirty.try_get_untracked().unwrap_or(false) {
            e.prevent_default();
        }
    });
    on_cleanup(move || {
        listener.with_value(|l| {
            let _ = window().remove_event_listener_with_callback(
                "blazar:before-navigate",
                l.as_ref().unchecked_ref(),
            );
        });
        listener.dispose();
        unload.remove();
    });
}

#[component]
pub fn AgentsPage() -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0u32);
    let pref: Value = storage::load("blazar.agents.view").unwrap_or(json!({}));
    let scope = RwSignal::new(
        if text(&pref, "scope") == "archived" {
            "archived"
        } else {
            "all"
        }
        .to_owned(),
    );
    let query = RwSignal::new(String::new());
    let rt = RwSignal::new(text(&pref, "runtime"));
    let avail = RwSignal::new(if text(&pref, "avail").is_empty() {
        "all".into()
    } else {
        text(&pref, "avail")
    });
    let sort = RwSignal::new(if text(&pref, "sort").is_empty() {
        "recent".into()
    } else {
        text(&pref, "sort")
    });
    let asc = RwSignal::new(text(&pref, "dir") == "asc");
    let selected = RwSignal::new(HashSet::<String>::new());
    let busy = RwSignal::new(false);
    let runtimes = LocalResource::new(|| api::get::<api::Runtimes>("/api/runtimes"));
    let profiles = LocalResource::new(move || {
        rev.track();
        let scope = scope.get();
        async move {
            api::get::<Vec<Value>>(if scope == "archived" {
                "/api/agent-profiles?scope=archived"
            } else {
                "/api/agent-profiles"
            })
            .await
        }
    });
    let has_archived = LocalResource::new(move || {
        rev.track();
        async move {
            api::get::<Vec<Value>>("/api/agent-profiles?scope=archived")
                .await
                .is_ok_and(|l| !l.is_empty())
        }
    });
    let has_any = move || {
        scope.get() == "archived"
            || has_archived.get() == Some(true)
            || profiles.with(|p| matches!(p, Some(Ok(l)) if !l.is_empty()))
    };
    let navigate = use_navigate();
    let nav = StoredValue::new(navigate.clone());
    let create = Callback::new(move |()| navigate("/agents/new", Default::default()));
    let rows = Rows::default();
    let shown = RwSignal::new(Vec::<Value>::new());
    let open_profile = move |a: &Value| {
        let href = format!("/agent/{}", api::enc(&text(a, "id")));
        nav.with_value(|n| n(&href, Default::default()));
    };
    rows.listen(
        "tr.agent-row",
        move || shown.try_with_untracked(Vec::len).unwrap_or(0),
        move |key, i| {
            let Some(a) = shown.try_with_untracked(|l| l.get(i).cloned()).flatten() else {
                return;
            };
            match key {
                RowKey::Open | RowKey::Inspect => open_profile(&a),
                RowKey::Delete => remove_profile(&a, archived(&a), move || {
                    rev.try_update(|r| *r += 1);
                }),
            }
        },
    );
    let act_on = move |id: String, action: &'static str| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/agent-profiles/{}/{action}", api::enc(&id)),
                &json!({}),
            )
            .await
            {
                Ok(_) => rev.update(|r| *r += 1),
                Err(e) => toast_error("没处理成", &e),
            }
            busy.set(false);
        });
    };
    let menu_of = move |a: Value| {
        let id = text(&a, "id");
        let gone = archived(&a);
        let copy = format!("/agents/new?template={}", api::enc(&id));
        let (for_restore, for_delete) = (id.clone(), a.clone());
        vec![
            vec![RowAction::open(format!("/agent/{}", api::enc(&id)))],
            vec![RowAction::new("复制", move || {
                nav.with_value(|n| n(&copy, Default::default()));
            })],
            if gone {
                vec![RowAction::new("恢复", move || {
                    act_on(for_restore.clone(), "restore")
                })]
            } else {
                Vec::new()
            },
            vec![RowAction::delete(
                if gone { "删除" } else { "归档" },
                move || {
                    remove_profile(&for_delete, gone, move || {
                        rev.try_update(|r| *r += 1);
                    });
                },
            )],
        ]
    };
    Effect::new(move |_| {
        storage::save(
            "blazar.agents.view",
            &json!({ "scope":scope.get(), "runtime":rt.get(), "avail":avail.get(), "sort":sort.get(), "dir":if asc.get() {"asc"} else {"desc"} }),
        )
    });
    Effect::new(move |_| {
        if scope.get() == "all"
            && let Some(Ok(p)) = profiles.get()
        {
            app.agents_n.set(p.len());
        }
    });
    let batch = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let action = if scope.get_untracked() == "archived" {
                "restore"
            } else {
                "archive"
            };
            let ids = selected.get_untracked();
            // Archiving can be taken back, so it gets the same 5-second undo as one row
            // rather than a confirm; the toast says what it does to running work.
            if action == "archive" {
                let rows = ids
                    .iter()
                    .map(|id| {
                        let path = format!("/api/agent-profiles/{}/archive", api::enc(id));
                        (id.clone(), Request::post(path).failing("没归档"))
                    })
                    .collect();
                undo::remove_rows(
                    format!("已归档 {} 个智能体，正在运行的任务会被取消", ids.len()),
                    rows,
                    move || rev.update(|v| *v += 1),
                );
                selected.set(HashSet::new());
                busy.set(false);
                return;
            }
            let mut failures = 0;
            for id in &ids {
                if api::send::<Value>(
                    "POST",
                    &format!("/api/agent-profiles/{}/{action}", api::enc(id)),
                    &json!({}),
                )
                .await
                .is_err()
                {
                    failures += 1;
                }
            }
            toast(format!(
                "已处理 {} 个，失败 {} 个",
                ids.len() - failures,
                failures
            ));
            selected.set(HashSet::new());
            rev.update(|v| *v += 1);
            busy.set(false);
        });
    };
    view! {
        <div class="page wide agent-page">
            <PageHead title="智能体" nav=|| crate::components::section::agents("/agents")>
                <a class="btn primary" href="/agents/new">
                    "新建智能体"
                </a>
            </PageHead>
            <Show when=has_any>
                <div class="agent-toolbar">
                    <select
                        class="filter"
                        disabled=move || busy.get()
                        aria-label="范围"
                        prop:value=move || scope.get()
                        on:change=move |e| {
                            scope.set(event_target_value(&e));
                            selected.set(HashSet::new());
                        }
                    >
                        <option value="all" prop:selected=move || scope.get() == "all">
                            "范围：在用的"
                        </option>
                        <option value="archived" prop:selected=move || scope.get() == "archived">
                            "范围：已归档"
                        </option>
                    </select>
                    <input
                        class="filter"
                        aria-label="搜索智能体"
                        placeholder="搜索名称或描述…"
                        prop:value=move || query.get()
                        on:input=move |e| query.set(event_target_value(&e))
                    />
                    <select
                        class="filter"
                        aria-label="可用性"
                        prop:value=move || avail.get()
                        on:change=move |e| avail.set(event_target_value(&e))
                    >
                        {[
                            ("all", "全部"),
                            ("online", "可用"),
                            ("offline", "需登录"),
                            ("unbound", "待配置"),
                        ]
                            .into_iter()
                            .map(|(v, l)| {
                                view! {
                                    <option value=v prop:selected=move || avail.get() == v>
                                        {format!("状态：{l}")}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                    <select
                        class="filter"
                        aria-label="Claude Code / Codex"
                        prop:value=move || rt.get()
                        on:change=move |e| rt.set(event_target_value(&e))
                    >
                        <option value="" prop:selected=move || rt.get().is_empty()>
                            "全部"
                        </option>
                        {move || {
                            runtimes
                                .get()
                                .and_then(Result::ok)
                                .map(|r| {
                                    r.runtimes
                                        .into_iter()
                                        .map(|r| {
                                            let id = r.id.clone();
                                            view! {
                                                <option value=r.id prop:selected=move || rt.get() == id>
                                                    {r.label}
                                                </option>
                                            }
                                        })
                                        .collect_view()
                                })
                        }}
                    </select>
                    <select
                        class="filter"
                        aria-label="排序"
                        prop:value=move || sort.get()
                        on:change=move |e| sort.set(event_target_value(&e))
                    >
                        {[
                            ("recent", "最近活跃"),
                            ("name", "名称"),
                            ("runs", "运行次数"),
                            ("created", "创建时间"),
                        ]
                            .into_iter()
                            .map(|(v, l)| {
                                view! {
                                    <option value=v prop:selected=move || sort.get() == v>
                                        {format!("排序：{l}")}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                    <button
                        class="btn"
                        aria-label=move || if asc.get() { "升序" } else { "降序" }
                        on:click=move |_| asc.update(|v| *v = !*v)
                    >
                        {move || if asc.get() { "↑ 升序" } else { "↓ 降序" }}
                    </button>
                    <span class="grow"></span>
                    <button
                        class="btn"
                        disabled=move || profiles.get().is_none()
                        on:click=move |_| rev.update(|v| *v += 1)
                    >
                        "刷新"
                    </button>
                </div>
            </Show>
            {move || {
                runtimes
                    .get()
                    .and_then(Result::err)
                    .map(|e| {
                        view! {
                            <InlineError
                                lead="无法检查 Claude Code / Codex"
                                error=e
                                retry=Callback::new(move |_| runtimes.refetch())
                            />
                        }
                    })
            }}
            {move || {
                (!selected.get().is_empty())
                    .then(|| {
                        view! {
                            <div class="agent-toolbar">
                                <b>{move || format!("已选 {} 个", selected.get().len())}</b>
                                <button
                                    class="btn"
                                    disabled=move || busy.get()
                                    on:click=move |_| selected.set(HashSet::new())
                                >
                                    "清除选择"
                                </button>
                                <button class="btn" disabled=move || busy.get() on:click=batch>
                                    {move || {
                                        if busy.get() {
                                            "处理中…"
                                        } else if scope.get() == "archived" {
                                            "恢复"
                                        } else {
                                            "归档"
                                        }
                                    }}
                                </button>
                            </div>
                        }
                    })
            }}
            {move || match profiles.get() {
                None => view! { <LoadingState text="正在加载智能体…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            lead="无法加载智能体"
                            error=e
                            retry=Callback::new(move |_| rev.update(|v| *v += 1))
                        />
                    }
                        .into_any()
                }
                Some(Ok(mut profiles)) => {
                    let has_profiles = !profiles.is_empty();
                    let q = query.get().to_lowercase();
                    let r = rt.get();
                    let av = avail.get();
                    let rts_ready = matches!(runtimes.get(), Some(Ok(_)));
                    let rts = runtimes
                        .get()
                        .and_then(Result::ok)
                        .map(|v| v.runtimes)
                        .unwrap_or_default();
                    profiles
                        .retain(|a| {
                            undo::shows(&text(a, "id"))
                                && (q.is_empty()
                                    || format!("{} {}", text(a, "name"), text(a, "description"))
                                        .to_lowercase()
                                        .contains(&q)) && (r.is_empty() || text(a, "runtime") == r)
                                && (av == "all" || availability(a, &rts) == av)
                        });
                    let order = sort.get();
                    profiles
                        .sort_by(|a, b| {
                            let cmp = match order.as_str() {
                                "runs" => n(a, "runs_30d").cmp(&n(b, "runs_30d")),
                                "name" => text(a, "name").cmp(&text(b, "name")),
                                "created" => text(a, "created_at").cmp(&text(b, "created_at")),
                                _ => text(a, "last_used_at").cmp(&text(b, "last_used_at")),
                            };
                            if asc.get() { cmp } else { cmp.reverse() }
                        });
                    shown.set(profiles.clone());
                    if profiles.is_empty() {
                        let archived_scope = scope.get() == "archived";
                        return if has_profiles {
                            view! {
                                <Blank
                                    icon=icon::FILTER
                                    title=empty::unmatched("智能体")
                                    detail="调整搜索或筛选条件后再试。"
                                    action=Action::clear(move || {
                                        query.set(String::new());
                                        rt.set(String::new());
                                        avail.set("all".into());
                                    })
                                />
                            }
                                .into_any()
                        } else if archived_scope {
                            view! {
                                <Blank
                                    icon=icon::ID_CARD
                                    title=empty::never("归档的智能体")
                                    detail="归档的智能体会保留在这里，随时可以恢复。"
                                    action=Action::new(
                                        "看在用的",
                                        move || scope.set("all".into()),
                                    )
                                />
                            }
                                .into_any()
                        } else {
                            view! {
                                <Blank
                                    icon=icon::ID_CARD
                                    title=empty::never("智能体")
                                    detail="智能体记下一套指令、用 Claude Code 还是 Codex、用哪个模型，之后可以直接拿来对话或派活。"
                                    action=Action::new("新建智能体", move || create.run(()))
                                />
                            }
                                .into_any()
                        };
                    }
                    view! {
                        <ListGroup>
                            <div class="agent-table">
                                <table>
                                    <thead>
                                        <tr>
                                            <th>"选择"</th>
                                            <th>"智能体"</th>
                                            <th>"状态"</th>
                                            <th>"Claude Code / Codex · 模型"</th>
                                            <th>"近 30 天"</th>
                                            <th>"最近活跃"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {profiles
                                            .into_iter()
                                            .enumerate()
                                            .map(|(i, a)| {
                                                let id = text(&a, "id");
                                                let check_id = id.clone();
                                                let toggle_id = id.clone();
                                                let status = availability(&a, &rts);
                                                view! {
                                                    <tr
                                                        class="agent-row link-row"
                                                        tabindex="-1"
                                                        data-index=i.to_string()
                                                        data-sel=move || rows.selected(i).to_string()
                                                        data-menu=move || rows.menu_on(i).to_string()
                                                        on:focusin=move |_| rows.sel.set(Some(i))
                                                        on:contextmenu=move |e| rows.open_menu(&e, i)
                                                        on:click=move |e: ev::MouseEvent| open_row(
                                                            &e,
                                                            "a.agent-name",
                                                        )
                                                    >
                                                        <td>
                                                            <input
                                                                type="checkbox"
                                                                disabled=move || busy.get()
                                                                aria-label=format!("选择 {}", text(&a, "name"))
                                                                prop:checked=move || {
                                                                    selected.with(|s| s.contains(&check_id))
                                                                }
                                                                on:change=move |e| {
                                                                    selected
                                                                        .update(|s| {
                                                                            if event_target_checked(&e) {
                                                                                s.insert(toggle_id.clone());
                                                                            } else {
                                                                                s.remove(&toggle_id);
                                                                            }
                                                                        })
                                                                }
                                                            />
                                                        </td>
                                                        <td>
                                                            <a
                                                                class="agent-name"
                                                                href=format!("/agent/{}", api::enc(&id))
                                                            >
                                                                <span>
                                                                    {if text(&a, "avatar").is_empty() {
                                                                        "🤖".into()
                                                                    } else {
                                                                        text(&a, "avatar")
                                                                    }}
                                                                </span>
                                                                <b>{text(&a, "name")}</b>
                                                            </a>
                                                            <div class="muted">{text(&a, "description")}</div>
                                                        </td>
                                                        <td>
                                                            <span
                                                                class="status"
                                                                data-tone=if rts_ready && status == "offline" {
                                                                    "attention"
                                                                } else {
                                                                    "other"
                                                                }
                                                            >
                                                                {if rts_ready { status_label(status) } else { "待检查" }}
                                                            </span>
                                                            {(n(&a, "running") > 0)
                                                                .then(|| {
                                                                    view! {
                                                                        <div class="muted">
                                                                            {format!("{} 次运行中", n(&a, "running"))}
                                                                        </div>
                                                                    }
                                                                })}
                                                        </td>
                                                        <td>
                                                            {text(&a, "runtime_label")}
                                                            <div class="muted mono">
                                                                {if text(&a, "model").is_empty() {
                                                                    "默认".into()
                                                                } else {
                                                                    text(&a, "model")
                                                                }}
                                                            </div>
                                                        </td>
                                                        <td>
                                                            {format!("{} 次运行", n(&a, "runs_30d"))}
                                                            {(n(&a, "failed_30d") > 0)
                                                                .then(|| {
                                                                    view! {
                                                                        <div class="muted">
                                                                            {format!("{} 次失败", n(&a, "failed_30d"))}
                                                                        </div>
                                                                    }
                                                                })}
                                                        </td>
                                                        <td>
                                                            {if text(&a, "last_used_at").is_empty() {
                                                                "尚未运行".into()
                                                            } else {
                                                                fmt::ago(&text(&a, "last_used_at"))
                                                            }}
                                                        </td>
                                                    </tr>
                                                }
                                            })
                                            .collect_view()}
                                    </tbody>
                                </table>
                            </div>
                        </ListGroup>
                    }
                        .into_any()
                }
            }}
            {move || {
                rows.menu
                    .get()
                    .and_then(|at| shown.with(|l| l.get(at.index).cloned()).map(|a| (at, a)))
                    .map(|(at, a)| {
                        view! {
                            <RowMenu
                                at
                                label=format!("{} 的操作", text(&a, "name"))
                                groups=menu_of(a)
                                on_close=Callback::new(move |()| rows.close_menu())
                            />
                        }
                    })
            }}
        </div>
    }
}

#[component]
pub fn AgentCreatePage() -> impl IntoView {
    let query = use_query_map();
    let data = LocalResource::new(move || {
        let template = query.with(|q| q.get("template"));
        let runtime = query.with(|q| q.get("runtime")).unwrap_or_default();
        async move {
            if let Some(id) = template {
                let mut v =
                    api::get::<Value>(&format!("/api/agent-profiles/{}", api::enc(&id))).await?;
                v["name"] = json!(format!("{}（副本）", text(&v, "name")));
                v["env"] = json!({});
                v["id"] = json!("");
                v["archived_at"] = Value::Null;
                Ok::<Value, api::ApiError>(v)
            } else {
                Ok(
                    json!({"name":"","runtime":runtime,"env":{},"custom_args":[],"starters":[],"max_concurrent":1}),
                )
            }
        }
    });
    view! {
        <div class="page agent-page">
            <PageHead
                parent=("智能体", "/agents")
                title=Signal::derive(move || {
                    if query.with(|q| q.get("template").is_some()) {
                        "复制智能体".to_owned()
                    } else {
                        "新建智能体".to_owned()
                    }
                })
            />
            {move || match data.get() {
                None => view! { <LoadingState text="正在加载配置…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            lead="无法加载配置"
                            error=e
                            retry=Callback::new(move |_| data.refetch())
                        />
                    }
                        .into_any()
                }
                Some(Ok(a)) => {
                    let copied = query.with_untracked(|q| q.get("template").is_some());
                    view! {
                        <AgentEditor initial=a create=true copied on_saved=Callback::new(|_| {}) />
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
pub fn AgentProfilePage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let rev = RwSignal::new(0u32);
    let dirty = RwSignal::new(false);
    let tab = RwSignal::new(query.with_untracked(|q| match q.get("view").as_deref() {
        Some("settings") => "settings".into(),
        Some("work") => "work".into(),
        Some("capabilities") => q.get("tab").unwrap_or("instructions".into()),
        _ => "overview".into(),
    }));
    let navigate = StoredValue::new(use_navigate());
    Effect::new(move |_| {
        let next = query.with(|q| match q.get("view").as_deref() {
            Some("settings") => "settings".to_owned(),
            Some("work") => "work".to_owned(),
            Some("capabilities") => q.get("tab").unwrap_or("instructions".into()),
            _ => "overview".to_owned(),
        });
        if !dirty.get_untracked() && tab.get_untracked() != next {
            tab.set(next);
        }
    });
    let data = LocalResource::new(move || {
        rev.track();
        let id = params.with(|p| p.get("id")).unwrap_or_default();
        async move { api::get::<Value>(&format!("/api/agent-profiles/{}", api::enc(&id))).await }
    });
    view! {
        <div class="page wide agent-page">
            {move || match data.get() {
                None => view! { <LoadingState text="正在加载智能体…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            lead="无法加载智能体"
                            error=e
                            retry=Callback::new(move |_| rev.update(|v| *v += 1))
                        />
                        <a href="/agents">"返回智能体列表"</a>
                    }
                        .into_any()
                }
                Some(Ok(a)) => {
                    let a = StoredValue::new(a);
                    let closed = archived(&a.get_value());
                    view! {
                        <PageHead
                            parent=("智能体", "/agents")
                            title=text(&a.get_value(), "name")
                        >
                            <AgentActions agent=a.get_value() rev blocked=dirty />
                        </PageHead>
                        {closed
                            .then(|| {
                                view! {
                                    <div class="card notice">
                                        "已归档，恢复后可以再次选择这个智能体。"
                                    </div>
                                }
                            })}
                        <div class="card pad agent-summary">
                            <span class="agent-avatar">
                                {if text(&a.get_value(), "avatar").is_empty() {
                                    "🤖".into()
                                } else {
                                    text(&a.get_value(), "avatar")
                                }}
                            </span>
                            <div>
                                <p class="muted">{text(&a.get_value(), "description")}</p>
                                <span class="muted">
                                    {format!(
                                        "{} · {} · {} 次运行中",
                                        text(&a.get_value(), "runtime_label"),
                                        if text(&a.get_value(), "model").is_empty() {
                                            "默认模型".into()
                                        } else {
                                            text(&a.get_value(), "model")
                                        },
                                        n(&a.get_value(), "running"),
                                    )}
                                </span>
                            </div>
                        </div>
                        <div class="seg agent-tabs" role="group" aria-label="智能体内容">
                            {[
                                ("overview", "概览"),
                                ("work", "工作记录"),
                                ("instructions", "指令与开场建议"),
                                ("skills", "技能"),
                                ("mcp", "MCP"),
                                ("settings", "设置"),
                            ]
                                .into_iter()
                                .map(|(key, label)| {
                                    view! {
                                        <button
                                            type="button"
                                            aria-pressed=move || tab.get() == key
                                            data-on=move || (tab.get() == key).to_string()
                                            on:click=move |_| {
                                                if tab.get_untracked() == key {
                                                    return;
                                                }
                                                spawn_local(async move {
                                                    if !dirty.get_untracked()
                                                        || confirm(
                                                                "放弃未保存的修改？",
                                                                "当前智能体尚未保存。",
                                                            )
                                                            .await
                                                    {
                                                        dirty.set(false);
                                                        tab.set(key.into());
                                                        let view = if matches!(
                                                            key,
                                                            "instructions" | "skills" | "mcp"
                                                        ) {
                                                            format!("capabilities&tab={key}")
                                                        } else {
                                                            key.to_owned()
                                                        };
                                                        let id = params
                                                            .with_untracked(|p| p.get("id"))
                                                            .unwrap_or_default();
                                                        if !crate::files_js::confirm_leave().await {
                                                            return;
                                                        }
                                                        navigate
                                                            .with_value(|go| go(
                                                                &format!("/agent/{}?view={view}", api::enc(&id)),
                                                                Default::default(),
                                                            ));
                                                    }
                                                });
                                            }
                                        >
                                            {label}
                                        </button>
                                    }
                                })
                                .collect_view()}
                        </div>
                        {move || match tab.get().as_str() {
                            "overview" => view! { <AgentOverview agent=a.get_value() /> }.into_any(),
                            "work" => {
                                view! { <AgentRuns runs=list(&a.get_value(), "runs") /> }.into_any()
                            }
                            "skills" => {
                                view! { <AgentCapabilities agent=a.get_value() kind="skills" /> }
                                    .into_any()
                            }
                            "mcp" => {
                                view! { <AgentCapabilities agent=a.get_value() kind="mcp" /> }
                                    .into_any()
                            }
                            _ => {
                                view! {
                                    <AgentEditor
                                        initial=a.get_value()
                                        create=false
                                        dirty
                                        section=if tab.get() == "instructions" {
                                            "instructions"
                                        } else {
                                            "settings"
                                        }
                                        on_saved=Callback::new(move |_| rev.update(|r| *r += 1))
                                    />
                                }
                                    .into_any()
                            }
                        }}
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn AgentActions(agent: Value, rev: RwSignal<u32>, blocked: RwSignal<bool>) -> impl IntoView {
    let a = StoredValue::new(agent);
    let busy = RwSignal::new(false);
    let navigate = use_navigate();
    let nav = StoredValue::new(navigate);
    let app = use_app();
    let pick = RwSignal::new(false);
    let runtimes = LocalResource::new(|| api::get::<api::Runtimes>("/api/runtimes"));
    let act = move |action: &'static str| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let agent = a.get_value();
            let id = text(&agent, "id");
            if matches!(action, "archive" | "delete") {
                if crate::files_js::confirm_leave().await {
                    nav.with_value(|n| n("/agents", Default::default()));
                    remove_profile(&agent, action == "delete", || {});
                }
                busy.set(false);
                return;
            }
            if action == "cancel-runs"
                && !confirm(
                    "取消全部运行？",
                    "正在运行的任务将被取消，最多需要 5 秒停止。",
                )
                .await
            {
                busy.set(false);
                return;
            }
            match api::send::<Value>(
                "POST",
                &format!("/api/agent-profiles/{}/{action}", api::enc(&id)),
                &json!({}),
            )
            .await
            {
                Ok(_) => {
                    toast(if action == "restore" {
                        "已恢复"
                    } else {
                        "已取消全部运行"
                    });
                    rev.update(|r| *r += 1);
                }
                Err(e) => toast_error("没处理成", &e),
            }
            busy.set(false);
        });
    };
    view! {
        <div class="agent-actions">
            {if archived(&a.get_value()) {
                view! {
                    <button
                        class="btn"
                        disabled=move || busy.get() || blocked.get()
                        on:click=move |_| act("restore")
                    >
                        "恢复"
                    </button>
                    <MoreMenu
                        label="更多操作".to_owned()
                        items=vec![MenuItem::danger("删除", move || act("delete"))]
                        disabled=Signal::derive(move || busy.get() || blocked.get())
                    />
                }
                    .into_any()
            } else {
                let copy = format!(
                    "/agents/new?template={}",
                    api::enc(&text(&a.get_value(), "id")),
                );
                let mut items = vec![
                    MenuItem::new(
                        "复制",
                        move || {
                            let _ = window().location().set_href(&copy);
                        },
                    ),
                ];
                if n(&a.get_value(), "running") > 0 {
                    items.push(MenuItem::new("取消全部运行", move || act("cancel-runs")));
                }
                items.push(MenuItem::danger("归档", move || act("archive")));
                view! {
                    <button
                        class="btn primary"
                        disabled=move || busy.get() || blocked.get()
                        on:click=move |_| {
                            match runtimes.get_untracked() {
                                Some(
                                    Ok(r),
                                ) if availability(&a.get_value(), &r.runtimes) != "unbound" => {
                                    pick.set(true)
                                }
                                Some(Err(e)) => toast_error("无法检查 Claude Code / Codex", &e),
                                Some(Ok(_)) => {
                                    toast(
                                        "先装好 Claude Code 或 Codex，再到「账号与额度」里登录",
                                    )
                                }
                                None => toast("正在检查 Claude Code / Codex，请稍后再试"),
                            }
                        }
                    >
                        "开始对话"
                    </button>
                    <MoreMenu
                        label="更多操作".to_owned()
                        items
                        disabled=Signal::derive(move || busy.get() || blocked.get())
                    />
                }
                    .into_any()
            }}
            {move || {
                pick.get()
                    .then(|| {
                        view! {
                            <Modal
                                label="选择工作区"
                                class="dlg agent-modal"
                                on_close=Callback::new(move |_| pick.set(false))
                            >
                                <h3>"在哪个工作区对话？"</h3>
                                {if app.top_workspaces().is_empty() {
                                    view! {
                                        <EmptyState
                                            title="先创建工作区"
                                            detail="选择项目后，即可和智能体开始对话。"
                                        />
                                        <a class="btn primary" href="/workspaces">
                                            "打开工作区列表"
                                        </a>
                                    }
                                        .into_any()
                                } else {
                                    app.top_workspaces()
                                        .into_iter()
                                        .map(|w| {
                                            view! {
                                                <button
                                                    class="agent-pick"
                                                    on:click=move |_| {
                                                        let href = format!(
                                                            "/w/{}?agent={}&new=1",
                                                            api::enc(&w.id),
                                                            api::enc(&text(&a.get_value(), "id")),
                                                        );
                                                        leptos::task::spawn_local(async move {
                                                            if !crate::files_js::confirm_leave().await {
                                                                return;
                                                            }
                                                            pick.set(false);
                                                            nav.with_value(|n| n(&href, Default::default()));
                                                        });
                                                    }
                                                >
                                                    <b>{crate::fmt::ws_title(&w.name, &w.node)}</b>
                                                    <span class="ws-machine" title=w.path>
                                                        {crate::fmt::machine(&w.node)}
                                                    </span>
                                                </button>
                                            }
                                        })
                                        .collect_view()
                                        .into_any()
                                }}
                                <button class="btn" on:click=move |_| pick.set(false)>
                                    "取消"
                                </button>
                            </Modal>
                        }
                    })
            }}
        </div>
    }
}

#[component]
fn AgentOverview(agent: Value) -> impl IntoView {
    let total = n(&agent, "succeeded_30d") + n(&agent, "failed_30d");
    view! {
        <div class="agent-overview">
            <div class="card pad">
                <h3>"近 30 天"</h3>
                <div class="agent-stats">
                    <div>
                        <b>{n(&agent, "runs_30d")}</b>
                        <span>"次运行"</span>
                    </div>
                    <div>
                        <b>
                            {if total == 0 {
                                "—".into()
                            } else {
                                format!("{}%", n(&agent, "succeeded_30d") * 100 / total)
                            }}
                        </b>
                        <span>"成功率"</span>
                    </div>
                    <div>
                        <b>
                            {crate::vocab::duration(
                                agent["avg_secs_30d"].as_f64().unwrap_or_default(),
                            )}
                        </b>
                        <span>"平均耗时"</span>
                    </div>
                    <div>
                        <b>{format!("${:.2}", agent["cost_30d"].as_f64().unwrap_or_default())}</b>
                        <span>"费用"</span>
                    </div>
                </div>
                <p class="muted">
                    {format!(
                        "{} 次失败 · {} 次取消",
                        n(&agent, "failed_30d"),
                        n(&agent, "cancelled_30d"),
                    )}
                </p>
                <dl class="agent-facts">
                    <dt>"用的软件"</dt>
                    <dd>{text(&agent, "runtime_label")}</dd>
                    <dt>"并发"</dt>
                    <dd>{n(&agent, "max_concurrent")}</dd>
                    <dt>"思考"</dt>
                    <dd>{text(&agent, "thinking_level")}</dd>
                    <dt>"创建"</dt>
                    <dd title=text(
                        &agent,
                        "created_at",
                    )>{fmt::ago(&text(&agent, "created_at"))}</dd>
                    <dt>"更新"</dt>
                    <dd title=text(
                        &agent,
                        "updated_at",
                    )>{fmt::ago(&text(&agent, "updated_at"))}</dd>
                </dl>
            </div>
        </div>
        <AgentRuns runs=list(&agent, "runs") />
    }
}
#[component]
fn AgentRuns(runs: Vec<Value>) -> impl IntoView {
    let runs = StoredValue::new(runs);
    let filter = RwSignal::new("all".to_owned());
    let nav = use_navigate();
    let nav = StoredValue::new(nav);
    let has_runs = !runs.with_value(Vec::is_empty);
    view! {
        <div class="card pad">
            <div class="agent-toolbar">
                <h3>"运行记录"</h3>
                <select
                    class="filter"
                    hidden=!has_runs
                    aria-label="运行状态"
                    prop:value=move || filter.get()
                    on:change=move |e| filter.set(event_target_value(&e))
                >
                    {[
                        ("all", "全部"),
                        ("running", "运行中"),
                        ("done", "已完成"),
                        ("failed", "失败"),
                        ("interrupted", "已中断"),
                    ]
                        .into_iter()
                        .map(|(v, l)| {
                            view! {
                                <option value=v prop:selected=move || filter.get() == v>
                                    {format!("状态：{l}")}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </div>
            {move || {
                let rows: Vec<_> = runs
                    .get_value()
                    .into_iter()
                    .filter(|r| filter.get() == "all" || text(r, "status") == filter.get())
                    .collect();
                if rows.is_empty() {
                    return if has_runs {
                        view! {
                            <Blank
                                icon=icon::FILTER
                                title=empty::unmatched("运行记录")
                                detail="换个状态看看。"
                                action=Action::clear(move || filter.set("all".into()))
                                inset=true
                            />
                        }
                            .into_any()
                    } else {
                        view! {
                            <Blank
                                icon=icon::ACTIVITY
                                title=empty::never("运行记录")
                                detail="用这个智能体开始对话或派活后，每次运行都会记在这里。"
                                inset=true
                            />
                        }
                            .into_any()
                    };
                }
                rows.into_iter()
                    .map(|r| {
                        let row = StoredValue::new(r.clone());
                        view! {
                            <button
                                class="agent-run"
                                disabled=r["workspace"].is_null()
                                on:click=move |_| {
                                    let r = row.get_value();
                                    let href = format!(
                                        "/w/{}?thread={}",
                                        api::enc(&text(&r, "workspace_id")),
                                        api::enc(&text(&r, "thread")),
                                    );
                                    leptos::task::spawn_local(async move {
                                        if !crate::files_js::confirm_leave().await {
                                            return;
                                        }
                                        nav.with_value(|n| n(&href, Default::default()));
                                    });
                                }
                            >
                                <StateOf state=text(&r, "status") />
                                <span class="grow">
                                    {if text(&r, "title").is_empty() {
                                        "对话".into()
                                    } else {
                                        text(&r, "title")
                                    }}
                                    <span class="muted">
                                        {format!(
                                            " · {}",
                                            if text(&r, "workspace").is_empty() {
                                                "工作区已删除".into()
                                            } else {
                                                text(&r, "workspace")
                                            },
                                        )}
                                    </span>
                                </span>
                                <span class="muted">
                                    {format!(
                                        "{} · {} 条",
                                        fmt::ago(&text(&r, "created_at")),
                                        n(&r, "events"),
                                    )}
                                </span>
                            </button>
                        }
                    })
                    .collect_view()
                    .into_any()
            }}
        </div>
    }
}

#[component]
fn AgentCapabilities(agent: Value, kind: &'static str) -> impl IntoView {
    let id = StoredValue::new(text(&agent, "id"));
    let closed = archived(&agent);
    let rev = RwSignal::new(0u32);
    let busy = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.track();
        async move {
            let lib = api::get::<Vec<Value>>(if kind == "skills" {
                "/api/skills"
            } else {
                "/api/mcp-servers"
            })
            .await?;
            let caps = api::get::<Value>(&format!(
                "/api/agent-profiles/{}/capabilities",
                api::enc(&id.get_value())
            ))
            .await?;
            Ok::<_, api::ApiError>((lib, caps))
        }
    });
    view! {
        <div class="card pad">
            <div class="agent-toolbar">
                <h3>{if kind == "skills" { "技能" } else { "MCP 服务器" }}</h3>
                <a
                    class="linkbtn"
                    href=if kind == "skills" { "/skills" } else { "/skills?tab=mcp" }
                >
                    "管理能力库 →"
                </a>
            </div>
            <p class="muted">
                {if kind == "skills" {
                    "勾选的技能在下一轮生效，不会改动 Claude Code / Codex 自己的技能目录。"
                } else if matches!(text(&agent, "runtime").as_str(), "claude" | "codex") {
                    "勾选的 MCP 服务器在下一轮启动时合并进配置。"
                } else {
                    "只有 Claude Code 与 Codex 支持加载 MCP 服务器。"
                }}
            </p>
            {move || match data.get() {
                None => view! { <LoadingState text="正在加载能力…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            lead="无法加载能力"
                            error=e
                            retry=Callback::new(move |_| rev.update(|v| *v += 1))
                        />
                    }
                        .into_any()
                }
                Some(Ok((lib, caps))) => {
                    if lib.is_empty() {
                        return view! {
                            <EmptyState
                                title="还没有可选能力"
                                detail="先在能力库添加技能或 MCP 服务器。"
                            />
                        }
                            .into_any();
                    }
                    let enabled = StoredValue::new(list(&caps, kind));
                    lib.into_iter()
                        .map(|v| {
                            let cid = text(&v, "id");
                            let checked = enabled.with_value(|e| e.contains(&json!(cid)));
                            view! {
                                <label class="agent-cap">
                                    <input
                                        type="checkbox"
                                        prop:checked=checked
                                        disabled=move || busy.get() || closed
                                        on:change=move |e| {
                                            let checked = event_target_checked(&e);
                                            let mut ids = enabled.get_value();
                                            ids.retain(|v| v.as_str() != Some(&cid));
                                            if checked {
                                                ids.push(json!(cid));
                                            }
                                            busy.set(true);
                                            spawn_local(async move {
                                                let mut body = json!({});
                                                body[kind] = json!(ids);
                                                match api::send::<
                                                    Value,
                                                >(
                                                        "PUT",
                                                        &format!(
                                                            "/api/agent-profiles/{}/capabilities",
                                                            api::enc(&id.get_value()),
                                                        ),
                                                        &body,
                                                    )
                                                    .await
                                                {
                                                    Ok(_) => toast("已保存，下一轮生效"),
                                                    Err(e) => toast_error("没保存上", &e),
                                                }
                                                busy.set(false);
                                                rev.update(|r| *r += 1);
                                            });
                                        }
                                    />
                                    <span>
                                        <b>{text(&v, "name")}</b>
                                        <small class="muted">{text(&v, "description")}</small>
                                    </span>
                                </label>
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
fn AgentEditor(
    initial: Value,
    create: bool,
    #[prop(optional)] copied: bool,
    on_saved: Callback<Value>,
    #[prop(optional)] dirty: Option<RwSignal<bool>>,
    #[prop(default = "all")] section: &'static str,
) -> impl IntoView {
    let dirty = dirty.unwrap_or_else(|| RwSignal::new(false));
    let original = RwSignal::new(initial.clone());
    let draft = RwSignal::new(initial);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let closed = archived(&draft.get_untracked());
    let navigate = use_navigate();
    let args = RwSignal::new(
        list(&draft.get_untracked(), "custom_args")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let env = RwSignal::new(
        draft.get_untracked()["env"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
    );
    let saved_args = RwSignal::new(args.get_untracked());
    let saved_env = RwSignal::new(env.get_untracked());
    Effect::new(move |_| {
        dirty.set(
            draft.get() != original.get()
                || args.get() != saved_args.get()
                || env.get() != saved_env.get(),
        )
    });
    guard_unsaved(dirty);
    on_cleanup(move || {
        let _ = dirty.try_set(false);
    });
    let starter_count = Memo::new(move |_| list(&draft.get(), "starters").len());
    let runtime = Memo::new(move |_| text(&draft.get(), "runtime"));
    let rts = LocalResource::new(|| api::get::<api::Runtimes>("/api/runtimes"));
    let accounts = LocalResource::new(|| api::get::<api::Accounts>("/api/accounts"));
    Effect::new(move |_| {
        if create && let Some(Ok(r)) = rts.get() {
            let current = text(&draft.get_untracked(), "runtime");
            if !r.runtimes.iter().any(|r| r.installed && r.id == current)
                && let Some(r) = r.runtimes.iter().find(|r| r.installed)
            {
                let pick = |v: &mut Value| {
                    v["runtime"] = json!(r.id);
                    for key in ["model", "thinking_level", "permission_mode", "account"] {
                        v[key] = Value::Null;
                    }
                };
                draft.update(pick);
                if current.is_empty() {
                    original.update(pick);
                } else {
                    toast("原来用的软件没装，已改用可用的 Claude Code / Codex，并重置模型与权限。");
                }
            }
        }
    });
    let models = LocalResource::new(move || {
        let rt = runtime.get();
        async move {
            if rt.is_empty() {
                return Ok::<Value, api::ApiError>(json!({ "models": [] }));
            }
            api::get::<Value>(&format!("/api/runtimes/{}/models", api::enc(&rt))).await
        }
    });
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        let mut body = draft.get_untracked();
        if env.get_untracked() != saved_env.get_untracked() {
            match super::skills::parse_kv(&env.get_untracked()) {
                Ok(v) => body["env"] = v,
                Err(e) => {
                    error.set(e);
                    return;
                }
            }
        }
        body["custom_args"] = json!(
            args.get_untracked()
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
        if text(&body, "name").trim().is_empty() {
            error.set("名称必填".into());
            return;
        }
        if text(&body, "runtime").is_empty() {
            error.set("先装好 Claude Code 或 Codex".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let navigate = navigate.clone();
        spawn_local(async move {
            let path = if create {
                "/api/agent-profiles".into()
            } else {
                format!("/api/agent-profiles/{}", api::enc(&text(&body, "id")))
            };
            match api::send::<Value>(if create { "POST" } else { "PUT" }, &path, &body).await {
                Ok(v) => {
                    toast("已保存智能体");
                    if create {
                        dirty.set(false);
                        if !crate::files_js::confirm_leave().await {
                            busy.set(false);
                            return;
                        }
                        navigate(
                            &format!("/agent/{}", api::enc(&text(&v, "id"))),
                            Default::default(),
                        );
                    } else {
                        draft.set(v.clone());
                        original.set(v.clone());
                        if let Some(current) = args.try_get_untracked() {
                            saved_args.set(current);
                        }
                        if let Some(current) = env.try_get_untracked() {
                            saved_env.set(current);
                        }
                        dirty.set(false);
                        on_saved.run(v);
                    }
                }
                Err(e) => error.set(crate::vocab::lead("没保存上", e.explained()).0),
            }
            busy.set(false);
        });
    };
    view! {
        <div class="agent-editor">
            {copied
                .then(|| {
                    view! {
                        <p class="muted">
                            "复制时会保留指令、开场建议和参数；环境变量需要重新填写。"
                        </p>
                    }
                })}
            {move || {
                rts.get()
                    .and_then(Result::err)
                    .map(|e| {
                        view! {
                            <InlineError
                                lead="无法检查 Claude Code / Codex"
                                error=e
                                retry=Callback::new(move |_| rts.refetch())
                            />
                        }
                    })
            }}
            {move || {
                accounts
                    .get()
                    .and_then(Result::err)
                    .map(|e| {
                        view! {
                            <InlineError
                                lead="无法加载账号"
                                error=e
                                retry=Callback::new(move |_| accounts.refetch())
                            />
                        }
                    })
            }} <fieldset disabled=move || busy.get() || closed>
                <div class="card pad" hidden=section == "instructions">
                    <h3>"身份"</h3>
                    <AgentField draft key="avatar" label="头像" />
                    <AgentField draft key="name" label="名称（1–40 字）" />
                    <AgentField
                        draft
                        key="description"
                        label="描述（最多 255 字）"
                        multiline=true
                    />
                </div>
                <div class="card pad" hidden=section == "settings">
                    <h3>"指令与开场建议"</h3>
                    <AgentField
                        draft
                        key="instructions"
                        label="工作指令（最多 8000 字）"
                        multiline=true
                    />
                    {move || {
                        (0..starter_count.get())
                            .map(|i| {
                                view! {
                                    <div class="agent-starter">
                                        <label>
                                            "标题"
                                            <input
                                                aria-label=format!("开场建议 {} 的标题", i + 1)
                                                prop:value=move || {
                                                    draft.with(|d| text(&d["starters"][i], "label"))
                                                }
                                                maxlength="80"
                                                on:input=move |e| {
                                                    draft
                                                        .update(|d| {
                                                            d["starters"][i]["label"] = json!(event_target_value(&e));
                                                        })
                                                }
                                            />
                                        </label>
                                        <label>
                                            "提示词"
                                            <textarea
                                                aria-label=format!("开场建议 {} 的提示词", i + 1)
                                                rows="3"
                                                maxlength="4000"
                                                prop:value=move || {
                                                    draft.with(|d| text(&d["starters"][i], "prompt"))
                                                }
                                                on:input=move |e| {
                                                    draft
                                                        .update(|d| {
                                                            d["starters"][i]["prompt"] = json!(event_target_value(&e));
                                                        })
                                                }
                                            />
                                        </label>
                                        <button
                                            class="btn"
                                            aria-label=format!("移除开场建议 {}", i + 1)
                                            on:click=move |_| {
                                                draft
                                                    .update(|d| {
                                                        if let Some(s) = d["starters"].as_array_mut() {
                                                            s.remove(i);
                                                        }
                                                    })
                                            }
                                        >
                                            "移除"
                                        </button>
                                    </div>
                                }
                            })
                            .collect_view()
                    }}
                    <button
                        class="btn"
                        disabled=move || { list(&draft.get(), "starters").len() >= 3 }
                        on:click=move |_| {
                            draft
                                .update(|d| {
                                    let mut starters = list(d, "starters");
                                    starters.push(json!({"label":"","prompt":""}));
                                    d["starters"] = json!(starters);
                                })
                        }
                    >
                        "添加开场建议（最多 3 条）"
                    </button>
                </div>
                <div class="card pad" hidden=section == "instructions">
                    <h3>"执行配置"</h3>
                    <label class="field">
                        "Claude Code / Codex"
                        <select
                            prop:value=move || text(&draft.get(), "runtime")
                            on:change=move |e| {
                                draft
                                    .update(|d| {
                                        d["runtime"] = json!(event_target_value(&e));
                                        for k in [
                                            "model",
                                            "thinking_level",
                                            "permission_mode",
                                            "account",
                                        ] {
                                            d[k] = Value::Null;
                                        }
                                    })
                            }
                        >
                            <option value="" prop:selected=move || runtime.get().is_empty()>
                                "选择 Claude Code 或 Codex"
                            </option>
                            {move || {
                                rts.get()
                                    .and_then(Result::ok)
                                    .map(|r| {
                                        r.runtimes
                                            .into_iter()
                                            .map(|r| {
                                                let id = r.id.clone();
                                                view! {
                                                    <option
                                                        value=r.id
                                                        prop:selected=move || runtime.get() == id
                                                    >
                                                        {format!(
                                                            "{}{}",
                                                            r.label,
                                                            if r.installed { "" } else { "（未安装）" },
                                                        )}
                                                    </option>
                                                }
                                            })
                                            .collect_view()
                                    })
                            }}
                        </select>
                    </label>
                    <label class="field">
                        "账号"
                        <select
                            prop:value=move || text(&draft.get(), "account")
                            on:change=move |e| {
                                draft
                                    .update(|d| {
                                        let v = event_target_value(&e);
                                        d["account"] = if v.is_empty() {
                                            Value::Null
                                        } else {
                                            json!(v)
                                        };
                                    })
                            }
                        >
                            <option
                                value=""
                                prop:selected=move || text(&draft.get(), "account").is_empty()
                            >
                                "跟着「账号与额度」里选的"
                            </option>
                            <option
                                value="auto"
                                prop:selected=move || text(&draft.get(), "account") == "auto"
                            >
                                "自动选择"
                            </option>
                            {move || {
                                accounts
                                    .get()
                                    .and_then(Result::ok)
                                    .map(|a| {
                                        a.accounts
                                            .into_iter()
                                            .filter(|a| a.provider == runtime.get())
                                            .map(|a| {
                                                let id = a.id.clone();
                                                view! {
                                                    <option
                                                        value=a.id
                                                        prop:selected=move || text(&draft.get(), "account") == id
                                                    >
                                                        {format!(
                                                            "{}{}",
                                                            a.label,
                                                            if a.disabled { "（已停用）" } else { "" },
                                                        )}
                                                    </option>
                                                }
                                            })
                                            .collect_view()
                                    })
                            }}
                        </select>
                    </label>
                    <label class="field">
                        "模型（留空使用默认值）"
                        <input
                            list="agent-model-options"
                            prop:value=move || text(&draft.get(), "model")
                            on:input=move |e| {
                                draft.update(|d| d["model"] = json!(event_target_value(&e)))
                            }
                        />
                        <datalist id="agent-model-options">
                            {move || {
                                models
                                    .get()
                                    .and_then(Result::ok)
                                    .map(|v| {
                                        list(&v, "models")
                                            .into_iter()
                                            .map(|m| {
                                                view! {
                                                    <option value=text(&m, "id")>{text(&m, "label")}</option>
                                                }
                                            })
                                            .collect_view()
                                    })
                            }}
                        </datalist>
                    </label>
                    <label class="field">
                        "思考等级"
                        <select
                            prop:value=move || text(&draft.get(), "thinking_level")
                            on:change=move |e| {
                                draft
                                    .update(|d| d["thinking_level"] = json!(event_target_value(&e)))
                            }
                        >
                            <option
                                value=""
                                prop:selected=move || {
                                    text(&draft.get(), "thinking_level").is_empty()
                                }
                            >
                                "跟随 CLI 配置"
                            </option>
                            {["minimal", "low", "medium", "high", "xhigh", "max", "ultra"]
                                .into_iter()
                                .map(|s| {
                                    view! {
                                        <option
                                            value=s
                                            prop:selected=move || {
                                                text(&draft.get(), "thinking_level") == s
                                            }
                                        >
                                            {s}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </label>
                    <label class="field">
                        "权限模式"
                        <select
                            data-ui-guard=""
                            prop:value=move || text(&draft.get(), "permission_mode")
                            on:change=move |e| {
                                draft
                                    .update(|d| {
                                        d["permission_mode"] = json!(event_target_value(&e));
                                    })
                            }
                        >
                            <option
                                value=""
                                prop:selected=move || {
                                    text(&draft.get(), "permission_mode").is_empty()
                                }
                            >
                                "按软件自己的设置"
                            </option>
                            {move || {
                                let modes: Vec<&str> = match runtime.get().as_str() {
                                    "codex" => {
                                        vec!["read-only", "workspace-write", "danger-full-access"]
                                    }
                                    "claude" | "grok" => {
                                        vec![
                                            "default",
                                            "acceptEdits",
                                            "auto",
                                            "plan",
                                            "bypassPermissions",
                                        ]
                                    }
                                    _ => vec![],
                                };
                                modes
                                    .into_iter()
                                    .map(|m| {
                                        view! {
                                            <option
                                                value=m
                                                prop:selected=move || {
                                                    text(&draft.get(), "permission_mode") == m
                                                }
                                            >
                                                {m}
                                            </option>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </select>
                    </label>
                    <label class="field">
                        "最大并发（1–50）"
                        <input
                            type="number"
                            min="1"
                            max="50"
                            prop:value=move || n(&draft.get(), "max_concurrent").max(1).to_string()
                            on:change=move |e| {
                                draft
                                    .update(|d| {
                                        d["max_concurrent"] = json!(
                                            event_target_value(&e).parse::<u64>().unwrap_or(1).clamp(1,50)
                                        );
                                    })
                            }
                        />
                    </label>
                </div>
                <div class="card pad" hidden=section == "instructions">
                    <h3>"环境变量"</h3>
                    <details>
                        <summary>"查看或编辑环境变量（显示明文）"</summary>
                        <p class="muted">"每行 KEY=value。"</p>
                        <textarea
                            class="mono"
                            rows="5"
                            aria-label="环境变量"
                            prop:value=move || env.get()
                            on:input=move |e| env.set(event_target_value(&e))
                        />
                    </details>
                </div>
                <details class="card pad agent-runtime-details" hidden=section == "instructions">
                    <summary>"自定义启动参数"</summary>
                    <p class="muted">"每行是一个完整参数，按顺序传入 CLI。"</p>
                    <textarea
                        class="mono"
                        rows="4"
                        aria-label="自定义参数"
                        prop:value=move || args.get()
                        on:input=move |e| args.set(event_target_value(&e))
                    />
                    <pre class="agent-command">
                        {move || {
                            format!(
                                "{} {}",
                                text(&draft.get(), "runtime"),
                                args
                                    .get()
                                    .lines()
                                    .map(|s| {
                                        if s.contains(' ') {
                                            format!("{s:?}")
                                        } else {
                                            s.to_owned()
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            )
                        }}
                    </pre>
                </details>
            </fieldset>
            {move || {
                (!error.get().is_empty())
                    .then(|| view! { <InlineError message=error.get() plain=true /> })
            }}
            {(!closed)
                .then(|| {
                    view! {
                        <div class="agent-save">
                            <span class="muted">
                                {move || if dirty.get() { "有未保存的修改" } else { "" }}
                            </span>
                            <button class="btn primary" disabled=move || busy.get() on:click=save>
                                {move || {
                                    if busy.get() {
                                        "保存中…"
                                    } else if create {
                                        "创建并打开"
                                    } else {
                                        "保存修改"
                                    }
                                }}
                            </button>
                        </div>
                    }
                })}
        </div>
    }
}

#[component]
fn AgentField(
    draft: RwSignal<Value>,
    key: &'static str,
    label: &'static str,
    #[prop(default = false)] multiline: bool,
) -> impl IntoView {
    view! {
        <label class="field">
            {label}
            {if multiline {
                view! {
                    <textarea
                        rows=if key == "instructions" { "9" } else { "3" }
                        prop:value=move || text(&draft.get(), key)
                        on:input=move |e| draft.update(|v| v[key] = json!(event_target_value(&e)))
                    />
                }
                    .into_any()
            } else {
                view! {
                    <input
                        prop:value=move || text(&draft.get(), key)
                        on:input=move |e| draft.update(|v| v[key] = json!(event_target_value(&e)))
                    />
                }
                    .into_any()
            }}
        </label>
    }
}
