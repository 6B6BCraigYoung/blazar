mod pulse;
mod rail;
mod room;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};

use crate::api::{self, ChatThread};
use crate::app_state::{AppData, use_app};
use crate::components::empty::{self, Action, Blank};
use crate::components::group::ListGroup;
use crate::components::icon;
use crate::components::page_head::PageHead;
use crate::components::rows::{RowAction, RowKey, RowMenu, Rows};
use crate::components::toast::toast;
use crate::components::undo::{self, Request};
use crate::{fmt, vocab};

pub use room::ConsolePage;

pub(crate) const NEW: &str = "/chats/new";

pub(crate) fn title_of(t: &ChatThread) -> String {
    if t.title.trim().is_empty() {
        "新对话".to_owned()
    } else {
        t.title.clone()
    }
}

pub(crate) fn rename_chat(app: AppData, id: String, current: String) {
    spawn_local(async move {
        let Some(name) =
            crate::components::dialog::prompt_with("重命名对话", "对话名称", &current, "重命名")
                .await
        else {
            return;
        };
        match api::send::<Value>(
            "PUT",
            &format!("/api/sessions/{id}/title"),
            &json!({ "title": name }),
        )
        .await
        {
            Ok(r) if r["ok"].as_bool() == Some(true) => app.load_chats(),
            Ok(r) => toast(r["reason"].as_str().unwrap_or("没能重命名").to_owned()),
            Err(e) => toast(format!("没能重命名：{e}")),
        }
    });
}

pub(crate) fn delete_chat(app: AppData, t: &ChatThread) {
    let Some(ws) = app.chats.with_untracked(|a| a.id.clone()) else {
        return;
    };
    if matches!(t.state(), "running" | "awaiting_approval") {
        toast("这个对话还在运行，先停下它再删");
        return;
    }
    undo::remove_row(
        format!("已删除「{}」", title_of(t)),
        t.id.clone(),
        Request::delete(format!("/api/workspaces/{ws}/sessions/{}", t.id)),
        move || app.load_chats(),
    );
}

#[component]
pub fn ChatsPage() -> impl IntoView {
    let app = use_app();
    let navigate = StoredValue::new_local(use_navigate());
    app.load_chats();
    let threads = Memo::new(move |_| {
        app.chats.with(|a| {
            a.threads
                .iter()
                .filter(|t| undo::shows(&t.id))
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let rows = Rows::default();
    let go = move |href: String| navigate.with_value(|n| n(&href, Default::default()));
    rows.listen(
        ".chat-row",
        move || threads.try_with_untracked(Vec::len).unwrap_or(0),
        move |key, i| {
            let Some(t) = threads.try_with_untracked(|l| l.get(i).cloned()).flatten() else {
                return;
            };
            match key {
                RowKey::Open => go(format!("/chats/{}", t.id)),
                RowKey::Inspect => {}
                RowKey::Delete => delete_chat(app, &t),
            }
        },
    );
    let new_chat = move || go(NEW.to_owned());
    let menu_of = move |t: ChatThread| -> Vec<Vec<RowAction>> {
        let (id, title) = (t.id.clone(), title_of(&t));
        vec![
            vec![RowAction::open(format!("/chats/{}", t.id))],
            vec![RowAction::new("重命名…", move || {
                rename_chat(app, id.clone(), title.clone())
            })],
            vec![RowAction::delete("删除", move || delete_chat(app, &t))],
        ]
    };
    view! {
        <div class="page work-page chats-page">
            <PageHead title="全部对话" parent=("中控台", "/")>
                <button type="button" class="btn primary" on:click=move |_| new_chat()>
                    "新对话"
                    <kbd>{crate::shortcuts::label(&crate::shortcuts::current("newchat"))}</kbd>
                </button>
            </PageHead>
            {move || {
                let list = threads.get();
                if list.is_empty() {
                    return view! {
                        <Blank
                            icon=icon::CHAT
                            title=empty::never("对话")
                            detail="不属于任何项目的事在这里问：看几个工作区的进展、让某台机器接着做、汇总所有机器的占用。"
                            action=Action::new("新对话", new_chat).keys("Mod+T").primary()
                        />
                    }
                        .into_any();
                }
                let items = list
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| row_view(rows, i, t))
                    .collect_view();
                view! {
                    <ListGroup>
                        <div class="chat-list" role="list" aria-label="对话">
                            {items}
                        </div>
                    </ListGroup>
                }
                    .into_any()
            }}
            {move || {
                rows.menu
                    .get()
                    .and_then(|at| {
                        threads
                            .with(|l| l.get(at.index).cloned())
                            .map(|t| {
                                view! {
                                    <RowMenu
                                        at
                                        label="对话的操作"
                                        groups=menu_of(t)
                                        on_close=Callback::new(move |()| rows.close_menu())
                                    />
                                }
                            })
                    })
            }}
        </div>
    }
}

fn row_view(rows: Rows, i: usize, t: ChatThread) -> impl IntoView {
    let (label, tone) = vocab::state(t.state());
    let href = format!("/chats/{}", t.id);
    let when = fmt::ago(t.when());
    let title = title_of(&t);
    let summary = if t.state() == "failed" && !t.summary.is_empty() {
        format!("失败：{}", vocab::cause(&t.summary))
    } else {
        t.summary.clone()
    };
    view! {
        <a
            class="chat-row lrow"
            role="listitem"
            href=href
            data-index=i.to_string()
            data-state=t.state().to_owned()
            data-sel=move || rows.selected(i).to_string()
            data-menu=move || rows.menu_on(i).to_string()
            on:focusin=move |_| rows.sel.set(Some(i))
            on:contextmenu=move |e| rows.open_menu(&e, i)
        >
            <span class="dot" data-tone=tone.key() title=label aria-hidden="true"></span>
            <span class="chat-main">
                <b class="chat-title">{title}</b>
                {(!summary.is_empty()).then(|| view! { <span class="chat-sum">{summary}</span> })}
            </span>
            <small class="chat-when">{when}</small>
        </a>
    }
}
