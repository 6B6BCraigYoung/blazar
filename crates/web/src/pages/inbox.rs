use super::work_shared::*;
use crate::action_gate::ActionGate;
use crate::components::empty::{self, Action, Blank};
use crate::components::group::ListGroup;
use crate::components::icon;
use crate::components::rows::{MenuAt, RowAction, RowMenu};
use crate::components::status::{InlineError, LoadingState, Status, Tone};
use crate::components::toast::toast;
use crate::failure::{self, Failure};
use crate::pages::workspaces::retry_workspace;
use crate::{api, app_state::use_app, fmt, storage};
use blazar_core_types::api::{Ask, DEFAULT_ASK_OPTIONS};
use blazar_core_types::failure::FailureCause;
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

pub(crate) const KINDS: &[(&str, &str)] = &[
    ("", "全部"),
    ("run_done", "运行完成"),
    ("run_failed", "运行失败"),
    ("approval", "等你批准"),
    ("question", "待回答"),
    ("denied", "被拦下"),
    ("bot_reply", "bot 回复"),
    ("autopilot_paused", "自动化已暂停"),
    ("rate_limit", "额度告警"),
    ("job", "后台任务"),
    ("alert", "提醒规则"),
];

pub(crate) const ALL: &str = "/?inbox=all";

#[derive(Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub workspace: String,
    pub thread: String,
    pub session: String,
    pub label: String,
    pub title: String,
    pub body: String,
    pub raw: String,
    pub href: String,
    pub read: bool,
    pub archived: bool,
    pub resolved: String,
    pub ref_id: String,
    pub created: String,
    pub failure: Option<Failure>,
    pub handled: bool,
    pub place: String,
    pub ask: Option<Ask>,
}

impl Item {
    pub fn asking(&self) -> bool {
        self.ref_id.starts_with("ask:") && self.open_request()
    }

    pub fn open_request(&self) -> bool {
        matches!(self.kind.as_str(), "approval" | "question")
            && !self.archived
            && self.resolved.is_empty()
            && !self.ref_id.is_empty()
    }

    pub fn open_denial(&self) -> bool {
        self.kind == "denied"
            && !self.archived
            && self.resolved.is_empty()
            && !self.ref_id.is_empty()
            && !self.session.is_empty()
    }

    pub fn needs_you(&self) -> bool {
        self.open_request()
            || self.open_denial()
            || (self.kind == "run_failed" && !self.handled && !self.archived)
    }
}

fn href_of(it: &Value, k: &str, ws: &str, thread: &str, chats: bool) -> String {
    if chats {
        return if thread.is_empty() {
            "/".into()
        } else {
            format!("/chats/{}", api::enc(thread))
        };
    }
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
        ALL.into()
    }
}

pub fn item_of(it: &Value, assistant: Option<&str>) -> Item {
    let k = s(it, "kind");
    let ws = s(it, "workspace_id");
    let thread = s(it, "thread_id");
    let chats = !ws.is_empty() && assistant == Some(ws.as_str());
    let raw = s(it, "body");
    let failure = (k == "run_failed").then(|| Failure {
        cause: FailureCause::parse(&s(it, "cause")).unwrap_or_else(|| FailureCause::of(&raw, true)),
        message: raw.lines().next().unwrap_or_default().to_owned(),
        workspace: ws.clone(),
        session: s(it, "session_id"),
        thread: thread.clone(),
        node: s(it, "node"),
        runtime: s(it, "runtime"),
        account: Some(s(it, "account")).filter(|a| !a.is_empty()),
        resets_at: Some(s(it, "resets_at")).filter(|r| !r.is_empty()),
    });
    let handled = it["handled"].as_bool() == Some(true) || !s(it, "resolved_at").is_empty();
    let place = if chats {
        "对话".to_owned()
    } else {
        fmt::ws_label_of(it)
    };
    let stored = s(it, "title");
    let stored = stored
        .strip_prefix(&format!("{place} · "))
        .filter(|_| !place.is_empty())
        .unwrap_or(&stored)
        .to_owned();
    let title = match &failure {
        Some(f) => f.line_or(Some(&stored)),
        None => stored,
    };
    let (body, raw) = match (&failure, k.as_str()) {
        (Some(_), _) | (None, "run_done") => (String::new(), raw),
        (None, "question") => {
            if s(it, "ref_id").starts_with("ask:") && !raw.trim().is_empty() {
                (raw, String::new())
            } else {
                (String::new(), raw)
            }
        }
        (None, "denied") => (raw, String::new()),
        (None, _) => {
            let human = crate::vocab::error(&raw, None);
            if human == raw {
                (raw, String::new())
            } else {
                (human, raw)
            }
        }
    };
    let ask = serde_json::from_value::<Ask>(it["ask"].clone()).ok();
    Item {
        id: s(it, "id"),
        workspace: ws.clone(),
        thread: thread.clone(),
        session: s(it, "session_id"),
        label: KINDS
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| *v)
            .unwrap_or("通知")
            .to_string(),
        title,
        body,
        raw,
        href: href_of(it, &k, &ws, &thread, chats),
        read: it["read"].as_bool() == Some(true),
        archived: it["archived"].as_bool() == Some(true),
        resolved: s(it, "resolved_at"),
        ref_id: s(it, "ref_id"),
        created: s(it, "created_at"),
        kind: k,
        failure,
        handled,
        place,
        ask,
    }
}

pub fn resolve_item(id: String, done: impl Fn() + 'static) {
    spawn_local(async move {
        match api::send::<Value>("POST", &format!("/api/inbox/{id}/resolve"), &json!({})).await {
            Ok(_) => done(),
            Err(e) => toast(e.to_string()),
        }
    });
}

fn answer_ask(ctx: Ctx, it: &Item, text: String) {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return;
    }
    let key = format!("ask:{}", it.id);
    if !ctx.gate.write().begin(&key) {
        return;
    }
    let id = it.id.clone();
    spawn_local(async move {
        let r = api::send::<Value>(
            "POST",
            &format!("/api/asks/{id}/answer"),
            &json!({ "text": text }),
        )
        .await;
        let _ = ctx.gate.try_update(|g| {
            g.complete(&key, false);
        });
        match r {
            Ok(r) if r["delivered"] == json!(false) => toast(
                r["reason"]
                    .as_str()
                    .unwrap_or("这个对话正在忙，稍后再回")
                    .to_owned(),
            ),
            Ok(_) => {
                toast("已回复");
                refresh(ctx.rev);
            }
            Err(e) => toast(format!("没能回复：{e}")),
        }
    });
}

fn ask_panel(ctx: Ctx, it: &Item) -> AnyView {
    let options = it
        .ask
        .as_ref()
        .map(|a| a.options.clone())
        .filter(|o| !o.is_empty())
        .unwrap_or_else(|| DEFAULT_ASK_OPTIONS.map(str::to_owned).to_vec());
    let reply = RwSignal::new(String::new());
    let key = format!("ask:{}", it.id);
    let (k1, k2) = (key.clone(), key);
    let item = it.clone();
    let send = move |text: String| answer_ask(ctx, &item, text);
    let pick = send.clone();
    let done_id = it.id.clone();
    view! {
        <div class="inbox-act" data-ui-guard="">
            {options
                .into_iter()
                .map(|label| {
                    let pick = pick.clone();
                    let shown = label.clone();
                    let waiting = k1.clone();
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
                disabled=move || ctx.gate.with(|g| g.busy(&k2))
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
            <button
                type="button"
                class="btn small"
                on:click=move |_| {
                    resolve_item(done_id.clone(), move || refresh(ctx.rev));
                }
            >
                "标为已处理"
            </button>
        </div>
    }
    .into_any()
}

pub fn mark_inbox(action: &str, ids: Vec<String>, done: impl Fn() + 'static) {
    let action = action.to_owned();
    spawn_local(async move {
        let mut body = json!({"action":action});
        if !ids.is_empty() {
            body["ids"] = json!(ids);
        }
        match api::send::<Value>("POST", "/api/inbox", &body).await {
            Ok(_) => done(),
            Err(e) => toast(e.to_string()),
        }
    });
}

fn failure_actions(it: &Item) -> Option<Failure> {
    (it.kind == "run_failed" && !it.handled && !it.archived)
        .then(|| it.failure.clone())
        .flatten()
}

fn failure_panel(f: Failure) -> AnyView {
    let app = use_app();
    let primary = f.primary();
    let href = f.node_href();
    let (ws, session) = (f.workspace.clone(), f.session.clone());
    let act = move |_| {
        let (ws, session) = (f.workspace.clone(), f.session.clone());
        failure::act(app, &f, move || retry_workspace(app, ws, session));
    };
    let done = move |_| {
        failure::mark_handled(
            app,
            ws.clone(),
            Some(session.clone()).filter(|s| !s.is_empty()),
            false,
        );
    };
    view! {
        <div class="inbox-act">
            <button type="button" class="btn small primary" on:click=act>
                {primary}
            </button>
            {href
                .map(|h| {
                    view! {
                        <a class="btn small" href=h>
                            "打开机器页"
                        </a>
                    }
                })}
            <button type="button" class="btn small" on:click=done>
                "标为已处理"
            </button>
        </div>
    }
    .into_any()
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

fn is_question(a: &Value) -> bool {
    crate::chat_model::tool_name(a["request"]["tool_name"].as_str().unwrap_or(""))
        == "AskUserQuestion"
}

fn pick_approval(ctx: Ctx, key: &str, a: &Value, workspace: String) {
    let id = a["id"].as_str().unwrap_or("").to_owned();
    match key {
        "1" => decide(ctx, id, true, None),
        "2" => allow_always(ctx.gate, id, a["request"].clone(), workspace, move || {
            let _ = ctx.appr_rev.try_update(|n| *n += 1);
            refresh(ctx.rev);
        }),
        _ => decide(ctx, id, false, None),
    }
}

fn decide_denial(ctx: Ctx, it: &Item, allow: bool) {
    let key = format!("{}:{}", it.session, it.ref_id);
    if !ctx.gate.write().begin(&key) {
        return;
    }
    let (sid, tid, ws, thread) = (
        it.session.clone(),
        it.ref_id.clone(),
        it.workspace.clone(),
        it.thread.clone(),
    );
    spawn_local(async move {
        let r = api::send::<Value>(
            "POST",
            &format!("/api/sessions/{sid}/denials"),
            &json!({ "tool_use_id": tid, "allow": allow }),
        )
        .await;
        let _ = ctx.gate.try_update(|g| {
            g.complete(&key, false);
        });
        match r {
            Ok(r) if r["recorded"].as_bool() == Some(true) => {
                if allow && let Some(text) = r["text"].as_str() {
                    let body = json!({
                        "text": text, "resume": true, "resume_session": thread,
                        "permission_mode": "auto", "images": [], "wait_secs": 20,
                    });
                    match api::send::<Value>("POST", &format!("/api/workspaces/{ws}/prompt"), &body)
                        .await
                    {
                        Ok(r) if r["admitted"] == json!(false) => toast(
                            r["reason"]
                                .as_str()
                                .unwrap_or("这个对话正在忙，稍后再重试")
                                .to_owned(),
                        ),
                        Ok(_) => toast("已允许，正在重试"),
                        Err(e) => toast(format!("已允许，但没能重试：{e}")),
                    }
                } else if !allow {
                    toast("已跳过");
                }
            }
            Ok(r) => toast(
                r["reason"]
                    .as_str()
                    .unwrap_or("这条操作已经决定过了")
                    .to_owned(),
            ),
            Err(e) => toast(format!("没能记录：{e}")),
        }
        refresh(ctx.rev);
    });
}

fn denied_panel(ctx: Ctx, it: &Item) -> AnyView {
    let key = format!("{}:{}", it.session, it.ref_id);
    let (k1, k2) = (key.clone(), key);
    let (a, n) = (it.clone(), it.clone());
    view! {
        <div class="inbox-act" data-ui-guard="">
            <code class="inbox-cmd" title=it.body.clone()>
                {it.body.clone()}
            </code>
            <button
                type="button"
                class="btn small primary"
                disabled=move || ctx.gate.with(|g| g.busy(&k1))
                on:click=move |_| decide_denial(ctx, &a, true)
            >
                "允许这一次并重试"
            </button>
            <button
                type="button"
                class="btn small"
                disabled=move || ctx.gate.with(|g| g.busy(&k2))
                on:click=move |_| decide_denial(ctx, &n, false)
            >
                "不用了"
            </button>
        </div>
    }
    .into_any()
}

#[component]
pub fn InboxPage() -> impl IntoView {
    let navigate = use_navigate();
    Effect::new(move |_| {
        navigate(
            ALL,
            NavigateOptions {
                replace: true,
                ..Default::default()
            },
        );
    });
    view! { <LoadingState text="正在打开中控台…" /> }
}

#[component]
pub fn InboxAll() -> impl IntoView {
    let saved =
        storage::load::<Value>("blazar.inboxview").unwrap_or(json!({"filter":"all","kind":""}));
    let filter = RwSignal::new(s(&saved, "filter"));
    let kind = RwSignal::new(s(&saved, "kind"));
    let rev = work_revision("inbox");
    let busy = RwSignal::new(false);
    let app = use_app();
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
        let assistant = app.assistant().map(|w| w.id);
        data.get()
            .and_then(|r| r.ok())
            .map(|r| {
                arr(&r, "items")
                    .iter()
                    .map(|it| item_of(it, assistant.as_deref()))
                    .collect::<Vec<Item>>()
            })
            .unwrap_or_default()
    });
    let has_any = move || {
        data.with(|d| {
            d.as_ref()
                .and_then(|r| r.as_ref().ok())
                .is_some_and(|r| r["total"].as_u64().unwrap_or(1) > 0)
        })
    };
    let read_all = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        mark_inbox("read", vec![], move || {
            refresh(rev);
            let _ = busy.try_set(false);
        });
    };
    view! {
        <div class="inbox-all">
            <div class="inbox-tools">
                <Show when=has_any>
                    <div class="work-filters">
                        {filter_select(
                            "显示",
                            filter,
                            &[("unread", "未读"), ("all", "全部"), ("archived", "已归档")],
                        )} {filter_select("类型", kind, KINDS)}
                    </div>
                </Show>
                <span class="grow"></span>
                <Show when=move || { has_any() && app.inbox_unread.get() > 0 }>
                    <button
                        type="button"
                        class="linkbtn"
                        disabled=move || busy.get()
                        on:click=read_all
                    >
                        "全部已读"
                    </button>
                </Show>
            </div>
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
                                    inset=true
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
                                    inset=true
                                />
                            }
                                .into_any();
                        }
                        return view! {
                            <Blank
                                icon=icon::CHECK
                                title=empty::DONE
                                detail="新的结果和请求会出现在这里。"
                                inset=true
                            />
                        }
                            .into_any();
                    }
                    view! {
                        <ListGroup>
                            <InboxCards items=Signal::from(items) rev label="通知" />
                        </ListGroup>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

#[component]
pub fn InboxCards(
    items: Signal<Vec<Item>>,
    rev: RwSignal<u32>,
    #[prop(default = "通知")] label: &'static str,
) -> impl IntoView {
    let app = use_app();
    let go = StoredValue::new_local(use_navigate());
    let list_el = NodeRef::<html::Div>::new();
    let busy = RwSignal::new(false);
    let mark = Callback::new(move |(action, ids): (String, Vec<String>)| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        mark_inbox(&action, ids, move || {
            refresh(rev);
            let _ = busy.try_set(false);
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
        if !it.open_request() || it.asking() {
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
    let keys = move |e: ev::KeyboardEvent| {
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
            "1" | "2" | "3" if !typing && !meta && !e.alt_key() && !e.shift_key() => {
                let Some(it) = selected() else {
                    return;
                };
                let Some(a) = pending_of(&it).filter(|a| !is_question(a)) else {
                    return;
                };
                e.prevent_default();
                pick_approval(ctx, &key, &a, it.workspace.clone());
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
    };
    view! {
        <div
            class="inbox-list"
            role="list"
            aria-label=label
            aria-busy=move || busy.get().to_string()
            node_ref=list_el
            on:keydown=keys
        >
            {move || {
                items
                    .get()
                    .into_iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let pending = pending_of(&it);
                        row_view(ctx, i, it, pending)
                    })
                    .collect_view()
            }}
        </div>
        {move || {
            ctx.menu
                .get()
                .and_then(|at| {
                    items
                        .with(|it| it.get(at.index).cloned())
                        .map(|it| {
                            let pending = pending_of(&it);
                            view! {
                                <RowMenu
                                    at
                                    label="通知的操作"
                                    groups=inbox_menu(ctx, it, pending)
                                    on_close=Callback::new(move |()| ctx.menu.set(None))
                                />
                            }
                        })
                })
        }}
    }
}

fn handled_button(ctx: Ctx, item_id: String) -> impl IntoView {
    view! {
        <button
            type="button"
            class="btn small"
            on:click=move |_| {
                resolve_item(item_id.clone(), move || refresh(ctx.rev));
            }
        >
            "标为已处理"
        </button>
    }
}

fn question_panel(ctx: Ctx, aid: String, a: &Value, href: String, item_id: String) -> AnyView {
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
                {handled_button(ctx, item_id)}
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
            {handled_button(ctx, item_id)}
        </div>
    }
    .into_any()
}

fn row_view(ctx: Ctx, i: usize, it: Item, pending: Option<Value>) -> impl IntoView {
    let href = it.href.clone();
    let read = it.read;
    let id_read = it.id.clone();
    let denial = it.open_denial();
    let body_in_panel = (pending.is_some() && it.kind == "approval") || denial;
    let acts = failure_actions(&it).map(failure_panel);
    let resolved_label = if it.kind == "run_failed" {
        "已处理"
    } else {
        "已解决"
    };
    let denied = denial.then(|| denied_panel(ctx, &it));
    let asking = it.asking().then(|| ask_panel(ctx, &it));
    let place = it.place.clone();
    let meta = move |label: &str, created: &str| {
        if place.is_empty() {
            format!("{label} · {}", fmt::ago(created))
        } else {
            format!("{place} · {label} · {}", fmt::ago(created))
        }
    };
    let item_id = it.id.clone();
    let panel = pending.map(|a| {
        let aid = a["id"].as_str().unwrap_or("").to_owned();
        if is_question(&a) {
            question_panel(ctx, aid, &a, href.clone(), item_id)
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
                        <kbd>{approval_key_hint(0)}</kbd>
                    </button>
                    <button
                        type="button"
                        class="btn small"
                        disabled=move || ctx.gate.with(|g| g.busy(&a3))
                        on:click=move |_| decide(ctx, a4.clone(), false, None)
                    >
                        "拒绝"
                        <kbd>{approval_key_hint(2)}</kbd>
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
            data-needs=it.needs_you().to_string()
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
                    title=it.raw.clone()
                    on:click=move |_| {
                        if !read {
                            ctx.mark.run(("read".into(), vec![id_read.clone()]));
                        }
                    }
                >
                    <strong>{it.title.clone()}</strong>
                    {(!it.body.is_empty() && !body_in_panel)
                        .then(|| view! { <span class="inbox-body">{it.body.clone()}</span> })}
                </a>
                {panel}
                {asking}
                {denied}
                {acts}
            </div>
            <small class="inbox-meta">
                {meta(&it.label, &it.created)}
                {(!it.resolved.is_empty())
                    .then(|| {
                        view! {
                            <Status
                                tone=Tone::Other
                                label=format!("{resolved_label} · {}", fmt::ago(&it.resolved))
                            />
                        }
                    })}
            </small>
        </div>
    }
}

fn inbox_menu(ctx: Ctx, item: Item, pending: Option<Value>) -> Vec<Vec<RowAction>> {
    let (id_read, id_arch) = (item.id.clone(), item.id.clone());
    let (read, archived) = (item.read, item.archived);
    let approval = pending.filter(|a| !is_question(a));
    let ws = item.workspace.clone();
    let app = use_app();
    let failing = failure_actions(&item)
        .map(|f| {
            let (fws, session) = (f.workspace.clone(), f.session.clone());
            let href = f.node_href();
            let label = f.primary();
            let mut acts = vec![RowAction::new(label, move || {
                let (ws, session) = (f.workspace.clone(), f.session.clone());
                failure::act(app, &f, move || retry_workspace(app, ws, session));
            })];
            if let Some(href) = href {
                acts.push(RowAction {
                    label: "打开机器页".into(),
                    hint: None,
                    danger: false,
                    disabled: false,
                    href: Some(href),
                    run: None,
                });
            }
            acts.push(RowAction::new("标为已处理", move || {
                failure::mark_handled(
                    app,
                    fws.clone(),
                    Some(session.clone()).filter(|s| !s.is_empty()),
                    false,
                );
            }));
            acts
        })
        .unwrap_or_default();
    let denied = if item.open_denial() {
        let (a, n) = (item.clone(), item.clone());
        vec![
            RowAction::new("允许这一次并重试", move || {
                decide_denial(ctx, &a, true)
            }),
            RowAction::new("不用了", move || decide_denial(ctx, &n, false)),
        ]
    } else if item.asking() {
        let id = item.id.clone();
        vec![RowAction::new("标为已处理", move || {
            resolve_item(id.clone(), move || refresh(ctx.rev));
        })]
    } else {
        Vec::new()
    };
    vec![
        failing,
        denied,
        approval
            .map(|a| {
                let (a1, a2, a3) = (a.clone(), a.clone(), a);
                let (w1, w2, w3) = (ws.clone(), ws.clone(), ws);
                vec![
                    RowAction::new("允许", move || pick_approval(ctx, "1", &a1, w1.clone()))
                        .hint(approval_key_hint(0)),
                    RowAction::new("总是允许", move || {
                        pick_approval(ctx, "2", &a2, w2.clone())
                    })
                    .hint(approval_key_hint(1)),
                    RowAction::new("拒绝", move || pick_approval(ctx, "3", &a3, w3.clone()))
                        .hint(approval_key_hint(2)),
                ]
            })
            .unwrap_or_default(),
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
