use super::work_shared::*;
use crate::action_gate::ActionGate;
use crate::components::empty::{self, Action, Blank};
use crate::components::group::ListGroup;
use crate::components::icon;
use crate::components::modal::Modal;
use crate::components::page_head::{MenuItem, MoreMenu, PageHead};
use crate::components::rows::{RowAction, RowKey, RowMenu, Rows};
use crate::components::status::{InlineError, LoadingState, StateOf};
use crate::components::toast::{toast, toast_error};
use crate::components::undo::{self, Request};
use crate::realtime::use_bus;
use crate::{api, app_state::use_app, bot_words as words, fmt};
use blazar_core_types::api::{ServerEvent, valid_bot_handle as valid_handle};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::{Value, json};

mod advanced;
use advanced::BotAdvanced;

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

pub(super) fn archive_bot(id: &str, name: &str, done: impl Fn() + 'static) {
    undo::remove_row(
        format!("已归档「{name}」"),
        id.to_owned(),
        Request::delete(format!("/api/bots/{}", api::enc(id))),
        done,
    );
}

async fn post(path: &str, body: Value) -> Result<Value, api::ApiError> {
    api::send::<Value>("POST", path, &body).await
}

fn is_question(a: &Value) -> bool {
    crate::chat_model::tool_name(a["request"]["tool_name"].as_str().unwrap_or(""))
        == "AskUserQuestion"
}

#[component]
pub fn BotsPage() -> impl IntoView {
    let rev = bots_revision();
    let app = use_app();
    let creating = RwSignal::new(false);
    let advanced = RwSignal::new(false);
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
    let has_archived = LocalResource::new(move || {
        rev.track();
        async move {
            api::get::<Value>("/api/bots?archived=true")
                .await
                .is_ok_and(|v| !arr(&v, "bots").is_empty())
        }
    });
    let has_active = move || data.with(|d| matches!(d, Some(Ok(r)) if !arr(r, "bots").is_empty()));
    let nav = StoredValue::new(use_navigate());
    let rows = Rows::default();
    let shown = RwSignal::new(Vec::<Value>::new());
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
                toast_error("操作失败", &e);
            }
            refresh(rev);
            app.load_bots();
            busy.try_set(false);
        });
    };
    let open_bot = move |b: &Value| {
        let href = format!("/bots/{}", api::enc(&s(b, "id")));
        nav.with_value(|n| n(&href, Default::default()));
    };
    rows.listen(
        "article.bot-card",
        move || shown.try_with_untracked(Vec::len).unwrap_or(0),
        move |key, i| {
            let Some(b) = shown.try_with_untracked(|l| l.get(i).cloned()).flatten() else {
                return;
            };
            match key {
                RowKey::Open | RowKey::Inspect => open_bot(&b),
                RowKey::Delete => {
                    if b["archived"].as_bool() != Some(true) {
                        archive_bot(&s(&b, "id"), &s(&b, "display_name"), move || {
                            refresh(rev);
                            app.load_bots();
                        });
                    }
                }
            }
        },
    );
    let menu_of = move |b: Value| {
        let id = s(&b, "id");
        let name = s(&b, "display_name");
        let paused = b["paused"].as_bool() == Some(true);
        let gone = b["archived"].as_bool() == Some(true);
        let temporary = b["temporary"].as_bool() == Some(true);
        let (id_toggle, id_promote, id_restore, id_archive) =
            (id.clone(), id.clone(), id.clone(), id.clone());
        let mut middle = Vec::new();
        if gone {
            middle.push(RowAction::new("恢复", move || {
                act(id_restore.clone(), "restore")
            }));
        } else {
            middle.push(RowAction::new(
                if paused { "恢复" } else { "暂停" },
                move || act(id_toggle.clone(), if paused { "resume" } else { "pause" }),
            ));
            if temporary {
                middle.push(RowAction::new("转为固定", move || {
                    act(id_promote.clone(), "promote")
                }));
            }
        }
        vec![
            vec![RowAction::open(format!("/bots/{}", api::enc(&id)))],
            middle,
            if gone {
                Vec::new()
            } else {
                vec![RowAction::delete("归档", move || {
                    archive_bot(&id_archive, &name, move || {
                        refresh(rev);
                        app.load_bots();
                    });
                })]
            },
        ]
    };
    view! {
        <div class="page work-page bots-page">
            <PageHead title="bot">
                <Show when=move || archived.get() || has_archived.get() == Some(true)>
                    <button
                        type="button"
                        class="btn"
                        on:click=move |_| archived.update(|a| *a = !*a)
                    >
                        {move || if archived.get() { "返回在用的" } else { "查看已归档" }}
                    </button>
                </Show>
                <button
                    type="button"
                    class="btn"
                    aria-haspopup="dialog"
                    title="同时运行几个 bot、什么时候提醒"
                    on:click=move |_| advanced.set(true)
                >
                    "规则与上限…"
                </button>
                <button type="button" class="btn primary" on:click=move |_| creating.set(true)>
                    "新建 bot"
                </button>
            </PageHead>
            {move || {
                fleet
                    .get()
                    .filter(|_| has_active() && !archived.get())
                    .and_then(|r| r.ok())
                    .map(|f| {
                        let (count, waiting) = data
                            .with(|d| {
                                d.as_ref()
                                    .and_then(|r| r.as_ref().ok())
                                    .map(|r| {
                                        let bots = arr(r, "bots");
                                        let live = bots
                                            .iter()
                                            .filter(|b| b["archived"].as_bool() != Some(true))
                                            .count();
                                        let waiting = bots
                                            .iter()
                                            .filter(|b| s(b, "state") == "awaiting")
                                            .count();
                                        (live, waiting)
                                    })
                            })
                            .unwrap_or((0, 0));
                        let facts = arr(&f, "unhealthy_facts");
                        let late = (!facts.is_empty())
                            .then(|| {
                                let title = facts
                                    .iter()
                                    .map(|x| {
                                        let owner = s(x, "owner");
                                        if owner.is_empty() {
                                            s(x, "key")
                                        } else {
                                            format!("{owner}：{}", s(x, "key"))
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                // Only what's about the bots themselves; machines and notices have
                                // their own pages.
                                view! {
                                    <span title=format!(
                                        "这些 bot 该定时报一次状态，已经过了时间还没报：\n{title}",
                                    )>{format!(" · {} 项检查没跟上", facts.len())}</span>
                                }
                            });
                        view! {
                            <div class="bots-summary muted">
                                {format!("{count} 个 bot")}
                                {(waiting > 0).then(|| format!(" · {waiting} 个在等你"))}
                                {late}
                            </div>
                        }
                    })
            }}
            {move || match data.get() {
                None => view! { <LoadingState text="读取 bot…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError error=e.clone() retry=Callback::new(move |_| refresh(rev)) />
                    }
                        .into_any()
                }
                Some(Ok(r)) => {
                    let bots: Vec<Value> = arr(&r, "bots")
                        .into_iter()
                        .filter(|b| undo::shows(&s(b, "id")))
                        .collect();
                    let showing_archived = archived.get_untracked();
                    shown.set(bots.clone());
                    if bots.is_empty() {
                        return if showing_archived {
                            view! {
                                <Blank
                                    icon=icon::BOT
                                    title=empty::never("归档的 bot")
                                    detail="临时 bot 闲置到期后会归档到这里，可以恢复。"
                                    action=Action::new(
                                        "返回在用的",
                                        move || archived.set(false),
                                    )
                                />
                            }
                                .into_any()
                        } else {
                            view! {
                                <Blank
                                    icon=icon::BOT
                                    title=empty::never("bot")
                                    detail="bot 是长期在线、负责固定工作的智能体，随时可以给它派活。"
                                    action=Action::new("新建 bot", move || creating.set(true))
                                />
                            }
                                .into_any()
                        };
                    }
                    view! {
                        <ListGroup class="bots-list">
                            {bots
                                .into_iter()
                                .enumerate()
                                .map(|(i, b)| {
                                    let id = s(&b, "id");
                                    let href = format!("/bots/{}", api::enc(&id));
                                    let state = s(&b, "state");
                                    let paused = b["paused"].as_bool() == Some(true);
                                    let is_archived = b["archived"].as_bool() == Some(true);
                                    let temporary = b["temporary"].as_bool() == Some(true);
                                    let (label, tone) = crate::vocab::state(&state);
                                    let mut line = words::now(&b, fmt::ago);
                                    if let Some(c) = words::claims(&arr(&b, "claims")) {
                                        line = format!("{line} · {c}");
                                    }
                                    let line_title = words::claims_title(&b)
                                        .map_or_else(|| line.clone(), |c| format!("{line}\n{c}"));
                                    let name = s(&b, "display_name");
                                    let (id_toggle, id_promote, id_restore) = (
                                        id.clone(),
                                        id.clone(),
                                        id.clone(),
                                    );
                                    view! {
                                        <article
                                            class="bot-card lrow"
                                            tabindex="-1"
                                            data-index=i.to_string()
                                            data-sel=move || rows.selected(i).to_string()
                                            data-menu=move || rows.menu_on(i).to_string()
                                            on:focusin=move |_| rows.sel.set(Some(i))
                                            on:contextmenu=move |e| rows.open_menu(&e, i)
                                        >
                                            <span
                                                class="dot"
                                                data-tone=tone.key()
                                                role="img"
                                                aria-label=label
                                                title=label
                                            ></span>
                                            <div class="bot-main">
                                                <div class="bot-head">
                                                    <a
                                                        class="cell-link"
                                                        href=href
                                                        title=format!("{name} · 标识 {}", s(&b, "handle"))
                                                    >
                                                        <strong>{name.clone()}</strong>
                                                    </a>
                                                    {words::badges(&b)
                                                        .into_iter()
                                                        .map(|(text, title)| {
                                                            view! {
                                                                <span class="gchip" title=title>
                                                                    {text}
                                                                </span>
                                                            }
                                                        })
                                                        .collect_view()}
                                                </div>
                                                <div class="bot-line muted" title=line_title>
                                                    {line}
                                                </div>
                                            </div>
                                            <div class="bot-acts">
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
                        </ListGroup>
                    }
                        .into_any()
                }
            }}
            {move || {
                rows.menu
                    .get()
                    .and_then(|at| shown.with(|l| l.get(at.index).cloned()).map(|b| (at, b)))
                    .map(|(at, b)| {
                        view! {
                            <RowMenu
                                at
                                label=format!("{} 的操作", s(&b, "display_name"))
                                groups=menu_of(b)
                                on_close=Callback::new(move |()| rows.close_menu())
                            />
                        }
                    })
            }}
            <Show when=move || advanced.get()>
                <BotAdvanced on_close=Callback::new(move |()| advanced.set(false)) />
            </Show>
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
                    "Claude Code / Codex"
                    <select on:change=move |e| runtime.set(event_target_value(&e))>
                        <option value="claude" selected=true>
                            "Claude Code"
                        </option>
                        <option value="codex">"Codex"</option>
                    </select>
                </label>
                <label class="field">
                    "改用已有的智能体（选了就不用写职责）"
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
                        <option value="lead">"总控 · 可以给其他 bot 派活"</option>
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
    let nav = StoredValue::new(use_navigate());
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
    let gate = RwSignal::new(ActionGate::default());
    let appr_rev = RwSignal::new(0u32);
    let run_ws_id = Memo::new(move |_| {
        data.with(|d| match d {
            Some(Ok(b)) => s(b, "run_ws_id"),
            _ => String::new(),
        })
    });
    let approvals = LocalResource::new(move || {
        rev.track();
        appr_rev.track();
        let ws = run_ws_id.get();
        async move {
            if ws.is_empty() {
                return Vec::<Value>::new();
            }
            api::get::<Vec<Value>>(&format!("/api/approvals?workspace={}", api::enc(&ws)))
                .await
                .unwrap_or_default()
        }
    });
    let waiting = Memo::new(move |_| {
        approvals
            .get()
            .unwrap_or_default()
            .into_iter()
            .find(|a| !is_question(a))
    });
    let after = move || {
        let _ = appr_rev.try_update(|n| *n += 1);
        refresh(rev);
    };
    let pick = move |key: &str, a: &Value| {
        let id = s(a, "id");
        match key {
            "1" => decide_approval(gate, id, true, None, after),
            "2" => allow_always(
                gate,
                id,
                a["request"].clone(),
                run_ws_id.get_untracked(),
                after,
            ),
            _ => decide_approval(gate, id, false, None, after),
        }
    };
    let keys = window_event_listener(ev::keydown, move |e| {
        if !list_keys_apply(&e) || e.meta_key() || e.ctrl_key() || e.alt_key() {
            return;
        }
        let key = e.key();
        if !matches!(key.as_str(), "1" | "2" | "3") {
            return;
        }
        if let Some(a) = waiting.get_untracked() {
            e.prevent_default();
            pick(&key, &a);
        }
    });
    on_cleanup(move || keys.remove());
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
                Err(e) => toast_error("操作失败", &e),
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
            "已占用",
        );
    };
    let release = move |cid: String, what: String| {
        undo::remove_row(
            format!("已释放「{what}」，别的 bot 现在可以占用它"),
            cid.clone(),
            Request::delete(format!("/api/claims/{}", api::enc(&cid))).failing("没释放"),
            move || {
                refresh(rev);
                app.load_bots();
            },
        );
    };
    view! {
        <div class="page work-page bot-page">
            {move || match data.get() {
                None => view! { <LoadingState text="读取 bot…" /> }.into_any(),
                Some(Err(e)) => {
                    view! {
                        <InlineError error=e.clone() retry=Callback::new(move |_| refresh(rev)) />
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
                    let name = s(&b, "display_name");
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
                                    items
                                        .push(
                                            MenuItem::new(
                                                "转为固定",
                                                move || call(
                                                    "POST",
                                                    format!("/api/bots/{}/promote", api::enc(&b6)),
                                                    json!({}),
                                                    "已转为固定 bot，不再自动归档",
                                                ),
                                            ),
                                        );
                                }
                                items
                                    .push(
                                        MenuItem::new(
                                            if hot {
                                                "结束保持在线"
                                            } else {
                                                "保持在线 30 分钟"
                                            },
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
                                        ),
                                    );
                                items
                                    .push(
                                        MenuItem::new(
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
                                        ),
                                    );
                                items
                                    .push(
                                        MenuItem::danger(
                                            "归档",
                                            move || {
                                                let id = b3.clone();
                                                let name = name.clone();
                                                spawn_local(async move {
                                                    if crate::files_js::confirm_leave().await {
                                                        nav.with_value(|n| n("/bots", Default::default()));
                                                        archive_bot(&id, &name, move || app.load_bots());
                                                    }
                                                });
                                            },
                                        ),
                                    );
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
                        {
                            let chips = words::badges(&b);
                            (!chips.is_empty())
                                .then(|| {
                                    view! {
                                        <div class="page-meta">
                                            {chips
                                                .into_iter()
                                                .map(|(text, title)| {
                                                    view! {
                                                        <span class="gchip" title=title>
                                                            {text}
                                                        </span>
                                                    }
                                                })
                                                .collect_view()}
                                        </div>
                                    }
                                })
                        }
                        {(!s(&b, "status_note").is_empty())
                            .then(|| {
                                view! { <div class="muted bot-line">{s(&b, "status_note")}</div> }
                            })}
                        <div class="work-layout">
                            <div class="work-main">
                                <section class="card pad bot-now">
                                    <h3>"现在在做什么"</h3>
                                    {move || {
                                        waiting
                                            .get()
                                            .map(|a| {
                                                let root = app
                                                    .workspace(&run_ws_id.get())
                                                    .map(|w| w.path)
                                                    .unwrap_or_default();
                                                let (tool, what) = approval_summary(&a["request"], &root);
                                                let id = s(&a, "id");
                                                let (a1, a2, a3) = (a.clone(), a.clone(), a);
                                                let (i1, i2, i3) = (id.clone(), id.clone(), id);
                                                view! {
                                                    <div class="bot-approval" data-ui-guard="">
                                                        <strong>"等你批准"</strong>
                                                        <div class="bot-approval-cmd">
                                                            <span class="bot-approval-tool">{tool}</span>
                                                            <code>{what}</code>
                                                        </div>
                                                        <div class="row-actions">
                                                            <button
                                                                type="button"
                                                                class="btn small"
                                                                disabled=move || gate.with(|g| g.busy(&i1))
                                                                on:click=move |_| pick("1", &a1)
                                                            >
                                                                "允许"
                                                                <kbd>{approval_key_hint(0)}</kbd>
                                                            </button>
                                                            <button
                                                                type="button"
                                                                class="btn small"
                                                                disabled=move || gate.with(|g| g.busy(&i2))
                                                                on:click=move |_| pick("2", &a2)
                                                            >
                                                                "总是允许"
                                                                <kbd>{approval_key_hint(1)}</kbd>
                                                            </button>
                                                            <button
                                                                type="button"
                                                                class="btn small"
                                                                disabled=move || gate.with(|g| g.busy(&i3))
                                                                on:click=move |_| pick("3", &a3)
                                                            >
                                                                "拒绝"
                                                                <kbd>{approval_key_hint(2)}</kbd>
                                                            </button>
                                                        </div>
                                                    </div>
                                                }
                                            })
                                    }}
                                    <div class="bot-line">{words::now(&b, fmt::ago)}</div>
                                    <div class="bot-line muted">
                                        {format!(
                                            "{} 封邮件待处理 · 最近活动 {}",
                                            b["pending_mail"].as_u64().unwrap_or(0),
                                            b["last_active_at"]
                                                .as_str()
                                                .map_or_else(|| "—".to_owned(), fmt::ago),
                                        )}
                                    </div>
                                    {
                                        let last = s(&b, "last_instruction");
                                        (!last.is_empty() && s(&b, "state") != "running")
                                            .then(|| {
                                                view! {
                                                    <div class="bot-line">
                                                        <span class="muted">"最后指令 "</span>
                                                        {last}
                                                    </div>
                                                }
                                            })
                                    }
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
                                    <h3>"派活与邮件"</h3>
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
                                    <h4 class="bot-sub">"收到的"</h4>
                                    {
                                        let mail = arr(&b, "mailbox");
                                        if mail.is_empty() {
                                            view! {
                                                <Blank
                                                    icon=icon::MAIL
                                                    title=empty::never("邮件")
                                                    detail="给它派活或发邮件后，会出现在这里。"
                                                    inset=true
                                                />
                                            }
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
                                    {
                                        let sent = arr(&b, "sent");
                                        (!sent.is_empty())
                                            .then(|| {
                                                view! {
                                                    <details class="work-more bot-more">
                                                        <summary>
                                                            {format!("它发出的 {} 封", sent.len())}
                                                        </summary>
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
                                                    </details>
                                                }
                                            })
                                    }
                                </section>
                            </div>
                            <aside class="work-detail">
                                <div class="work-flow">
                                    <section class="card pad">
                                        <h3>"对话与记忆"</h3>
                                        {arr(&b, "all_threads")
                                            .is_empty()
                                            .then(|| {
                                                view! {
                                                    <Blank
                                                        icon=icon::CHAT
                                                        title=empty::never("对话")
                                                        detail="收到邮件后，它会为每件事开一段对话，下次接着往下做。"
                                                        inset=true
                                                    />
                                                }
                                            })}
                                        <ul class="bot-claims">
                                            {arr(&b, "all_threads")
                                                .into_iter()
                                                .map(|t| {
                                                    let tid = s(&t, "id");
                                                    let closed = !s(&t, "closed_at").is_empty();
                                                    let b7 = b4.clone();
                                                    let kind = match s(&t, "kind").as_str() {
                                                        "main" => "主线".to_owned(),
                                                        "task" => "任务".to_owned(),
                                                        other => other.to_owned(),
                                                    };
                                                    view! {
                                                        <li>
                                                            <span>
                                                                {if s(&t, "title").is_empty() {
                                                                    kind
                                                                } else {
                                                                    format!("{kind} · {}", s(&t, "title"))
                                                                }}
                                                            </span>
                                                            {t["thread_id"]
                                                                .as_str()
                                                                .map(|_| {
                                                                    view! { <span class="muted">"接着上次"</span> }
                                                                })}
                                                            {closed
                                                                .then(|| view! { <span class="muted">"已结束"</span> })}
                                                            <span class="grow"></span>
                                                            {(!closed)
                                                                .then(|| {
                                                                    view! {
                                                                        <button
                                                                            type="button"
                                                                            class="btn small ghost"
                                                                            title="下一封邮件开一段新对话"
                                                                            disabled=move || busy.get()
                                                                            on:click=move |_| call(
                                                                                "POST",
                                                                                format!(
                                                                                    "/api/bots/{}/threads/{}/close",
                                                                                    api::enc(&b7),
                                                                                    api::enc(&tid),
                                                                                ),
                                                                                json!({}),
                                                                                "这段对话已结束，下一封邮件开新对话",
                                                                            )
                                                                        >
                                                                            "结束"
                                                                        </button>
                                                                    }
                                                                })}
                                                        </li>
                                                    }
                                                })
                                                .collect_view()}
                                        </ul>
                                        {
                                            let mem = arr(&b, "memories");
                                            (!mem.is_empty())
                                                .then(|| {
                                                    view! {
                                                        <h4 class="bot-sub">"记忆"</h4>
                                                        <ul class="bot-claims">
                                                            {mem
                                                                .into_iter()
                                                                .map(|m| {
                                                                    view! {
                                                                        <li>
                                                                            <span>{s(&m, "name")}</span>
                                                                            <span class="muted">{s(&m, "description")}</span>
                                                                        </li>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </ul>
                                                    }
                                                })
                                        }
                                    </section>
                                    <section class="card pad">
                                        <h3>"资源与设置"</h3>
                                        {
                                            let claims = arr(&b, "claims");
                                            if claims.is_empty() {
                                                view! {
                                                    <p class="muted bot-line">
                                                        "没有占用资源。占用 GPU、目录或仓库后，别的 bot 就不会撞上。"
                                                    </p>
                                                }
                                                    .into_any()
                                            } else {
                                                view! {
                                                    <ul class="bot-claims">
                                                        {claims
                                                            .into_iter()
                                                            .map(|c| {
                                                                let cid = s(&c, "id");
                                                                let rid = cid.clone();
                                                                let what = words::claim(&c);
                                                                let detail = words::claim_detail(&c);
                                                                view! {
                                                                    <li
                                                                        title=detail
                                                                        style=move || {
                                                                            if undo::shows(&rid) { "" } else { "display: none" }
                                                                        }
                                                                    >
                                                                        <span>{what.clone()}</span>
                                                                        <span class="muted">
                                                                            {crate::vocab::term(&s(&c, "mode")).to_owned()}
                                                                        </span>
                                                                        <span class="grow"></span>
                                                                        <button
                                                                            type="button"
                                                                            class="btn small ghost"
                                                                            disabled=move || busy.get()
                                                                            on:click=move |_| release(cid.clone(), what.clone())
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
                                        <div class="work-kv">
                                            <span class="muted">"机器"</span>
                                            <span>
                                                {format!(
                                                    "{}{}",
                                                    fmt::machine(&s(&b, "node")),
                                                    b["node_status"]
                                                        .as_str()
                                                        .map(|st| format!(" · {}", crate::vocab::state(st).0))
                                                        .unwrap_or_default(),
                                                )}
                                            </span>
                                            <span class="muted">"工作目录"</span>
                                            <span>
                                                <a class="cell-link" href=format!("/w/{}", api::enc(&home))>
                                                    "打开"
                                                </a>
                                            </span>
                                            <span class="muted">"去留"</span>
                                            <span>
                                                {if temporary {
                                                    format!("临时 · 闲置 {idle_hours} 小时自动归档")
                                                } else {
                                                    "固定 · 一直留着".to_owned()
                                                }}
                                            </span>
                                        </div>
                                        <details class="work-more bot-more">
                                            <summary>"更多"</summary>
                                            <div class="work-kv">
                                                <span class="muted">"标识"</span>
                                                <span class="mono">{s(&b, "handle")}</span>
                                                <span class="muted">"分工"</span>
                                                <span>{role_label(&s(&b, "role"))}</span>
                                                <span class="muted">"优先级"</span>
                                                <span>{s(&b, "priority")}</span>
                                                <span class="muted">"在哪里运行"</span>
                                                <span>
                                                    {if s(&b, "brain") == "node" {
                                                        "工作机器上"
                                                    } else {
                                                        "本机"
                                                    }}
                                                </span>
                                            </div>
                                            <div class="bot-claim-form">
                                                <input
                                                    class="input mono"
                                                    aria-label="要占用的资源"
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
                                                    aria-label="在哪台机器上"
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
                                                                <option value=n
                                                                    .name
                                                                    .clone()>{fmt::machine(&n.name)}</option>
                                                            }
                                                        })
                                                        .collect_view()}
                                                </select>
                                                <select
                                                    class="input"
                                                    aria-label="占用方式"
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
                                                    "占用"
                                                </button>
                                            </div>
                                        </details>
                                    </section>
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
