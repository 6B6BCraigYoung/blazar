use super::work_shared::*;
use crate::components::page_head::PageHead;
use crate::components::{
    dialog::{self, Choice},
    empty::{self, Action, Blank},
    group::ListGroup,
    icon,
    modal::Modal,
    rows::{RowAction, RowKey, RowMenu, Rows},
    status::{InlineError, LoadingState, StateOf},
    toast::toast,
};
use crate::vocab::duration as span;
use crate::{api, fmt};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsValue;

const FILTERS: &[(&str, &str)] = &[
    ("", "全部"),
    ("running", "运行中"),
    ("done", "已完成"),
    ("failed", "失败"),
    ("killed", "已结束"),
    ("lost", "已失联"),
];

fn ms(iso: &str) -> Option<f64> {
    let t = js_sys::Date::parse(iso);
    (!t.is_nan()).then_some(t)
}

fn elapsed(j: &Value, now: f64) -> String {
    let Some(start) = ms(&s(j, "created_at")) else {
        return "—".into();
    };
    let end = ms(&s(j, "ended_at")).unwrap_or(now);
    span((end - start) / 1000.0)
}

fn deadline(j: &Value, now: f64) -> String {
    match ms(&s(j, "deadline_at")) {
        None => "—".into(),
        Some(at) if s(j, "status") != "running" => clock(at),
        Some(at) if at >= now => format!("还剩 {}", span((at - now) / 1000.0)),
        Some(at) => format!("已超过 {}", span((now - at) / 1000.0)),
    }
}

fn clock(at: f64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(at));
    format!(
        "{}-{:02} {:02}:{:02}",
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes()
    )
}

fn exit_code(j: &Value) -> String {
    match (j["exit_code"].as_i64(), s(j, "status").as_str()) {
        (Some(c), _) => c.to_string(),
        (None, "done") if s(j, "kind") == "register" => "未知".into(),
        _ => "—".into(),
    }
}

fn waits(j: &Value) -> String {
    let items: Vec<String> = j["waits_for"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            [
                ("path", "文件"),
                ("node", "机器"),
                ("fact", "状态记录"),
                ("job", "后台任务"),
            ]
            .into_iter()
            .find_map(|(k, name)| {
                w[k].as_str().map(|v| {
                    let v = if k == "job" {
                        v.get(..8).unwrap_or(v)
                    } else {
                        v
                    };
                    format!("{name} {v}")
                })
            })
        })
        .collect();
    if items.is_empty() {
        String::new()
    } else {
        format!("等待：{}", items.join(" · "))
    }
}

fn owner(j: &Value) -> AnyView {
    match s(j, "owner_kind").as_str() {
        "bot" if !s(j, "bot_handle").is_empty() => view! {
            <a href=format!("/bots/{}", api::enc(&s(j, "bot_id"))) title=s(j, "bot_handle")>
                {Some(s(j, "bot_name")).filter(|n| !n.is_empty()).unwrap_or_else(|| s(j, "bot_handle"))}
            </a>
        }
        .into_any(),
        "bot" => view! { <span class="muted">"已删除的 bot"</span> }.into_any(),
        "session" => {
            let name = Some(s(j, "owner_workspace"))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "会话".into());
            view! { <span>{name}</span> }.into_any()
        }
        _ => view! { <span class="muted">"手动"</span> }.into_any(),
    }
}

#[component]
pub fn JobsPage() -> impl IntoView {
    let rev = work_revision("jobs");
    let filter = RwSignal::new(String::new());
    let now = RwSignal::new(js_sys::Date::now());
    let ticker = StoredValue::new_local(gloo_timers::callback::Interval::new(15_000, move || {
        let _ = now.try_set(js_sys::Date::now());
    }));
    on_cleanup(move || ticker.dispose());
    let log_of = RwSignal::new(None::<Value>);
    let killing = RwSignal::new(String::new());
    let rows = Rows::default();
    let shown = RwSignal::new(Vec::<Value>::new());
    let data = LocalResource::new(move || {
        rev.track();
        let f = filter.get();
        async move {
            let path = if f.is_empty() {
                "/api/jobs".to_owned()
            } else {
                format!("/api/jobs?status={}", api::enc(&f))
            };
            api::get::<Value>(&path).await.map(|v| arr(&v, "jobs"))
        }
    });
    let kill = move |j: Value| {
        if !killing.get_untracked().is_empty() {
            return;
        }
        spawn_local(async move {
            let id = s(&j, "id");
            let ok = dialog::ask(
                "结束后台任务",
                &format!(
                    "结束「{}」？进程先收到 TERM，几秒内没退出再强制结束。",
                    s(&j, "label")
                ),
                vec![Choice::plain("取消"), Choice::danger("结束")],
            )
            .await
                == Some(1);
            if !ok {
                return;
            }
            killing.set(id.clone());
            match api::send::<Value>(
                "POST",
                &format!("/api/jobs/{}/kill", api::enc(&id)),
                &json!({}),
            )
            .await
            {
                Ok(_) => toast("已结束"),
                Err(e) => toast(e.to_string()),
            }
            killing.try_set(String::new());
            refresh(rev);
        });
    };
    rows.listen(
        "tr.job-row",
        move || shown.try_with_untracked(Vec::len).unwrap_or(0),
        move |key, i| {
            let Some(j) = shown.try_with_untracked(|l| l.get(i).cloned()).flatten() else {
                return;
            };
            match key {
                RowKey::Open | RowKey::Inspect => log_of.set(Some(j)),
                RowKey::Delete => {
                    if s(&j, "status") == "running" {
                        kill(j);
                    }
                }
            }
        },
    );
    let menu_of = move |j: Value| {
        let (for_log, for_kill) = (j.clone(), j.clone());
        let command = s(&j, "command");
        vec![
            vec![RowAction::opens(move || log_of.set(Some(for_log.clone())))],
            vec![
                RowAction::new("复制命令", move || {
                    spawn_local(copy(command.clone()));
                })
                .when(!s(&j, "command").is_empty()),
            ],
            if s(&j, "status") == "running" {
                vec![RowAction::delete("结束", move || kill(for_kill.clone()))]
            } else {
                Vec::new()
            },
        ]
    };
    view! {
        <div class="page work-page jobs-page">
            <PageHead title="任务" nav=|| crate::components::section::tasks("/jobs") />
            <Show when=move || {
                !filter.with(String::is_empty)
                    || data.with(|d| matches!(d, Some(Ok(l)) if !l.is_empty()))
            }>
                <div class="work-filters">{filter_select("状态", filter, FILTERS)}</div>
            </Show>
            {move || match data.get() {
                None => view! { <LoadingState text="正在读取后台任务…" /> }.into_any(),
                Some(Err(e)) => view! {
                    <InlineError
                        message=format!("无法读取后台任务：{e}")
                        retry=Callback::new(move |()| refresh(rev))
                    />
                }
                .into_any(),
                Some(Ok(list)) if list.is_empty() => {
                    if filter.with(String::is_empty) {
                        view! {
                            <Blank
                                icon=icon::TERMINAL
                                title=empty::never("后台任务")
                                detail="智能体启动的长时间命令（训练、构建）会出现在这里，结束时通知你。"
                            />
                        }
                            .into_any()
                    } else {
                        view! {
                            <Blank
                                icon=icon::FILTER
                                title=empty::unmatched("后台任务")
                                detail="换个状态看看。"
                                action=Action::clear(move || filter.set(String::new()))
                            />
                        }
                            .into_any()
                    }
                }
                Some(Ok(list)) => {
                    shown.set(list.clone());
                    view! {
                    <ListGroup>
                        <div class="jobs-table">
                            <table class="tb">
                                <thead>
                                    <tr>
                                        <th>"状态"</th>
                                        <th>"标签"</th>
                                        <th>"机器"</th>
                                        <th>"发起方"</th>
                                        <th>"开始"</th>
                                        <th>"已运行"</th>
                                        <th>"截止"</th>
                                        <th>"退出码"</th>
                                        <th class="jobs-actions-head">"操作"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {list
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, j)| {
                                            let error = s(&j, "last_error");
                                            let (for_log, for_open, row) = (j.clone(), j.clone(), j.clone());
                                            let started = s(&j, "created_at");
                                            view! {
                                                <tr
                                                    class="job-row"
                                                    tabindex="-1"
                                                    data-index=i.to_string()
                                                    data-sel=move || rows.selected(i).to_string()
                                                    data-menu=move || rows.menu_on(i).to_string()
                                                    on:focusin=move |_| rows.sel.set(Some(i))
                                                    on:contextmenu=move |e| rows.open_menu(&e, i)
                                                    on:dblclick=move |_| log_of.set(Some(for_open.clone()))
                                                >
                                                    <td>
                                                        <StateOf
                                                            state=s(&j, "status")
                                                            title=(!error.is_empty()).then(|| error.clone())
                                                        />
                                                    </td>
                                                    <td class="jobs-label">
                                                        <div class="jobs-label-text" title=s(&j, "command")>
                                                            {s(&j, "label")}
                                                        </div>
                                                        {(!waits(&j).is_empty())
                                                            .then(|| {
                                                                view! {
                                                                    <div class="jobs-waits muted" title=waits(&j)>
                                                                        {waits(&j)}
                                                                    </div>
                                                                }
                                                            })}
                                                    </td>
                                                    <td>{fmt::machine(&s(&j, "node"))}</td>
                                                    <td>{owner(&j)}</td>
                                                    <td class="num" title=started.clone()>
                                                        {fmt::ago(&started)}
                                                    </td>
                                                    <td class="num">{move || elapsed(&row, now.get())}</td>
                                                    <td class="num">{move || deadline(&j, now.get())}</td>
                                                    <td class="num">{exit_code(&for_log)}</td>
                                                    <td class="jobs-actions">
                                                        <button
                                                            class="btn small"
                                                            on:click=move |_| log_of.set(Some(for_log.clone()))
                                                        >
                                                            "日志"
                                                        </button>
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
            {move || log_of.get().map(|j| view! { <JobLog job=j on_close=Callback::new(move |()| log_of.set(None)) /> })}
            {move || {
                rows.menu
                    .get()
                    .and_then(|at| shown.with(|l| l.get(at.index).cloned()).map(|j| (at, j)))
                    .map(|(at, j)| {
                        view! {
                            <RowMenu
                                at
                                label=format!("{} 的操作", s(&j, "label"))
                                groups=menu_of(j)
                                on_close=Callback::new(move |()| rows.close_menu())
                            />
                        }
                    })
            }}
        </div>
    }
}

#[component]
fn JobLog(job: Value, on_close: Callback<()>) -> impl IntoView {
    let id = s(&job, "id");
    let tick = RwSignal::new(0u32);
    let text = RwSignal::new(String::new());
    let note = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let pre = NodeRef::<leptos::html::Pre>::new();
    Effect::new(move |_| {
        text.track();
        if let Some(el) = pre.get() {
            el.set_scroll_top(el.scroll_height());
        }
    });
    Effect::new(move |_| {
        tick.track();
        let id = id.clone();
        busy.set(true);
        spawn_local(async move {
            match api::get::<Value>(&format!("/api/jobs/{}/log?tail=200", api::enc(&id))).await {
                Ok(v) => {
                    let log = s(&v, "log");
                    text.try_set(if log.trim().is_empty() {
                        "（日志是空的）".into()
                    } else {
                        log
                    });
                    note.try_set(s(&v, "path"));
                }
                Err(e) => {
                    note.try_set(format!("读不到日志：{e}"));
                }
            }
            busy.try_set(false);
        });
    });
    let title = format!("日志 · {}", s(&job, "label"));
    view! {
        <Modal label=title.clone() class="dlg wide jobs-log" on_close>
            <h3>{title}</h3>
            <pre class="jobs-log-text" tabindex="0" node_ref=pre>{move || text.get()}</pre>
            <div class="jobs-log-note muted">{move || note.get()}</div>
            <div class="dlg-foot">
                <button
                    type="button"
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| tick.update(|n| *n = n.wrapping_add(1))
                >
                    "刷新"
                </button>
                <button type="button" class="btn primary" on:click=move |_| on_close.run(())>
                    "关闭"
                </button>
            </div>
        </Modal>
    }
}
