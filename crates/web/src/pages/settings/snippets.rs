use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::{PrefEmpty, PrefGroup, PrefItem, PrefList, Sheet};
use crate::components::status::{InlineError, LoadingState};
use crate::components::undo::{self, Request};

use super::{rows, save, text};

#[derive(Clone)]
struct Draft {
    id: Option<String>,
    name: String,
    body: String,
}

#[component]
pub(super) fn SnippetSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let selected = RwSignal::new(None::<String>);
    let draft = RwSignal::new(None::<Draft>);
    let list = RwSignal::new(Vec::<Value>::new());
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/snippets")
    });
    let edit = Callback::new(move |id: String| {
        let found = list.with_untracked(|l| l.iter().find(|s| text(s, "id") == id).cloned());
        if let Some(s) = found {
            draft.set(Some(Draft {
                id: Some(id),
                name: text(&s, "name"),
                body: text(&s, "body"),
            }));
        }
    });
    let remove = Callback::new(move |id: String| {
        let name = list.with_untracked(|l| {
            l.iter()
                .find(|s| text(s, "id") == id)
                .map(|s| text(s, "name"))
                .unwrap_or_default()
        });
        selected.set(None);
        undo::remove_row(
            format!("已删除「{name}」"),
            id.clone(),
            Request::delete(format!("/api/snippets/{}", api::enc(&id))),
            move || {
                rev.try_update(|n| *n += 1);
            },
        );
    });
    view! {
        <PrefGroup title="常用内容" aside="在对话里输入 @ 选择">
            <PrefList
                label="片段"
                selected
                on_add=Callback::new(move |()| {
                    draft
                        .set(
                            Some(Draft {
                                id: None,
                                name: String::new(),
                                body: String::new(),
                            }),
                        )
                })
                on_remove=remove
                on_edit=edit
                busy=busy
            >
                {move || match data.get() {
                    None => view! { <LoadingState text="正在加载设置…" /> }.into_any(),
                    Some(Err(e)) => {
                        view! {
                            <InlineError
                                lead="无法加载设置"
                                error=e.clone()
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
                            .filter(|s| undo::shows(&text(s, "id")))
                            .collect();
                        if items.is_empty() {
                            return view! {
                                <PrefEmpty text="还没有片段。按「+」把验收清单、代码规范这类常用内容存下来。" />
                            }
                                .into_any();
                        }
                        items
                            .into_iter()
                            .map(|s| {
                                let preview: String = text(&s, "body").chars().take(100).collect();
                                view! {
                                    <PrefItem
                                        id=text(&s, "id")
                                        selected
                                        on_open=edit
                                        title=format!("@{}", text(&s, "name"))
                                        detail=preview
                                    />
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </PrefList>
        </PrefGroup>
        {move || {
            draft
                .get()
                .map(|d| {
                    view! {
                        <SnippetSheet
                            draft=d
                            on_close=Callback::new(move |()| draft.set(None))
                            on_saved=Callback::new(move |()| {
                                draft.set(None);
                                rev.update(|n| *n += 1);
                            })
                        />
                    }
                })
        }}
    }
}

#[component]
fn SnippetSheet(draft: Draft, on_close: Callback<()>, on_saved: Callback<()>) -> impl IntoView {
    let editing = draft.id.is_some();
    let id = StoredValue::new(draft.id);
    let name = RwSignal::new(draft.name);
    let body = RwSignal::new(draft.body);
    let busy = RwSignal::new(false);
    let incomplete =
        move || name.with(|n| n.trim().is_empty()) || body.with(|b| b.trim().is_empty());
    let submit = move || {
        if busy.get_untracked() || incomplete() {
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            let path = id
                .get_value()
                .map(|id| format!("/api/snippets/{}", api::enc(&id)))
                .unwrap_or_else(|| "/api/snippets".into());
            if save(
                if id.get_value().is_some() {
                    "PUT"
                } else {
                    "POST"
                },
                &path,
                json!({"name": name.get_untracked().trim(), "body": body.get_untracked()}),
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
            title=if editing { "编辑片段" } else { "新建片段" }
            class="dlg sheet wide"
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
                        disabled=move || busy.get() || incomplete()
                        on:click=move |_| submit()
                    >
                        {move || if busy.get() { "保存中…" } else { "保存" }}
                    </button>
                }
            }
        >
            <label class="field">
                "名称"
                <input
                    data-modal-initial-focus=""
                    maxlength="40"
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || name.get()
                    on:input=move |e| name.set(event_target_value(&e))
                />
            </label>
            <label class="field">
                "内容"
                <textarea
                    class="mono"
                    rows="10"
                    prop:value=move || body.get()
                    on:input=move |e| body.set(event_target_value(&e))
                ></textarea>
            </label>
        </Sheet>
    }
}
