use super::work_shared::*;
use crate::components::dialog::{self, Choice};
use crate::components::modal::Modal;
use crate::components::page_head::{MenuItem, MoreMenu, PageHead};
use crate::components::status::{EmptyState, InlineError, LoadingState, StateOf};
use crate::components::toast::toast;
use crate::realtime::use_bus;
use crate::{api, app_state::use_app, fmt};
use blazar_core_types::api::{ServerEvent, valid_bot_handle as valid_handle};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;
use serde_json::{Value, json};

pub fn bots_revision() -> RwSignal<u32> {
    let rev = RwSignal::new(0u32);
    let bus = use_bus();
    let id = bus.subscribe(move |e| {
        if matches!(e, ServerEvent::BotsChanged | ServerEvent::FactsChanged) {
            rev.try_update(|n| *n = n.wrapping_add(1));
        }
    });
    on_cleanup(move || bus.unsubscribe(id));
    Effect::new(move |_| {
        bus.resyncs.track();
        rev.update(|n| *n = n.wrapping_add(1));
    });
    rev
}

fn role_label(role: &str) -> &'static str {
    if role == "lead" { "总控" } else { "执行" }
}

fn pill(state: &str) -> AnyView {
    view! { <StateOf state=state.to_owned() /> }.into_any()
}

fn lifetime_chip(b: &Value) -> Option<AnyView> {
    if b["temporary"].as_bool() != Some(true) {
        return None;
    }
    let hours = b["idle_ttl_secs"].as_i64().unwrap_or(86_400) / 3600;
    let title = format!("临时 bot：闲置 {hours} 小时后自动归档");
    Some(
        view! {
            <span title=title>"临时"</span>
        }
        .into_any(),
    )
}

fn mail_status(s: &str) -> &'static str {
    match s {
        "queued" => "排队中",
        "delivered" => "处理中",
        "done" => "已处理",
        "failed" => "失败",
        "expired" => "已作废",
        _ => "",
    }
}

fn from_label(m: &Value, bots: &[Value]) -> String {
    match s(m, "from_kind").as_str() {
        "user" => "你".into(),
        "bot" => {
            let id = s(m, "from_ref");
            bots.iter()
                .find(|b| s(b, "id") == id)
                .map_or_else(|| "bot".to_owned(), |b| format!("bot:{}", s(b, "handle")))
        }
        "rule" => format!("规则 {}", s(m, "from_ref")),
        "feishu" => "飞书".into(),
        "schedule" => "定时".into(),
        _ => "系统".into(),
    }
}

async fn post(path: &str, body: Value) -> Result<Value, api::ApiError> {
    api::send::<Value>("POST", path, &body).await
}

#[component]
pub fn BotsPage() -> impl IntoView {
    let rev = bots_revision();
    let app = use_app();
    let creating = RwSignal::new(false);
    let archived = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.track();
        let archived = archived.get();
        async move {
            api::get::<Value>(if archived {
                "/api/bots?archived=true"
            } else {
                "/api/bots?recent=1"
            })
            .await
        }
    });
    let fleet = LocalResource::new(move || {
        rev.track();
        async move { api::get::<Value>("/api/fleet/status").await }
    });
    let act = move |id: String, suffix: &'static str| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            if let Err(e) = post(&format!("/api/bots/{}/{suffix}", api::enc(&id)), json!({})).await
            {
                toast(e.to_string());
            }
            refresh(rev);
            app.load_bots();
            busy.try_set(false);
        });
    };
    view! {
        <div class="page work-page bots-page">
            <PageHead title="bot">
                <button type="button" class="btn" on:click=move |_| archived.update(|a| *a = !*a)>
                    {move || if archived.get() { "返回在用的" } else { "查看已归档" }}
                </button>
                <button type="button" class="btn primary" on:click=move |_| creating.set(true)>
                    "＋ 新建 bot"
                </button>
            </PageHead>
            {move || {
                fleet
                    .get()
                    .and_then(|r| r.ok())
                    .map(|f| {
                        let nodes: Vec<_> = app
                            .nodes()
                            .into_iter()
                            .filter(|n| n.is_usable())
                            .collect();
                        let online = nodes.iter().filter(|n| n.status == "online").count();
                        let alerts = arr(&f, "alerts");
                        let facts = arr(&f, "unhealthy_facts");
                        view! {
                            <div class="bots-summary muted">
                                {format!("{online}/{} 台机器在线", nodes.len())}
                                {(!alerts.is_empty())
                                    .then(|| format!(" · {} 条待处理通知", alerts.len()))}
                                {(!facts.is_empty())
                                    .then(|| format!(" · {} 条事实异常", facts.len()))}
                            </div>
                        }
                    })
            }}
            {move || match data.get() {
                None => view! { <LoadingState text="读取 bot…" /> }.into_any(),
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
                    let bots = arr(&r, "bots");
                    let showing_archived = archived.get_untracked();
                    if bots.is_empty() {
                        return if showing_archived {
                            view! {
                                <EmptyState
                                    title="没有归档的 bot"
                                    detail="临时 bot 闲置到期后会出现在这里，可以恢复。"
                                />
                            }
                                .into_any()
                        } else {
                            view! {
                                <EmptyState
                                    title="还没有 bot"
                                    detail="长期在线、负责固定工作的智能体，随时可以给它派活。"
                                />
                            }
                                .into_any()
                        };
                    }
                    view! {
                        <div class="bots-grid">
                            {bots
                                .into_iter()
                                .map(|b| {
                                    let id = s(&b, "id");
                                    let href = format!("/bots/{}", api::enc(&id));
                                    let state = s(&b, "state");
                                    let paused = b["paused"].as_bool() == Some(true);
                                    let is_archived = b["archived"].as_bool() == Some(true);
                                    let temporary = b["temporary"].as_bool() == Some(true);
                                    let pending = b["pending_mail"].as_u64().unwrap_or(0);
                                    let last = s(&b, "last_instruction");
                                    let claims = arr(&b, "claims");
                                    let (id_toggle, id_promote, id_restore) = (
                                        id.clone(),
                                        id.clone(),
                                        id.clone(),
                                    );
                                    view! {
                                        <article class="card bot-card">
                                            <div class="bot-head">
                                                <a href=href.clone()>
                                                    <strong>{s(&b, "display_name")}</strong>
                                                </a>
                                                <span class="mono muted">{s(&b, "handle")}</span>
                                                <span class="gchip">{role_label(&s(&b, "role"))}</span>
                                                <span class="gchip">{s(&b, "priority")}</span>
                                                {lifetime_chip(&b)}
                                                <span class="grow"></span>
                                                {pill(&state)}
                                            </div>
                                            <div class="bot-line muted">
                                                {if is_archived {
                                                    format!(
                                                        "归档于 {}",
                                                        b["archived_at"].as_str().map(fmt::ago).unwrap_or_default(),
                                                    )
                                                } else if pending > 0 {
                                                    format!("{pending} 封邮件待处理")
                                                } else {
                                                    "没有待处理邮件".into()
                                                }}
                                                {b["hot"].as_bool().filter(|h| *h).map(|_| " · 保持在线")}
                                                {b["last_active_at"]
                                                    .as_str()
                                                    .map(|t| format!(" · 最近活动 {}", fmt::ago(t)))}
                                            </div>
                                            {(!last.is_empty())
                                                .then(|| {
                                                    view! {
                                                        <div class="bot-line">
                                                            <span class="muted">"最后指令 "</span>
                                                            {last}
                                                        </div>
                                                    }
                                                })}
                                            {(!claims.is_empty())
                                                .then(|| {
                                                    view! {
                                                        <div class="bot-line muted">
                                                            "认领 "
                                                            {claims
                                                                .iter()
                                                                .map(|c| format!("{} {}", fmt::machine(&s(c, "node")), s(c, "scope")))
                                                                .collect::<Vec<_>>()
                                                                .join("，")}
                                                        </div>
                                                    }
                                                })}
                                            <div class="row-actions">
                                                <a class="btn small" href=href>
                                                    "打开"
                                                </a>
                                                {if is_archived {
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class="btn small ghost"
                                                            disabled=move || busy.get()
                                                            on:click=move |_| act(id_restore.clone(), "restore")
                                                        >
                                                            "恢复"
                                                        </button>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class="btn small ghost"
                                                            disabled=move || busy.get()
                                                            on:click=move |_| act(
                                                                id_toggle.clone(),
                                                                if paused { "resume" } else { "pause" },
                                                            )
                                                        >
                                                            {if paused { "恢复" } else { "暂停" }}
                                                        </button>
                                                        {temporary
                                                            .then(|| {
                                                                view! {
                                                                    <button
                                                                        type="button"
                                                                        class="btn small ghost"
                                                                        disabled=move || busy.get()
                                                                        on:click=move |_| act(id_promote.clone(), "promote")
                                                                    >
                                                                        "转为固定"
                                                                    </button>
                                                                }
                                                            })}
                                                    }
                                                        .into_any()
                                                }}
                                            </div>
                                        </article>
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                        .into_any()
                }
            }}
            <Show when=move || creating.get()>
                <NewBotForm on_close=Callback::new(move |_| {
                    creating.set(false);
                    refresh(rev);
                    app.load_bots();
                }) />
            </Show>
        </div>
    }
}

#[component]
fn NewBotForm(on_close: Callback<()>) -> impl IntoView {
    let handle = RwSignal::new(String::new());
    let display = RwSignal::new(String::new());
    let duty = RwSignal::new(String::new());
    let agent = RwSignal::new(String::new());
    let runtime = RwSignal::new("claude".to_owned());
    let role = RwSignal::new("worker".to_owned());
    let priority = RwSignal::new("P1".to_owned());
    let lifetime = RwSignal::new("temporary:86400".to_owned());
    let home = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let agents =
        LocalResource::new(|| async { api::get::<Vec<Value>>("/api/agent-profiles").await });
    let handle_ok = move || valid_handle(&handle.get());
    let ready = move || handle_ok() && (!duty.get().trim().is_empty() || !agent.get().is_empty());
    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        let h = handle.get_untracked().trim().to_owned();
        if !valid_handle(&h) {
            error.set(Some("标识只能用小写字母、数字和连字符，1–32 个字符".into()));
            return;
        }
        let d = duty.get_untracked().trim().to_owned();
        let a = agent.get_untracked();
        if d.is_empty() && a.is_empty() {
            error.set(Some("写一句它负责什么，或者选一个已有的档案".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let (life, ttl) = lifetime
                .get_untracked()
                .split_once(':')
                .map(|(l, t)| (l.to_owned(), t.parse::<i64>().unwrap_or(0)))
                .unwrap_or(("fixed".into(), 0));
            let mut body = json!({
                "handle": h,
                "role": role.get_untracked(),
                "priority": priority.get_untracked(),
                "lifetime": life,
            });
            if ttl > 0 {
                body["idle_ttl_secs"] = json!(ttl);
            }
            if a.is_empty() {
                body["duty"] = json!(d);
                body["runtime"] = json!(runtime.get_untracked());
            } else {
                body["agent_id"] = json!(a);
            }
            let dn = display.get_untracked().trim().to_owned();
            if !dn.is_empty() {
                body["display_name"] = json!(dn);
            }
            let p = home.get_untracked().trim().to_owned();
            if !p.is_empty() {
                body["home_path"] = json!(p);
            }
            match post("/api/bots", body).await {
                Ok(_) => {
                    toast("已创建，给它派活就会开始工作");
                    on_close.run(());
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.try_set(false);
        });
    };
    view! {
        <Modal label="新建 bot" on_close=Callback::new(move |_| on_close.run(()))>
            <h3>"新建 bot"</h3>
            <label class="field">
                "标识"
                <input
                    data-modal-initial-focus=""
                    maxlength="32"
                    placeholder="trainer"
                    aria-label="标识"
                    aria-describedby="new-bot-handle-rule"
                    prop:value=move || handle.get()
                    on:input=move |e| handle.set(event_target_value(&e))
                />
                <small
                    id="new-bot-handle-rule"
                    class="muted"
                    class:bad=move || !handle.get().is_empty() && !handle_ok()
                >
                    "小写字母、数字和连字符，1–32 个字符，不能以连字符开头或结尾"
                </small>
            </label>
            <label class="field">
                {move || {
                    if agent.get().is_empty() {
                        "它负责什么"
                    } else {
                        "它负责什么（已选档案，可以不写）"
                    }
                }}
                <textarea
                    rows="3"
                    placeholder="例如：盯着 gpu-0 上的训练队列，每小时汇报 loss，异常就发邮件给总控。"
                    prop:value=move || duty.get()
                    on:input=move |e| duty.set(event_target_value(&e))
                ></textarea>
            </label>
            <label class="field">
                "生命周期" <select on:change=move |e| lifetime.set(event_target_value(&e))>
                    <option value="temporary:86400" selected=true>
                        "临时 · 闲置 1 天自动归档"
                    </option>
                    <option value="temporary:14400">"临时 · 闲置 4 小时自动归档"</option>
                    <option value="fixed:0">"固定 · 一直留着"</option>
                </select>
            </label>
            <details class="runtime-details">
                <summary>"更多设置"</summary>
                <label class="field">
                    "运行时" <select on:change=move |e| runtime.set(event_target_value(&e))>
                        <option value="claude" selected=true>
                            "Claude Code"
                        </option>
                        <option value="codex">"Codex"</option>
                    </select>
                </label>
                <label class="field">
                    "改用已有的角色档案（选了就不用写职责）"
                    <select on:change=move |e| agent.set(event_target_value(&e))>
                        <option value="" selected=move || agent.get().is_empty()>
                            "不用"
                        </option>
                        {move || {
                            agents
                                .get()
                                .and_then(|r| r.ok())
                                .unwrap_or_default()
                                .into_iter()
                                .filter(|a| a["archived_at"].is_null())
                                .map(|a| {
                                    let id = s(&a, "id");
                                    let sel = id.clone();
                                    view! {
                                        <option value=id selected=move || agent.get() == sel>
                                            {format!("{} · {}", s(&a, "name"), s(&a, "runtime"))}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
                <label class="field">
                    "分工" <select on:change=move |e| role.set(event_target_value(&e))>
                        <option value="worker" selected=true>
                            "执行 · 负责具体工作"
                        </option>
                        <option value="lead">
                            "总控 · 可以给其他 bot 派活"
                        </option>
                    </select>
                </label>
                <label class="field">
                    "优先级" <select on:change=move |e| priority.set(event_target_value(&e))>
                        <option value="P0">"P0 · 真机这类不能等的"</option>
                        <option value="P1" selected=true>
                            "P1 · 日常"
                        </option>
                        <option value="P2">"P2 · 额度紧张时让路"</option>
                    </select>
                </label>
                <label class="field">
                    "工作目录（在本机，留空用 ~/blazar-bots/<标识>）"
                    <input
                        class="mono"
                        placeholder="/Users/me/blazar-bots/trainer"
                        prop:value=move || home.get()
                        on:input=move |e| home.set(event_target_value(&e))
                    />
                </label>
            </details>
            {move || error.get().map(|message| view! { <InlineError message /> })}
            <div class="dlg-foot">
                <span class="muted small grow">
                    "创建后它会在本机拥有自己的工作目录；收到工作时才开始运行、消耗额度。"
                </span>
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
                    disabled=move || busy.get() || !ready()
                    on:click=move |_| submit()
                >
                    {move || if busy.get() { "创建中…" } else { "创建" }}
                </button>
            </div>
        </Modal>
    }
}

#[component]
pub fn BotPage() -> impl IntoView {
    let params = use_params_map();
    let id = Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default()));
    let rev = bots_revision();
    let app = use_app();
    let busy = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.track();
        let id = id.get();
        async move { api::get::<Value>(&format!("/api/bots/{}", api::enc(&id))).await }
    });
    let all_bots = LocalResource::new(move || {
        rev.track();
        async move {
            api::get::<Value>("/api/bots")
                .await
                .map(|r| arr(&r, "bots"))
                .unwrap_or_default()
        }
    });
    let body = RwSignal::new(String::new());
    let subject = RwSignal::new(String::new());
    let urgent = RwSignal::new(false);
    let claim_scope = RwSignal::new(String::new());
    let claim_node = RwSignal::new("local".to_owned());
    let claim_mode = RwSignal::new("exclusive".to_owned());
    let call = move |method: &'static str, path: String, payload: Value, done: &'static str| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>(method, &path, &payload).await {
                Ok(_) => {
                    if !done.is_empty() {
                        toast(done);
                    }
                }
                Err(e) => toast(e.to_string()),
            }
            refresh(rev);
            app.load_bots();
            busy.try_set(false);
        });
    };
    let send_mail = move || {
        let text = body.get_untracked().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let mut payload = json!({ "body": text, "priority": if urgent.get_untracked() { "urgent" } else { "normal" } });
        let sub = subject.get_untracked().trim().to_owned();
        if !sub.is_empty() {
            payload["subject"] = json!(sub);
        }
        body.set(String::new());
        subject.set(String::new());
        call(
            "POST",
            format!("/api/bots/{}/mail", api::enc(&id.get_untracked())),
            payload,
            "已发出，bot 会被唤醒",
        );
    };
    let add_claim = move || {
        let scope = claim_scope.get_untracked().trim().to_owned();
        if scope.is_empty() {
            return;
        }
        claim_scope.set(String::new());
        call(
            "POST",
            "/api/claims".into(),
            json!({ "bot": id.get_untracked(), "scope": scope, "node": claim_node.get_untracked(), "mode": claim_mode.get_untracked() }),
            "已认领",
        );
    };
    view! {
        <div class="page work-page bot-page">
            {move || match data.get() {
                None => view! { <LoadingState text="读取 bot…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=e.to_string()
                            retry=Callback::new(move |_| refresh(rev))
                        />
                    }
                        .into_any()
                }
                Some(Ok(b)) => {
                    let bots = all_bots.get().unwrap_or_default();
                    let bid = s(&b, "id");
                    let paused = b["paused"].as_bool() == Some(true);
                    let hot = b["hot"].as_bool() == Some(true);
                    let is_archived = b["archived"].as_bool() == Some(true);
                    let temporary = b["temporary"].as_bool() == Some(true);
                    let idle_hours = b["idle_ttl_secs"].as_i64().unwrap_or(86_400) / 3600;
                    let home = s(&b, "home_ws_id");
                    let run_ws = s(&b, "run_ws_id");
                    let (b1, b2, b3, b4, b5, b6) = (
                        bid.clone(),
                        bid.clone(),
                        bid.clone(),
                        bid.clone(),
                        bid.clone(),
                        bid.clone(),
                    );
                    let nodes = app.nodes();
                    let state = s(&b, "state");
                    view! {
                        <PageHead
                            parent=("bot", "/bots")
                            title=s(&b, "display_name")
                            status=move || pill(&state)
                        >
                            <a class="btn" href=format!("/w/{}", api::enc(&run_ws))>
                                "打开对话"
                            </a>
                            {if is_archived {
                                view! {
                                    <button
                                        type="button"
                                        class="btn primary"
                                        disabled=move || busy.get()
                                        on:click=move |_| call(
                                            "POST",
                                            format!("/api/bots/{}/restore", api::enc(&b5)),
                                            json!({}),
                                            "已恢复",
                                        )
                                    >
                                        "恢复"
                                    </button>
                                }
                                    .into_any()
                            } else {
                                let mut items = Vec::new();
                                if temporary {
                                    items.push(MenuItem::new(
                                        "转为固定",
                                        move || call(
                                            "POST",
                                            format!("/api/bots/{}/promote", api::enc(&b6)),
                                            json!({}),
                                            "已转为固定 bot，不再自动归档",
                                        ),
                                    ));
                                }
                                items.push(MenuItem::new(
                                    if hot { "结束保持在线" } else { "保持在线 30 分钟" },
                                    move || call(
                                        "PUT",
                                        format!("/api/bots/{}", api::enc(&b1)),
                                        json!({ "hot_for_secs": if hot { 0 } else { 1800 } }),
                                        if hot {
                                            "已结束保持在线"
                                        } else {
                                            "接下来 30 分钟保持在线，新消息会立即送达"
                                        },
                                    ),
                                ));
                                items.push(MenuItem::new(
                                    if paused { "恢复" } else { "暂停" },
                                    move || call(
                                        "POST",
                                        format!(
                                            "/api/bots/{}/{}",
                                            api::enc(&b2),
                                            if paused { "resume" } else { "pause" },
                                        ),
                                        json!({}),
                                        "",
                                    ),
                                ));
                                items.push(MenuItem::danger(
                                    "归档",
                                    move || {
                                        let id = b3.clone();
                                        spawn_local(async move {
                                            if dialog::ask(
                                                    "归档 bot",
                                                    "归档后它不再接收新工作，排队的工作会作废；工作目录和对话历史保留，可以再恢复。",
                                                    vec![Choice::plain("取消"), Choice::danger("归档")],
                                                )
                                                .await == Some(1)
                                            {
                                                if let Err(e) = api::send::<
                                                    Value,
                                                >(
                                                        "DELETE",
                                                        &format!("/api/bots/{}", api::enc(&id)),
                                                        &json!({}),
                                                    )
                                                    .await
                                                {
                                                    toast(e.to_string());
                                                } else {
                                                    let _ = window().location().set_href("/bots");
                                                }
                                            }
                                        });
                                    },
                                ));
                                view! {
                                    <MoreMenu
                                        label="更多操作".to_owned()
                                        items
                                        disabled=Signal::derive(move || busy.get())
                                    />
                                }
                                    .into_any()
                            }}
                        </PageHead>
                        <div class="page-meta muted">
                            <span class="mono">{s(&b, "handle")}</span>
                            <span>{role_label(&s(&b, "role"))}</span>
                            <span>{s(&b, "priority")}</span>
                            {lifetime_chip(&b)}
                        </div>
                        {temporary
                            .then(|| {
                                view! {
                                    <div class="muted bot-line">
                                        {format!(
                                            "临时 bot：闲置 {idle_hours} 小时没有邮件、没有会话就自动归档。",
                                        )}
                                    </div>
                                }
                            })}
                        {(!s(&b, "status_note").is_empty())
                            .then(|| {
                                view! { <div class="muted bot-line">{s(&b, "status_note")}</div> }
                            })}
                        <div class="work-layout">
                            <div class="work-main">
                                <section class="card pad">
                                    <h3>"派活 / 发邮件"</h3>
                                    <label class="work-field">
                                        <span>"主题（可省）"</span>
                                        <input
                                            class="input"
                                            prop:value=move || subject.get()
                                            on:input=move |e| subject.set(event_target_value(&e))
                                        />
                                    </label>
                                    <label class="work-field">
                                        <span>"正文"</span>
                                        <textarea
                                            class="input"
                                            rows=4
                                            placeholder="写给它的指令。它空闲时会被唤醒；勾紧急会打断它正在跑的一轮。"
                                            prop:value=move || body.get()
                                            on:input=move |e| body.set(event_target_value(&e))
                                        ></textarea>
                                    </label>
                                    <div class="row-actions">
                                        <label class="muted">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || urgent.get()
                                                on:change=move |e| urgent.set(event_target_checked(&e))
                                            />
                                            " 紧急"
                                        </label>
                                        <span class="grow"></span>
                                        <button
                                            type="button"
                                            class="btn primary small"
                                            disabled=move || {
                                                busy.get() || is_archived
                                                    || body.with(|t| t.trim().is_empty())
                                            }
                                            on:click=move |_| send_mail()
                                        >
                                            "发送"
                                        </button>
                                    </div>
                                </section>
                                <section class="card pad">
                                    <h3>"收件箱"</h3>
                                    {
                                        let mail = arr(&b, "mailbox");
                                        if mail.is_empty() {
                                            view! { <div class="muted">"还没有邮件。"</div> }
                                                .into_any()
                                        } else {
                                            view! {
                                                <div class="work-inbox">
                                                    {mail
                                                        .into_iter()
                                                        .map(|m| {
                                                            let status = s(&m, "status");
                                                            let who = from_label(&m, &bots);
                                                            let text = s(&m, "body");
                                                            let sub = s(&m, "subject");
                                                            view! {
                                                                <article
                                                                    class="work-inbox-row"
                                                                    class:work-unread=status == "queued"
                                                                >
                                                                    <div class="work-inbox-main">
                                                                        <strong>
                                                                            {if sub.is_empty() {
                                                                                who.clone()
                                                                            } else {
                                                                                format!("{who} · {sub}")
                                                                            }}
                                                                        </strong>
                                                                        <span class="bot-mail-body">{text}</span>
                                                                        <small class="muted">
                                                                            {format!(
                                                                                "{} · {}{}",
                                                                                mail_status(&status),
                                                                                fmt::ago(&s(&m, "created_at")),
                                                                                if s(&m, "priority") == "urgent" {
                                                                                    " · 紧急"
                                                                                } else {
                                                                                    ""
                                                                                },
                                                                            )}
                                                                        </small>
                                                                    </div>
                                                                </article>
                                                            }
                                                        })
                                                        .collect_view()}
                                                </div>
                                            }
                                                .into_any()
                                        }
                                    }
                                </section>
                                {
                                    let sent = arr(&b, "sent");
                                    (!sent.is_empty())
                                        .then(|| {
                                            view! {
                                                <section class="card pad">
                                                    <h3>"它发出的邮件"</h3>
                                                    <div class="work-inbox">
                                                        {sent
                                                            .into_iter()
                                                            .map(|m| {
                                                                let to = s(&m, "to_bot");
                                                                let name = bots
                                                                    .iter()
                                                                    .find(|x| s(x, "id") == to)
                                                                    .map_or_else(
                                                                        || "bot".to_owned(),
                                                                        |x| format!("bot:{}", s(x, "handle")),
                                                                    );
                                                                view! {
                                                                    <article class="work-inbox-row">
                                                                        <div class="work-inbox-main">
                                                                            <strong>
                                                                                {format!(
                                                                                    "→ {name}{}",
                                                                                    if s(&m, "subject").is_empty() {
                                                                                        String::new()
                                                                                    } else {
                                                                                        format!(" · {}", s(&m, "subject"))
                                                                                    },
                                                                                )}
                                                                            </strong>
                                                                            <span class="bot-mail-body">{s(&m, "body")}</span>
                                                                            <small class="muted">
                                                                                {format!(
                                                                                    "{} · {}",
                                                                                    mail_status(&s(&m, "status")),
                                                                                    fmt::ago(&s(&m, "created_at")),
                                                                                )}
                                                                            </small>
                                                                        </div>
                                                                    </article>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </div>
                                                </section>
                                            }
                                        })
                                }
                            </div>
                            <aside class="work-detail">
                                <div class="work-flow">
                                    <section class="card pad">
                                        <h3>"现状"</h3>
                                        <div class="work-kv">
                                            <span class="muted">"思考位置"</span>
                                            <span>
                                                {if s(&b, "brain") == "node" {
                                                    "工作机器"
                                                } else {
                                                    "本机"
                                                }}
                                            </span>
                                            <span class="muted">"机器"</span>
                                            <span>
                                                {format!(
                                                    "{}{}",
                                                    s(&b, "node"),
                                                    b["node_status"]
                                                        .as_str()
                                                        .map(|st| format!(" · {st}"))
                                                        .unwrap_or_default(),
                                                )}
                                            </span>
                                            <span class="muted">"家工作区"</span>
                                            <span>
                                                <a href=format!("/w/{}", api::enc(&home))>"打开"</a>
                                            </span>
                                            <span class="muted">"待处理"</span>
                                            <span>
                                                {format!("{} 封", b["pending_mail"].as_u64().unwrap_or(0))}
                                            </span>
                                            <span class="muted">"最近活动"</span>
                                            <span>
                                                {b["last_active_at"]
                                                    .as_str()
                                                    .map_or_else(|| "—".to_owned(), fmt::ago)}
                                            </span>
                                        </div>
                                        {(!s(&b, "last_instruction").is_empty())
                                            .then(|| {
                                                view! {
                                                    <div class="bot-line">
                                                        <span class="muted">"最后指令 "</span>
                                                        {s(&b, "last_instruction")}
                                                    </div>
                                                }
                                            })}
                                        {
                                            let steps = arr(&b, "recent");
                                            (!steps.is_empty())
                                                .then(|| {
                                                    view! {
                                                        <ul class="bot-steps">
                                                            {steps
                                                                .into_iter()
                                                                .map(|x| {
                                                                    view! { <li>{x.as_str().unwrap_or("").to_owned()}</li> }
                                                                })
                                                                .collect_view()}
                                                        </ul>
                                                    }
                                                })
                                        }
                                    </section>
                                    <section class="card pad">
                                        <h3>"认领的资源"</h3>
                                        {
                                            let claims = arr(&b, "claims");
                                            if claims.is_empty() {
                                                view! {
                                                    <div class="muted">"还没认领任何资源。"</div>
                                                }
                                                    .into_any()
                                            } else {
                                                view! {
                                                    <ul class="bot-claims">
                                                        {claims
                                                            .into_iter()
                                                            .map(|c| {
                                                                let cid = s(&c, "id");
                                                                view! {
                                                                    <li>
                                                                        <span class="mono">
                                                                            {format!("{} {}", fmt::machine(&s(&c, "node")), s(&c, "scope"))}
                                                                        </span>
                                                                        <span class="gchip">{crate::vocab::term(&s(&c, "mode")).to_owned()}</span>
                                                                        {c["hands"]
                                                                            .as_str()
                                                                            .map(|h| {
                                                                                view! { <span class="muted mono">{h.to_owned()}</span> }
                                                                            })}
                                                                        <span class="grow"></span>
                                                                        <button
                                                                            type="button"
                                                                            class="btn small ghost"
                                                                            disabled=move || busy.get()
                                                                            on:click=move |_| call(
                                                                                "DELETE",
                                                                                format!("/api/claims/{}", api::enc(&cid)),
                                                                                json!({}),
                                                                                "已释放",
                                                                            )
                                                                        >
                                                                            "释放"
                                                                        </button>
                                                                    </li>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </ul>
                                                }
                                                    .into_any()
                                            }
                                        }
                                        <div class="bot-claim-form">
                                            <input
                                                class="input mono"
                                                placeholder="gpu:0,1 · dir:/data · repo"
                                                prop:value=move || claim_scope.get()
                                                on:input=move |e| claim_scope.set(event_target_value(&e))
                                                on:keydown=move |e| {
                                                    if e.key() == "Enter" {
                                                        add_claim()
                                                    }
                                                }
                                            />
                                            <select
                                                class="input"
                                                on:change=move |e| claim_node.set(event_target_value(&e))
                                            >
                                                <option value="local" selected=true>
                                                    "本机"
                                                </option>
                                                {nodes
                                                    .into_iter()
                                                    .filter(|n| n.name != "local")
                                                    .map(|n| {
                                                        view! {
                                                            <option value=n.name.clone()>{fmt::machine(&n.name)}</option>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </select>
                                            <select
                                                class="input"
                                                on:change=move |e| claim_mode.set(event_target_value(&e))
                                            >
                                                <option value="exclusive" selected=true>
                                                    "独占"
                                                </option>
                                                <option value="shared">"共享"</option>
                                                <option value="read">"只读"</option>
                                            </select>
                                            <button
                                                type="button"
                                                class="btn small"
                                                disabled=move || busy.get()
                                                on:click=move |_| add_claim()
                                            >
                                                "认领"
                                            </button>
                                        </div>
                                    </section>
                                    <section class="card pad">
                                        <h3>"线程"</h3>
                                        <ul class="bot-claims">
                                            {arr(&b, "all_threads")
                                                .into_iter()
                                                .map(|t| {
                                                    let tid = s(&t, "id");
                                                    let closed = !s(&t, "closed_at").is_empty();
                                                    let b7 = b4.clone();
                                                    view! {
                                                        <li>
                                                            <span>
                                                                {format!(
                                                                    "{}{}",
                                                                    s(&t, "kind"),
                                                                    if s(&t, "title").is_empty() {
                                                                        String::new()
                                                                    } else {
                                                                        format!(" · {}", s(&t, "title"))
                                                                    },
                                                                )}
                                                            </span>
                                                            {t["thread_id"]
                                                                .as_str()
                                                                .map(|_| {
                                                                    view! { <span class="gchip ok">"已续接"</span> }
                                                                })}
                                                            {closed
                                                                .then(|| view! { <span class="gchip">"已关闭"</span> })}
                                                            <span class="grow"></span>
                                                            {(!closed)
                                                                .then(|| {
                                                                    view! {
                                                                        <button
                                                                            type="button"
                                                                            class="btn small ghost"
                                                                            disabled=move || busy.get()
                                                                            on:click=move |_| call(
                                                                                "POST",
                                                                                format!(
                                                                                    "/api/bots/{}/threads/{}/close",
                                                                                    api::enc(&b7),
                                                                                    api::enc(&tid),
                                                                                ),
                                                                                json!({}),
                                                                                "线程已关闭，下一封邮件开新对话",
                                                                            )
                                                                        >
                                                                            "关闭"
                                                                        </button>
                                                                    }
                                                                })}
                                                        </li>
                                                    }
                                                })
                                                .collect_view()}
                                        </ul>
                                    </section>
                                    {
                                        let mem = arr(&b, "memories");
                                        (!mem.is_empty())
                                            .then(|| {
                                                view! {
                                                    <section class="card pad">
                                                        <h3>"它的 memory"</h3>
                                                        <ul class="bot-claims">
                                                            {mem
                                                                .into_iter()
                                                                .map(|m| {
                                                                    view! {
                                                                        <li>
                                                                            <span class="mono">{s(&m, "name")}</span>
                                                                            <span class="muted">{s(&m, "description")}</span>
                                                                        </li>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </ul>
                                                    </section>
                                                }
                                            })
                                    }
                                </div>
                            </aside>
                        </div>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}
