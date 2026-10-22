use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::dialog::{Choice, ask};
use crate::components::prefs::{
    PrefAdvanced, PrefEmpty, PrefGroup, PrefItem, PrefList, PrefRow, Sheet, Switch,
};
use crate::components::status::{InlineError, LoadingState};

use super::{flag, rows, save, text};

const EVENTS: &[(&str, &str)] = &[
    ("turn_start", "一轮开始前"),
    ("turn_end", "一轮结束"),
    ("session_start", "会话建立"),
    ("session_end", "会话结束"),
    ("error", "出错"),
];

const TARGETS: &[(&str, &str)] = &[("hub", "Blazar 所在机器"), ("node", "工作区所在机器")];

fn label_in(table: &[(&str, &str)], key: &str) -> String {
    table
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(key, |(_, v)| v)
        .to_owned()
}

#[component]
pub(super) fn HookSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let selected = RwSignal::new(None::<String>);
    let adding = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/hooks")
    });
    let remove = Callback::new(move |id: String| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            if ask(
                "删除自动执行脚本",
                "删除后不再自动执行这个脚本。",
                vec![Choice::plain("保留脚本"), Choice::danger("删除脚本")],
            )
            .await
                == Some(1)
                && save(
                    "DELETE",
                    &format!("/api/hooks/{}", api::enc(&id)),
                    json!({}),
                )
                .await
            {
                selected.set(None);
                rev.update(|n| *n += 1);
            }
            busy.set(false);
        });
    });
    view! {
        <PrefGroup title="自动执行脚本" note="在会话开始、结束或出错时执行脚本。">
            <PrefList
                label="自动执行脚本"
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
                        let list = rows(&v);
                        if list.is_empty() {
                            return view! {
                                <PrefEmpty text="还没有自动执行脚本。按「+」添加脚本并选择执行时点。" />
                            }
                                .into_any();
                        }
                        list.into_iter()
                            .map(|h| {
                                view! {
                                    <PrefItem
                                        id=text(&h, "id")
                                        selected
                                        title=text(&h, "command")
                                        code=true
                                        detail=format!(
                                            "{} · {}{}",
                                            label_in(EVENTS, &text(&h, "event")),
                                            label_in(TARGETS, &text(&h, "target")),
                                            if flag(&h, "blocking") { " · 失败时阻止执行" } else { "" },
                                        )
                                    />
                                }
                            })
                            .collect_view()
                            .into_any()
                    }
                }}
            </PrefList>
            <PrefAdvanced title="执行说明与变量">
                <p>"一轮开始前的脚本可在失败时阻止执行。"</p>
                <p>
                    "脚本能读到这些变量："
                    <code>"BLAZAR_EVENT"</code>
                    "、"
                    <code>"BLAZAR_WORKSPACE"</code>
                    "、"
                    <code>"BLAZAR_NODE"</code>
                    "、"
                    <code>"BLAZAR_CWD"</code>
                    "、"
                    <code>"BLAZAR_SESSION"</code>
                    "、"
                    <code>"BLAZAR_TOOL"</code>
                    "。"
                </p>
            </PrefAdvanced>
        </PrefGroup>
        <Show when=move || adding.get()>
            <HookSheet
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
fn HookSheet(on_close: Callback<()>, on_saved: Callback<()>) -> impl IntoView {
    let event = RwSignal::new("turn_start".to_owned());
    let target = RwSignal::new("hub".to_owned());
    let command = RwSignal::new(String::new());
    let timeout = RwSignal::new(30u32);
    let blocking = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let can_block = Signal::derive(move || event.get() == "turn_start");
    let empty = move || command.with(|c| c.trim().is_empty());
    let submit = move || {
        if busy.get_untracked() || empty() {
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            if save(
                "POST",
                "/api/hooks",
                json!({
                    "event": event.get_untracked(),
                    "target": target.get_untracked(),
                    "command": command.get_untracked().trim(),
                    "blocking": blocking.get_untracked() && can_block.get_untracked(),
                    "timeout_secs": timeout.get_untracked(),
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
            title="添加自动执行脚本"
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
                        {move || if busy.get() { "保存中…" } else { "添加脚本" }}
                    </button>
                }
            }
        >
            <label class="field">
                "时点"
                <select
                    prop:value=move || event.get()
                    on:change=move |e| event.set(event_target_value(&e))
                >
                    {EVENTS
                        .iter()
                        .map(|&(k, v)| {
                            view! {
                                <option value=k selected=move || event.get() == k>
                                    {v}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </label>
            <label class="field">
                "执行位置"
                <select
                    prop:value=move || target.get()
                    on:change=move |e| target.set(event_target_value(&e))
                >
                    {TARGETS
                        .iter()
                        .map(|&(k, v)| {
                            view! {
                                <option value=k selected=move || target.get() == k>
                                    {v}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </label>
            <label class="field">
                "命令"
                <input
                    data-modal-initial-focus=""
                    autocomplete="off"
                    spellcheck="false"
                    prop:value=move || command.get()
                    on:input=move |e| command.set(event_target_value(&e))
                />
            </label>
            <label class="field">
                "超时（秒）"
                <input
                    type="number"
                    min="1"
                    max="600"
                    prop:value=move || timeout.get().to_string()
                    on:input=move |e| {
                        timeout.set(event_target_value(&e).parse().unwrap_or(30).clamp(1, 600))
                    }
                />
            </label>
            <PrefRow
                label="失败时阻止执行"
                detail="只对一轮开始前的脚本有效。"
                for_id="hook-blocking"
                enabled=can_block
            >
                <Switch
                    id="hook-blocking"
                    checked=blocking
                    disabled=Signal::derive(move || !can_block.get())
                    on_change=move |v: bool| blocking.set(v)
                />
            </PrefRow>
        </Sheet>
    }
}
