use super::work_shared::*;
use crate::action_gate::ActionGate;
use crate::components::empty::{self, Action, Blank};
use crate::components::group::ListGroup;
use crate::components::icon;
use crate::components::page_head::PageHead;
use crate::components::rows::{MenuAt, RowAction, RowMenu};
use crate::components::status::{InlineError, LoadingState, Status, Tone};
use crate::components::toast::toast;
use crate::{api, app_state::use_app, fmt, storage};
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

const KINDS: &[(&str, &str)] = &[
    ("", "全部"),
    ("run_done", "运行完成"),
    ("run_failed", "运行失败"),
    ("approval", "等你批准"),
    ("question", "待回答"),
    ("bot_reply", "bot 回复"),
    ("autopilot_paused", "自动化已暂停"),
    ("rate_limit", "额度告警"),
    ("job", "后台任务"),
    ("alert", "提醒规则"),
];

#[derive(Clone, PartialEq)]
struct Item {
    id: String,
    kind: String,
    label: String,
    title: String,
    body: String,
    raw: String,
    href: String,
    read: bool,
    archived: bool,
    resolved: String,
    ref_id: String,
    created: String,
}

impl Item {
    fn open_request(&self) -> bool {
        matches!(self.kind.as_str(), "approval" | "question")
            && !self.archived
            && self.resolved.is_empty()
            && !self.ref_id.is_empty()
    }
}

fn href_of(it: &Value, k: &str, ws: &str, thread: &str) -> String {
    let rule_ref = s(it, "ref_id");
    let rule = rule_ref
        .strip_prefix("rule:")
        .and_then(|r| r.split_once(':'));
    if let Some((rule, subject)) = rule.filter(|(r, _)| {
        k == "alert"
            && matches!(
                *r,
                "quota_high" | "job_orphan" | "node_offline" | "bot_stuck"
            )
    }) {
        match rule {
            "quota_high" => "/runtimes".into(),
            "job_orphan" => "/jobs".into(),
            "node_offline" => format!("/nodes/{}", api::enc(subject)),
            _ => format!("/bots/{}", api::enc(subject)),
        }
    } else if k == "autopilot_paused" {
        format!("/autopilots?id={}", api::enc(&s(it, "ref_id")))
    } else if k == "rate_limit" {
        "/runtimes".into()
    } else if k == "job" && thread.is_empty() {
        "/jobs".into()
    } else if !thread.is_empty() && !ws.is_empty() {
        thread_link(ws, thread)
    } else if !ws.is_empty() {
        format!("/w/{}", api::enc(ws))
    } else {
        "/inbox".into()
    }
}

fn item_of(it: &Value) -> Item {
    let k = s(it, "kind");
    let ws = s(it, "workspace_id");
    let thread = s(it, "thread_id");
    let raw = s(it, "body");
    let human = if k == "run_failed" {
        crate::vocab::cause(&raw)
    } else {
        crate::vocab::error(&raw, None)
    };
    Item {
        id: s(it, "id"),
        label: KINDS
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| *v)
            .unwrap_or("通知")
            .to_string(),
        title: s(it, "title"),
        body: human.clone(),
        raw: if human == raw { String::new() } else { raw },
        href: href_of(it, &k, &ws, &thread),
        read: it["read"].as_bool() == Some(true),
        archived: it["archived"].as_bool() == Some(true),
        resolved: s(it, "resolved_at"),
        ref_id: s(it, "ref_id"),
        created: s(it, "created_at"),
        kind: k,
    }
}

#[derive(Clone, Copy)]
struct Ctx {
    sel: RwSignal<usize>,
    menu: RwSignal<Option<MenuAt>>,
    gate: RwSignal<ActionGate>,
    mark: Callback<(String, Vec<String>)>,
    rev: RwSignal<u32>,
    appr_rev: RwSignal<u32>,
}

fn decide(ctx: Ctx, id: String, allow: bool, answers: Option<Value>) {
    decide_approval(ctx.gate, id, allow, answers, move || {
        let _ = ctx.appr_rev.try_update(|n| *n += 1);
        refresh(ctx.rev);
    });
}

#[component]
pub fn InboxPage() -> impl IntoView {
    let saved =
        storage::load::<Value>("blazar.inboxview").unwrap_or(json!({"filter":"all","kind":""}));
    let filter = RwSignal::new(s(&saved, "filter"));
    let kind = RwSignal::new(s(&saved, "kind"));
    let rev = work_revision("inbox");
    let busy = RwSignal::new(false);
    let app = use_app();
    let go = StoredValue::new_local(use_navigate());
    let list_el = NodeRef::<html::Div>::new();
    let data = LocalResource::new(move || {
        rev.track();
        let (filter, kind) = (filter.get(), kind.get());
        storage::save("blazar.inboxview", &json!({"filter":filter,"kind":kind}));
        async move {
            let r = api::get::<Value>(&format!(
                "/api/inbox?filter={}&kind={}",
                api::enc(&filter),
                api::enc(&kind)
            ))
            .await;
            if let Ok(r) = &r {
                app.inbox_unread.try_set(r["unread"].as_u64().unwrap_or(0));
            }
            r
        }
    });
    let items = Memo::new(move |_| {
        data.get()
            .and_then(|r| r.ok())
            .map(|r| arr(&r, "items").iter().map(item_of).collect::<Vec<Item>>())
            .unwrap_or_default()
    });
    let mark = Callback::new(move |(action, ids): (String, Vec<String>)| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let mut body = json!({"action":action});
            if !ids.is_empty() {
                body["ids"] = json!(ids);
            }
            match api::send::<Value>("POST", "/api/inbox", &body).await {
                Ok(_) => refresh(rev),
                Err(e) => toast(e.to_string()),
            }
            busy.try_set(false);
        });
    });
    let ctx = Ctx {
        sel: RwSignal::new(0),
        menu: RwSignal::new(None),
        gate: RwSignal::new(ActionGate::default()),
        mark,
        rev,
        appr_rev: RwSignal::new(0),
    };
    let approvals = LocalResource::new(move || {
        ctx.appr_rev.track();
        app.state.track();
        let need = items.with(|it| it.iter().any(Item::open_request));
        async move {
            if !need {
                return Vec::<Value>::new();
            }
            api::get::<Vec<Value>>("/api/approvals")
                .await
                .unwrap_or_default()
        }
    });
    let pending_of = move |it: &Item| -> Option<Value> {
        if !it.open_request() {
            return None;
        }
        let id = it.ref_id.clone();
        approvals.get().and_then(|l| {
            l.iter()
                .find(|a| a["id"].as_str() == Some(id.as_str()))
                .cloned()
        })
    };
    Effect::new(move |_| {
        let n = items.with(Vec::len);
        if n > 0 && ctx.sel.get_untracked() >= n {
            ctx.sel.set(n - 1);
        }
    });
    let has_any = move || {
        data.with(|d| {
            d.as_ref()
                .and_then(|r| r.as_ref().ok())
                .is_some_and(|r| r["total"].as_u64().unwrap_or(1) > 0)
        })
    };
    let focus_row = move |i: usize| {
        if let Some(list) = list_el.try_get_untracked().flatten()
            && let Ok(Some(row)) = list.query_selector(&format!(".inbox-row[data-index=\"{i}\"]"))
            && let Ok(row) = row.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = row.focus();
        }
    };
    let selected = move || {
        let i = ctx.sel.try_get_untracked()?;
        items.try_with_untracked(|it| it.get(i).cloned()).flatten()
    };
    let open = move |it: Item| {
        if !it.read {
            mark.run(("read".into(), vec![it.id.clone()]));
        }
        go.with_value(|n| n(&it.href, Default::default()));
    };
    let keys = window_event_listener(ev::keydown, move |e| {
        if e.default_prevented() || e.is_composing() || overlay_open() {
            return;
        }
        let n = items.try_with_untracked(Vec::len).unwrap_or(0);
        if n == 0 {
            return;
        }
        let typing = in_field(&e);
        let meta = e.meta_key() || e.ctrl_key();
        let key = e.key();
        match key.as_str() {
            "ArrowDown" | "ArrowUp" if !typing && !meta => {
                e.prevent_default();
                let i = ctx.sel.try_get_untracked().unwrap_or(0);
                let next = if key == "ArrowDown" {
                    (i + 1).min(n - 1)
                } else {
                    i.saturating_sub(1)
                };
                ctx.sel.set(next);
                focus_row(next);
            }
            "Enter" if !typing && !meta => {
                if let Some(it) = selected() {
                    e.prevent_default();
                    open(it);
                }
            }
            "Enter" | "Backspace" if meta && !typing => {
                let Some(a) = selected().and_then(|it| pending_of(&it)) else {
                    return;
                };
                if crate::chat_model::tool_name(a["request"]["tool_name"].as_str().unwrap_or(""))
                    == "AskUserQuestion"
                {
                    return;
                }
                e.prevent_default();
                decide(
                    ctx,
                    a["id"].as_str().unwrap_or("").to_owned(),
                    key == "Enter",
                    None,
                );
            }
            "e" | "E" if !typing && !meta => {
                if let Some(it) = selected() {
                    e.prevent_default();
                    mark.run((
                        if it.archived { "unarchive" } else { "archive" }.into(),
                        vec![it.id],
                    ));
                }
            }
            "u" | "U" if !typing && !meta => {
                if let Some(it) = selected() {
                    e.prevent_default();
                    mark.run((if it.read { "unread" } else { "read" }.into(), vec![it.id]));
                }
            }
            _ => {}
        }
    });
    on_cleanup(move || keys.remove());
    view! {
        <div class="page work-page inbox-page">
            <PageHead
                title="收件箱"
                status=move || {
                    let n = app.inbox_unread.get();
                    (n != 0).then(|| view! { <Status tone=Tone::Attention label=format!("{n} 条未读") /> })
                }
            >
                <Show when=move || { has_any() && app.inbox_unread.get() > 0 }>
                    <button
                        type="button"
                        class="btn"
                        disabled=move || busy.get()
                        on:click=move |_| mark.run(("read".into(), vec![]))
                    >
                        "全部已读"
                    </button>
                </Show>
            </PageHead>
            <Show when=has_any>
                <div class="work-filters">
                    {filter_select(
                        "显示",
                        filter,
                        &[("unread", "未读"), ("all", "全部"), ("archived", "已归档")],
                    )} {filter_select("类型", kind, KINDS)}
                </div>
            </Show>
            {move || match data.get() {
                None => view! { <LoadingState text="加载通知…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=e.to_string()
                            retry=Callback::new(move |_| refresh(rev))
                        />
                    }
                        .into_any()
                }
                Some(Ok(r)) => {
                    let list = items.get();
                    if list.is_empty() {
                        if r["total"].as_u64() == Some(0) {
                            return view! {
                                <Blank
                                    icon=icon::INBOX
                                    title=empty::never("通知")
                                    detail="工作跑完、出错或需要你批准时，会在这里通知你。"
                                />
                            }
                                .into_any();
                        }
                        if !kind.get().is_empty() || filter.get() == "archived" {
                            return view! {
                                <Blank
                                    icon=icon::FILTER
                                    title=empty::unmatched("通知")
                                    detail="换个类型，或者看看全部通知。"
                                    action=Action::clear(move || {
                                        filter.set("all".into());
                                        kind.set(String::new());
                                    })
                                />
                            }
                                .into_any();
                        }
                        return view! {
                            <Blank
                                icon=icon::CHECK
                                title=empty::DONE
                                detail="新的结果和请求会出现在这里。"
                            />
                        }
                            .into_any();
                    }
                    view! {
                        <ListGroup>
                            <div
                                class="inbox-list"
                                role="list"
                                aria-label="通知"
                                aria-busy=move || busy.get().to_string()
                                node_ref=list_el
                            >
                                {list
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, it)| {
                                        let pending = pending_of(&it);
                                        row_view(ctx, i, it, pending)
                                    })
                                    .collect_view()}
                            </div>
                        </ListGroup>
                    }
                        .into_any()
                }
            }}
            {move || {
                ctx.menu
                    .get()
                    .and_then(|at| {
                        items
                            .with(|it| it.get(at.index).cloned())
                            .map(|it| {
                                view! {
                                    <RowMenu
                                        at
                                        label="通知的操作"
                                        groups=inbox_menu(ctx, it)
                                        on_close=Callback::new(move |()| ctx.menu.set(None))
                                    />
                                }
                            })
                    })
            }}
        </div>
    }
}

fn question_panel(ctx: Ctx, aid: String, a: &Value, href: String) -> AnyView {
    let questions = a["request"]["input"]["questions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if questions.len() != 1 {
        return view! {
            <div class="inbox-act">
                <a class="btn small primary" href=href>
                    "去回答"
                </a>
            </div>
        }
        .into_any();
    }
    let q = questions[0]["question"].as_str().unwrap_or("").to_owned();
    let options: Vec<String> = questions[0]["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .filter_map(|x| x["label"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let reply = RwSignal::new(String::new());
    let gate_id = aid.clone();
    let answer = move |text: String| {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        decide(ctx, aid.clone(), true, Some(json!({ q.clone(): text })));
    };
    let send = answer.clone();
    view! {
        <div class="inbox-act" data-ui-guard="">
            {options
                .into_iter()
                .map(|label| {
                    let pick = answer.clone();
                    let shown = label.clone();
                    let waiting = gate_id.clone();
                    view! {
                        <button
                            type="button"
                            class="btn small"
                            disabled=move || ctx.gate.with(|g| g.busy(&waiting))
                            on:click=move |_| pick(label.clone())
                        >
                            {shown}
                        </button>
                    }
                })
                .collect_view()}
            <input
                class="inbox-reply"
                aria-label="回复"
                placeholder="回复…"
                prop:value=move || reply.get()
                on:input=move |e| reply.set(event_target_value(&e))
                on:keydown=move |e: ev::KeyboardEvent| {
                    if e.key() == "Enter" && !e.is_composing() {
                        e.prevent_default();
                        e.stop_propagation();
                        send(reply.get_untracked());
                    }
                }
            />
        </div>
    }
    .into_any()
}

fn row_view(ctx: Ctx, i: usize, it: Item, pending: Option<Value>) -> impl IntoView {
    let href = it.href.clone();
    let read = it.read;
    let id_read = it.id.clone();
    let body_in_panel = pending.is_some() && it.kind == "approval";
    let panel = pending.map(|a| {
        let aid = a["id"].as_str().unwrap_or("").to_owned();
        if crate::chat_model::tool_name(a["request"]["tool_name"].as_str().unwrap_or(""))
            == "AskUserQuestion"
        {
            question_panel(ctx, aid, &a, href.clone())
        } else {
            let (label, what) = approval_summary(&a["request"], "");
            let (a1, a2, a3, a4) = (aid.clone(), aid.clone(), aid.clone(), aid);
            view! {
                <div class="inbox-act" data-ui-guard="">
                    <code class="inbox-cmd" title=what.clone()>
                        {format!("{label}  {what}")}
                    </code>
                    <button
                        type="button"
                        class="btn small"
                        disabled=move || ctx.gate.with(|g| g.busy(&a1))
                        on:click=move |_| decide(ctx, a2.clone(), true, None)
                    >
                        "允许"
                        <kbd>{crate::shortcuts::label("Mod+Enter")}</kbd>
                    </button>
                    <button
                        type="button"
                        class="btn small"
                        disabled=move || ctx.gate.with(|g| g.busy(&a3))
                        on:click=move |_| decide(ctx, a4.clone(), false, None)
                    >
                        "拒绝"
                    </button>
                </div>
            }
            .into_any()
        }
    });
    view! {
        <div
            class="inbox-row lrow"
            role="listitem"
            data-index=i.to_string()
            data-kind=it.kind.clone()
            data-unread=(!read).to_string()
            data-archived=it.archived.to_string()
            data-sel=move || (ctx.sel.get() == i).to_string()
            data-menu=move || ctx.menu.with(|m| m.is_some_and(|m| m.index == i)).to_string()
            tabindex=move || if ctx.sel.get() == i { "0" } else { "-1" }
            on:focusin=move |_| ctx.sel.set(i)
            on:contextmenu=move |e: ev::MouseEvent| {
                e.prevent_default();
                ctx.sel.set(i);
                ctx.menu.set(Some(MenuAt::from(&e, i)));
            }
        >
            <span class="inbox-dot" aria-hidden="true"></span>
            <div class="inbox-col">
                <a
                    class="inbox-main"
                    href=href
                    tabindex="-1"
                    on:click=move |_| {
                        if !read {
                            ctx.mark.run(("read".into(), vec![id_read.clone()]));
                        }
                    }
                >
                    <strong>{it.title.clone()}</strong>
                    {(!it.body.is_empty() && !body_in_panel)
                        .then(|| view! { <span class="inbox-body">{it.body.clone()}</span> })}
                    {(!it.raw.is_empty())
                        .then(|| view! { <small class="inbox-raw">{it.raw.clone()}</small> })}
                </a>
                {panel}
            </div>
            <small class="inbox-meta">
                {format!("{} · {}", it.label, fmt::ago(&it.created))}
                {(!it.resolved.is_empty())
                    .then(|| {
                        view! {
                            <Status
                                tone=Tone::Other
                                label=format!("已解决 · {}", fmt::ago(&it.resolved))
                            />
                        }
                    })}
            </small>
        </div>
    }
}

fn inbox_menu(ctx: Ctx, item: Item) -> Vec<Vec<RowAction>> {
    let (id_read, id_arch) = (item.id.clone(), item.id.clone());
    let (read, archived) = (item.read, item.archived);
    vec![
        vec![RowAction::open(item.href.clone())],
        vec![
            RowAction::new(
                if read { "标为未读" } else { "标为已读" },
                move || {
                    ctx.mark.run((
                        if read { "unread" } else { "read" }.into(),
                        vec![id_read.clone()],
                    ));
                },
            )
            .hint("U"),
            RowAction::new(if archived { "恢复" } else { "归档" }, move || {
                ctx.mark.run((
                    if archived { "unarchive" } else { "archive" }.into(),
                    vec![id_arch.clone()],
                ));
            })
            .hint("E"),
        ],
    ]
}
