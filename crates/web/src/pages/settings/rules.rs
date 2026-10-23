use leptos::prelude::*;
use serde_json::{Value, json};

use crate::components::prefs::{
    PrefAdvanced, PrefEmpty, PrefGroup, PrefItem, PrefList, Sheet, Switch,
};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::components::undo::{self, Request};
use crate::{api, app_state::use_app};

use super::{flag, rows, save, text};

#[component]
pub(super) fn RuleSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let selected = RwSignal::new(None::<String>);
    let adding = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/approval-rules")
    });
    let list = RwSignal::new(Vec::<Value>::new());
    let remove = Callback::new(move |id: String| {
        let name = list.with_untracked(|l| {
            l.iter()
                .find(|r| text(r, "id") == id)
                .map(|r| format!("{} {}", text(r, "tool"), text(r, "pattern")))
                .unwrap_or_default()
        });
        selected.set(None);
        undo::remove_row(
            format!("已删除规则「{}」", name.trim()),
            id.clone(),
            Request::delete(format!("/api/approval-rules/{}", api::enc(&id))),
            move || rev.update(|n| *n += 1),
        );
    });
    view! {
        <PrefGroup
            title="自动批准"
            note="匹配的操作无需逐次批准，仍会记录在对话中。"
            guard=true
        >
            <PrefList
                label="自动批准规则"
                selected
                on_add=Callback::new(move |()| adding.set(true))
                on_remove=remove
                busy=busy
            >
                {move || match data.get() {
                    None => view! { <LoadingState text="正在加载设置…" /> }.into_any(),
                    Some(Err(e)) => {
                        view! {
                            <InlineError
                                message=format!("无法加载设置：{e}")
                                retry=Callback::new(move |_| data.refetch())
                            />
                        }
                            .into_any()
                    }
                    Some(Ok(v)) => {
                        let items = rows(&v);
                        list.set(items.clone());
                        let items: Vec<Value> = items
                            .into_iter()
                            .filter(|r| undo::shows(&text(r, "id")))
                            .collect();
                        if items.is_empty() {
                            return view! {
                                <PrefEmpty text="还没有自动批准规则。按「+」填写可信任的工具和适用范围。" />
                            }
                                .into_any();
                        }
                        items
                            .into_iter()
                            .map(move |r| {
                                let id = text(&r, "id");
                                let enabled = RwSignal::new(flag(&r, "enabled"));
                                let scope = r["workspace_name"]
                                    .as_str()
                                    .unwrap_or(
                                        if r["workspace_id"].is_null() {
                                            "全局"
                                        } else {
                                            "工作区已删除"
                                        },
                                    )
                                    .to_owned();
                                let switch_id = id.clone();
                                view! {
                                    <PrefItem
                                        id=id
                                        selected
                                        title=format!("{} {}", text(&r, "tool"), text(&r, "pattern"))
                                        code=true
                                        detail=format!("{scope} · 命中 {} 次", r["hits"])
                                        trailing=move || {
                                            let id = switch_id.clone();
                                            view! {
                                                <Switch
                                                    label="启用规则"
                                                    checked=enabled
                                                    disabled=busy
                                                    on_change=move |v: bool| {
                                                        let id = id.clone();
                                                        enabled.set(v);
                                                        busy.set(true);
                                                        leptos::task::spawn_local(async move {
                                                            if !save(
                                                                    "PUT",
                                                                    &format!("/api/approval-rules/{}", api::enc(&id)),
                                                                    json!({"enabled":v}),
                                                                )
                                                                .await
                                                            {
                                                                enabled.set(!v);
                                                            }
                                                            busy.set(false);
                                                        });
                                                    }
                                                />
                                            }
                                        }
                                    />
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </PrefList>
            <PrefAdvanced title="规则怎样匹配">
                <p>"默认没有规则；含 shell 控制符的命令不会自动批准。"</p>
                <p>"工具名可以写 Read、Edit、Bash，或 mcp__x__* 这样带通配的 MCP 工具名；路径或命令前缀留空时匹配该工具的全部调用。"</p>
            </PrefAdvanced>
        </PrefGroup>
        <Show when=move || adding.get()>
            <RuleSheet
                on_close=Callback::new(move |()| adding.set(false))
                on_saved=Callback::new(move |()| {
                    adding.set(false);
                    rev.update(|n| *n += 1);
                })
            />
        </Show>
    }
}

#[component]
fn RuleSheet(on_close: Callback<()>, on_saved: Callback<()>) -> impl IntoView {
    let app = use_app();
    let tool = RwSignal::new(String::new());
    let pattern = RwSignal::new(String::new());
    let ws = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let empty = move || tool.with(|t| t.trim().is_empty());
    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        if empty() {
            toast("请填写工具名称");
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            let workspace = ws.get_untracked();
            if save(
                "POST",
                "/api/approval-rules",
                json!({
                    "tool": tool.get_untracked().trim(),
                    "pattern": pattern.get_untracked().trim(),
                    "workspace_id": if workspace.is_empty() { None } else { Some(workspace) },
                }),
            )
            .await
            {
                on_saved.run(());
            }
            busy.set(false);
        });
    };
    view! {
        <Sheet
            title="新建自动批准规则"
            on_close=Callback::new(move |()| {
                if !busy.get_untracked() {
                    on_close.run(());
                }
            })
            foot=move || {
                view! {
                    <button
                        type="button"
                        class="btn"
                        disabled=move || busy.get()
                        on:click=move |_| on_close.run(())
                    >
                        "取消"
                    </button>
                    <button
                        type="button"
                        class="btn primary"
                        disabled=move || busy.get() || empty()
                        on:click=move |_| submit()
                    >
                        {move || if busy.get() { "保存中…" } else { "添加规则" }}
                    </button>
                }
            }
        >
            <label class="field">
                "工具名称"
                <input
                    data-modal-initial-focus=""
                    placeholder="Read / Edit / Bash / mcp__x__*"
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || tool.get()
                    on:input=move |e| tool.set(event_target_value(&e))
                />
            </label>
            <label class="field">
                "路径或命令前缀"
                <input
                    placeholder="可空"
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || pattern.get()
                    on:input=move |e| pattern.set(event_target_value(&e))
                />
            </label>
            <label class="field">
                "范围"
                <select prop:value=move || ws.get() on:change=move |e| ws.set(event_target_value(&e))>
                    <option value="" selected=move || ws.get().is_empty()>
                        "全局"
                    </option>
                    {move || {
                        app.in_tree_order()
                            .into_iter()
                            .map(|(depth, w)| {
                                let id = w.id.clone();
                                let label = crate::app_state::indented_label(depth, &w);
                                view! {
                                    <option value=w.id.clone() selected=move || ws.get() == id>
                                        {label}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
            </label>
        </Sheet>
    }
}
