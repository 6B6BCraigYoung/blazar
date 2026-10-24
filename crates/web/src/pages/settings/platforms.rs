use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::prefs::{PrefGroup, PrefItem, PrefList, PrefRow, Sheet};
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::components::undo::{self, Request};

use super::{flag, rows, save, text};

#[component]
pub(super) fn PlatformSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/skills/market/providers")
    });
    view! {
        <PrefGroup note="搜索技能时去这些来源找。密钥保存后不再显示。">
            <PrefRow label="搜索和安装技能" detail="在「智能体 → 技能」里。">
                <a class="btn" href="/skills?tab=market">
                    "打开技能库"
                </a>
            </PrefRow>
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
                Some(Ok(v)) => view! { <ProviderList list=rows(&v) rev=rev /> }.into_any(),
            }}
        </PrefGroup>
    }
}

#[component]
fn ProviderList(list: Vec<Value>, rev: RwSignal<i32>) -> impl IntoView {
    let list = StoredValue::new(list);
    let busy = RwSignal::new(false);
    let selected = RwSignal::new(None::<String>);
    let editing = RwSignal::new(None::<String>);
    let label_of = move |id: &str| {
        list.with_value(|l| {
            l.iter()
                .find(|p| text(p, "id") == id)
                .map(|p| text(p, "label"))
                .unwrap_or_else(|| id.to_owned())
        })
    };
    let keyed = move |id: &str| {
        undo::shows(id)
            && list.with_value(|l| {
                l.iter()
                    .find(|p| text(p, "id") == id)
                    .is_some_and(|p| flag(p, "has_key"))
            })
    };
    let accepts_key = move |id: &str| {
        list.with_value(|l| {
            l.iter()
                .find(|p| text(p, "id") == id)
                .is_some_and(|p| p.get("has_key").is_some())
        })
    };
    let edit = Callback::new(move |id: String| {
        if accepts_key(&id) {
            editing.set(Some(id));
        } else {
            toast(format!("{} 不需要密钥", label_of(&id)));
        }
    });
    let remove = Callback::new(move |id: String| {
        if !keyed(&id) {
            toast(format!("{} 没有配置密钥", label_of(&id)));
            return;
        }
        undo::remove_row(
            format!("已删除 {} 的密钥", label_of(&id)),
            id.clone(),
            Request {
                method: "PUT",
                path: "/api/skills/market/key".into(),
                body: json!({"provider": id, "key": ""}),
            },
            move || {
                rev.try_update(|n| *n += 1);
            },
        );
    });
    let add = Callback::new(move |()| {
        let first = list.with_value(|l| {
            l.iter()
                .find(|p| p.get("has_key").is_some())
                .map(|p| text(p, "id"))
        });
        match first {
            Some(id) => editing.set(Some(id)),
            None => toast("没有需要密钥的来源"),
        }
    });
    view! {
        <PrefList
            label="技能来源"
            selected
            on_add=add
            on_remove=remove
            on_edit=edit
            busy=busy
        >
            {list
                .get_value()
                .into_iter()
                .map(|p| {
                    let id = text(&p, "id");
                    let home = text(&p, "home");
                    let status_id = id.clone();
                    let takes_key = p.get("has_key").is_some();
                    let has_key = flag(&p, "has_key");
                    let status = move || {
                        if !takes_key {
                            ""
                        } else if has_key && undo::shows(&status_id) {
                            "已配置密钥"
                        } else {
                            "未配置密钥"
                        }
                    };
                    view! {
                        <PrefItem
                            id=id
                            selected
                            on_open=edit
                            title=text(&p, "label")
                            detail=text(&p, "note")
                            trailing=move || {
                                let status = status.clone();
                                view! {
                                    <span>{status}</span>
                                    <a
                                        class="linkbtn"
                                        href=home.clone()
                                        target="_blank"
                                        rel="noopener noreferrer"
                                    >
                                        "打开 ↗"
                                    </a>
                                }
                            }
                        />
                    }
                })
                .collect_view()}
        </PrefList>
        {move || {
            editing
                .get()
                .map(|id| {
                    view! {
                        <KeySheet
                            provider=id.clone()
                            label=label_of(&id)
                            on_close=Callback::new(move |()| editing.set(None))
                            on_saved=Callback::new(move |()| {
                                editing.set(None);
                                rev.update(|n| *n += 1);
                            })
                        />
                    }
                })
        }}
    }
}

#[component]
fn KeySheet(
    provider: String,
    label: String,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> impl IntoView {
    let key = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let provider = StoredValue::new(provider);
    let empty = move || key.with(|k| k.trim().is_empty());
    let submit = move || {
        if busy.get_untracked() || empty() {
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            if save(
                "PUT",
                "/api/skills/market/key",
                json!({"provider": provider.get_value(), "key": key.get_untracked().trim()}),
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
            title=format!("{label} 的 API key")
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
                        {move || if busy.get() { "保存中…" } else { "保存" }}
                    </button>
                }
            }
        >
            <p>"密钥保存后这里不再显示。"</p>
            <label class="field">
                "API key"
                <input
                    type="password"
                    autocomplete="new-password"
                    data-modal-initial-focus=""
                    prop:value=move || key.get()
                    on:input=move |e| key.set(event_target_value(&e))
                    on:keydown=move |e| {
                        if e.key() == "Enter" {
                            e.prevent_default();
                            submit();
                        }
                    }
                />
            </label>
        </Sheet>
    }
}
