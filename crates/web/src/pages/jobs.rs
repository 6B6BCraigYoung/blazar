use super::work_shared::*;
use crate::components::{
    dialog::{self, Choice},
    modal::Modal,
    status::{EmptyState, InlineError, LoadingState, StateOf},
    toast::toast,
};
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

fn span(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    match s {
        0..60 => format!("{s} 秒"),
        60..3600 => format!("{} 分 {} 秒", s / 60, s % 60),
        3600..86_400 => format!("{} 小时 {} 分", s / 3600, s % 3600 / 60),
        _ => format!("{} 天 {} 小时", s / 86_400, s % 86_400 / 3600),
    }
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
                ("node", "节点"),
                ("fact", "事实"),
                ("job", "任务"),
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
            <a href=format!("/bots/{}", api::enc(&s(j, "bot_id")))>{s(j, "bot_handle")}</a>
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
        now.set(js_sys::Date::now());
    }));
    on_cleanup(move || ticker.dispose());
    let log_of = RwSignal::new(None::<Value>);
    let killing = RwSignal::new(String::new());
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
    view! {
        <div class="page work-page jobs-page">
            <div class="page-head">
                <h1>"任务"</h1>
                {crate::components::section::tasks("/jobs")}
                <span class="grow"></span>
            </div>
            <div class="work-filters">
                {select_field("状态", filter, FILTERS)}
                <button class="btn small" on:click=move |_| refresh(rev)>
                    "刷新"
                </button>
            </div>
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
                    let narrowed = !filter.get().is_empty();
                    view! {
                        <EmptyState
                            title=if narrowed { "没有这个状态的后台任务" } else { "还没有后台任务" }
                            detail=if narrowed {
                                "换个状态看看。"
                            } else {
                                "bot 或对话用 job_spawn 派出的长时间命令、用 job_register 登记的进程会出现在这里，结束时会通知发起方。"
                            }
                            class="jobs-empty"
                        />
                    }
                    .into_any()
                }
                Some(Ok(list)) => view! {
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
                                    .map(|j| {
                                        let running = s(&j, "status") == "running";
                                        let error = s(&j, "last_error");
                                        let id = s(&j, "id");
                                        let (for_log, for_kill, row) = (j.clone(), j.clone(), j.clone());
                                        let started = s(&j, "created_at");
                                        view! {
                                            <tr>
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
                                                    <div class="jobs-waits muted" title=waits(&j)>
                                                        {waits(&j)}
                                                    </div>
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
                                                    <button
                                                        class="btn small danger"
                                                        disabled=move || !running || killing.get() == id
                                                        on:click=move |_| kill(for_kill.clone())
                                                    >
                                                        "结束"
                                                    </button>
                                                </td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                .into_any(),
            }}
            {move || log_of.get().map(|j| view! { <JobLog job=j on_close=Callback::new(move |()| log_of.set(None)) /> })}
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
