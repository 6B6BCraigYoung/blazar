use super::work_shared::*;
use crate::components::{
    dialog::{self, Choice},
    modal::Modal,
    status::{EmptyState, InlineError, LoadingState},
    toast::toast,
};
use crate::{api, fmt};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_query_map;
use serde_json::{Value, json};
#[derive(Clone, Copy)]
struct ApUi {
    rev: RwSignal<u32>,
    selected: RwSignal<String>,
    editor: RwSignal<Option<Value>>,
    busy: RwSignal<bool>,
    token: RwSignal<Option<(String, String)>>,
}
impl ApUi {
    fn mutate(self, id: String, method: &'static str, suffix: &'static str, body: Value) {
        if self.busy.try_get_untracked().unwrap_or(true) {
            return;
        }
        self.busy.set(true);
        spawn_local(async move {
            match api::send::<Value>(
                method,
                &format!("/api/autopilots/{}{suffix}", api::enc(&id)),
                &body,
            )
            .await
            {
                Ok(r) => {
                    if suffix == "/run" {
                        toast(format!(
                            "{} {}",
                            status(&s(&r, "status")).0,
                            s(&r, "reason")
                        ));
                    }
                    if suffix == "/webhook" {
                        if method == "DELETE" {
                            self.token.try_set(None);
                        } else {
                            let origin = window().location().origin().unwrap_or_default();
                            self.token.try_set(Some((
                                id.clone(),
                                format!("{}{}", origin, s(&r, "path")),
                            )));
                        }
                    }
                    if method == "DELETE" && suffix.is_empty() {
                        self.selected.try_update(|current| {
                            if current == &id {
                                current.clear();
                            }
                        });
                    }
                    refresh(self.rev);
                }
                Err(e) => toast(e.to_string()),
            }
            self.busy.try_set(false);
        });
    }
}
fn local_time(iso: &str) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso));
    if d.get_time().is_nan() {
        return iso.into();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes()
    )
}
fn until(iso: &str) -> String {
    let s = (js_sys::Date::parse(iso) - js_sys::Date::now()) / 1000.0;
    if s.is_nan() {
        "—".into()
    } else if s <= 0.0 {
        "马上".into()
    } else if s < 90.0 {
        format!("{s:.0} 秒后")
    } else if s < 5400.0 {
        format!("{:.0} 分钟后", s / 60.0)
    } else if s < 129600.0 {
        format!("{:.0} 小时后", s / 3600.0)
    } else {
        format!("{:.0} 天后", s / 86400.0)
    }
}
fn schedule_label(value: &Value) -> String {
    let schedule = Schedule::parse(&s(value, "cron"));
    let label = match schedule.kind.as_str() {
        "none" => {
            return if value["webhook"] == true {
                "手动或外部触发"
            } else {
                "手动触发"
            }
            .into();
        }
        "minutes" => format!("每 {} 分钟", schedule.n),
        "hours" => format!("每 {} 小时，第 {} 分钟", schedule.n, schedule.minute),
        "daily" => format!("每天 {}", schedule.time),
        "weekly" => {
            let weekdays = ["日", "一", "二", "三", "四", "五", "六"];
            let days = schedule
                .days
                .iter()
                .filter_map(|day| weekdays.get(usize::from(*day)))
                .copied()
                .collect::<Vec<_>>()
                .join("、");
            format!("每周{days} {}", schedule.time)
        }
        _ => "自定日程".into(),
    };
    let timezone = s(value, "timezone");
    if timezone.is_empty() {
        label
    } else {
        format!("{label} · {timezone}")
    }
}
#[component]
pub fn AutopilotsPage() -> impl IntoView {
    let ui = ApUi {
        rev: work_revision("autopilots"),
        selected: RwSignal::new(String::new()),
        editor: RwSignal::new(None),
        busy: RwSignal::new(false),
        token: RwSignal::new(None),
    };
    let query = use_query_map();
    Effect::new(move |_| ui.selected.set(query.read().get("id").unwrap_or_default()));
    let data = LocalResource::new(move || {
        ui.rev.track();
        api::get::<Vec<Value>>("/api/autopilots")
    });
    let tick = RwSignal::new(0u32);
    let timer = StoredValue::new_local(gloo_timers::callback::Interval::new(30_000, move || {
        tick.try_update(|n| *n = n.wrapping_add(1));
    }));
    on_cleanup(move || timer.dispose());
    view! {<div class="page work-page autopilots-page"><div class="page-head"><h1>"自动化"</h1><span class="grow"></span><button class="btn small" on:click=move |_|refresh(ui.rev)>"刷新"</button><button class="btn primary" on:click=move |_|ui.editor.set(Some(json!({})))>"新建自动化"</button></div><div class="work-layout"><div class="work-main">{move ||match data.get(){None=>view!{<LoadingState text="正在读取自动化…"/>}.into_any(),Some(r)=>match &r{Err(e)=>view!{<InlineError message=format!("无法读取自动化：{e}") retry=Callback::new(move |()|refresh(ui.rev))/>}.into_any(),Ok(items)=>{
        if items.is_empty(){return view!{<EmptyState title="还没有自动化" detail="新建自动化，让智能体按日程处理重复工作。也支持手动和 Webhook 触发。"/>}.into_any();}
        items.iter().cloned().map(|a|{let id=s(&a,"id");let selected=id.clone();let next=s(&a,"next_run_at");view!{<button class="card work-ap-row" class:work-selected=move ||ui.selected.get()==selected on:click=move |_|ui.selected.set(id.clone())><div class="row-actions"><strong>{s(&a,"name")}</strong>{badge(&s(&a,"status"))}</div><span class="muted">{format!("{} · {}",s(&a,"workspace_name"),who(&a))}</span><span class="autopilot-schedule">{schedule_label(&a)}</span><div class="row-actions">{(!s(&a,"last_status").is_empty()).then(||badge(&s(&a,"last_status")))}<small class="muted">{format!("上次：{}",if s(&a,"last_run_at").is_empty(){"尚未运行".into()}else{fmt::ago(&s(&a,"last_run_at"))})}</small><span class="grow"></span>{(!next.is_empty()&&s(&a,"status")=="active").then(||{let title=local_time(&next);view!{<small title=title>{move ||{tick.track();format!("下次：{}",until(&next))}}</small>}})}</div></button>}}).collect_view().into_any()
    }}}}</div><Show when=move ||!ui.selected.get().is_empty()>{move ||{let id=ui.selected.get();view!{<AutopilotDetail id ui/>}}}</Show></div>{move ||ui.editor.get().map(|init|view!{<AutopilotEditor init ui/>})}</div>}
}
#[component]
fn AutopilotDetail(id: String, ui: ApUi) -> impl IntoView {
    let key = StoredValue::new(id);
    let data = LocalResource::new(move || {
        ui.rev.track();
        let id = key.get_value();
        async move { api::get::<Value>(&format!("/api/autopilots/{}", api::enc(&id))).await }
    });
    view! {<aside class="card work-detail" aria-label="自动化详情"><header class="work-detail-head"><strong>"自动化详情"</strong><span class="grow"></span><button class="btn small ghost" aria-label="关闭自动化详情" on:click=move |_|{ui.selected.set(String::new());ui.token.set(None);}>"×"</button></header>{move ||match data.get(){None=>view!{<LoadingState text="正在读取自动化…"/>}.into_any(),Some(r)=>match &r{Err(e)=>view!{<InlineError message=format!("无法读取自动化：{e}") retry=Callback::new(move |()|refresh(ui.rev)) class="pad"/>}.into_any(),Ok(a)=>{
        let a=a.clone();let id=s(&a,"id");let (run_id,toggle_id,hook_id,off_id,token_id)=(id.clone(),id.clone(),id.clone(),id.clone(),id.clone());let (edit,duplicate,delete)=(a.clone(),a.clone(),a.clone());let paused=s(&a,"status")=="paused";let hook=a["webhook"]==true;let ws=s(&a,"workspace_id");let visible_token_id=token_id.clone();
        view!{<div class="pad"><div class="row-actions"><h2>{s(&a,"name")}</h2>{badge(&s(&a,"status"))}</div><div class="row-actions"><button class="btn small primary" disabled=move ||ui.busy.get() on:click=move |_|ui.mutate(run_id.clone(),"POST","/run",json!({}))>"立即运行"</button><button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|ui.mutate(toggle_id.clone(),"PUT","",json!({"status":if paused{"active"}else{"paused"}}))>{if paused{"恢复"}else{"暂停"}}</button><button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|ui.editor.set(Some(edit.clone()))>"编辑"</button></div>
        {(!s(&a,"paused_reason").is_empty()).then(||view!{<p class="warn-tx">{s(&a,"paused_reason")}</p>})}
        <dl class="work-kv"><dt>"工作区"</dt><dd><a href=format!("/w/{}",api::enc(&ws))>{if s(&a,"workspace_name").is_empty(){"工作区已删除".into()}else{s(&a,"workspace_name")}}</a></dd><dt>"指派给"</dt><dd>{who(&a)}</dd><dt>"日程"</dt><dd>{schedule_label(&a)}</dd></dl>
        <h3>"执行内容"</h3><pre class="work-instructions">{s(&a,"instructions")}</pre>
        <details class="work-more"><summary>"运行设置"</summary><dl class="work-kv"><dt>"表达式"</dt><dd class="mono">{if s(&a,"cron").is_empty(){"未设置".into()}else{s(&a,"cron")}}</dd><dt>"时区"</dt><dd>{s(&a,"timezone")}</dd><dt>"方式"</dt><dd>{if s(&a,"mode")=="run"{"直接运行"}else{"每次建一条任务"}}</dd><dt>"工作区正忙"</dt><dd>{if s(&a,"concurrency")=="wait"{"等待空闲（最多 30 分钟）"}else{"跳过这一次"}}</dd><dt>"权限 / 模型"</dt><dd>{format!("{} / {}",if s(&a,"permission_mode").is_empty(){"跟随设置".into()}else{s(&a,"permission_mode")},if s(&a,"model").is_empty(){"跟随设置".into()}else{s(&a,"model")})}</dd></dl></details>
        {(s(&a,"status")=="active"&&!arr(&a,"upcoming").is_empty()).then(||view!{<h3>"接下来（本地时间）"</h3>{arr(&a,"upcoming").iter().filter_map(Value::as_str).map(|t|view!{<div class="work-run"><span>{local_time(t)}</span><small class="muted">{until(t)}</small></div>}).collect_view()}})}
        <details class="work-more" open=move ||ui.token.with(|token|token.as_ref().is_some_and(|(id,_)|id==&visible_token_id))><summary>"外部触发（Webhook）"</summary>
        {move ||ui.token.get().filter(|(id,_)|id==&token_id).map(|(_,url)|{let (copy_url,curl_url)=(url.clone(),url.clone());view!{<div class="work-hook"><p class="warn-tx small">"地址只显示这一次，请立即复制；丢失后需要轮换。持有地址的人可触发运行，请勿公开。"</p><input class="input mono" readonly aria-label="Webhook 地址" prop:value=url/><div class="row-actions"><button class="btn small" on:click=move |_|{let url=copy_url.clone();spawn_local(copy(url));}>"复制地址"</button><button class="btn small" on:click=move |_|{let command=format!("curl -X POST '{}' -H 'content-type: application/json' -d '{{\"reason\":\"ci failed\"}}'",curl_url);spawn_local(copy(command));}>"复制 curl 示例"</button></div></div>}})}
        <p class="muted small">{if hook{format!("已开启，地址以 …{} 结尾。POST 请求体会作为数据附给智能体。",s(&a,"webhook_hint"))}else{"开启后，外部系统可通过带令牌的地址触发运行。".into()}}</p><div class="row-actions"><button class=if hook{"btn small danger"}else{"btn small"} disabled=move ||ui.busy.get() on:click=move |_|{let id=hook_id.clone();spawn_local(async move{if hook&&dialog::ask("轮换 Webhook 地址","旧地址将立即失效，外部系统需要更新为新地址。",vec![Choice::plain("取消"),Choice::danger("轮换")]).await!=Some(1){return;}ui.mutate(id,"POST","/webhook",json!({}));});}>{if hook{"轮换地址"}else{"开启 Webhook"}}</button>{hook.then(||view!{<button class="btn small danger" disabled=move ||ui.busy.get() on:click=move |_|{let id=off_id.clone();spawn_local(async move{if dialog::ask("关闭 Webhook","关闭后地址立即失效。",vec![Choice::plain("取消"),Choice::danger("关闭")]).await==Some(1){ui.mutate(id,"DELETE","/webhook",json!({}));}});}>"关闭 Webhook"</button>})}</div>
        </details>
        <h3>{format!("运行记录（{}）",a["runs"].as_u64().unwrap_or(0))}</h3>{if arr(&a,"run_list").is_empty(){view!{<EmptyState title="还没有运行记录" detail="可以立即运行一次，或等待下次触发。" class="autopilot-inline-empty"/>}.into_any()}else{arr(&a,"run_list").into_iter().map(|r|{let task=s(&r,"task_id");let thread=s(&r,"thread_id");view!{<article class="work-run"><div class="row-actions">{badge(&s(&r,"status"))}<span class="muted small">{match s(&r,"source").as_str(){"manual"=>"手动","schedule"=>"日程",_=>"Webhook"}}</span><span class="grow"></span><small class="muted" title=s(&r,"triggered_at")>{fmt::ago(&s(&r,"triggered_at"))}</small></div>{if !task.is_empty(){view!{<a href=format!("/tasks?task={}",api::enc(&task))>{format!("{} {}",s(&r,"task_key"),s(&r,"task_title"))}</a>}.into_any()}else if !thread.is_empty(){view!{<a href=thread_link(&ws,&thread)>"打开对话"</a>}.into_any()}else{().into_any()}}<small class="muted">{s(&r,"reason")}</small></article>}}).collect_view().into_any()}}
        <details class="work-more"><summary>"更多操作"</summary><div class="work-section-head"><button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|{let mut copy=duplicate.clone();copy["id"]=Value::Null;copy["name"]=json!(format!("{} 副本",s(&copy,"name")));ui.editor.set(Some(copy));}>"复制一份"</button><span class="grow"></span><button class="btn small danger" disabled=move ||ui.busy.get() on:click=move |_|{let a=delete.clone();spawn_local(async move{if dialog::ask("删除自动化",&format!("删除「{}」和它的运行记录？已创建的任务与对话会保留。",s(&a,"name")),vec![Choice::plain("取消"),Choice::danger("删除")]).await==Some(1){ui.mutate(s(&a,"id"),"DELETE","",json!({}));}});}>"删除自动化"</button></div></details>
        </div>}.into_any()
    }}}}</aside>}
}
#[derive(Clone)]
struct Schedule {
    kind: String,
    n: String,
    minute: String,
    time: String,
    days: Vec<u8>,
    cron: String,
}
impl Schedule {
    fn parse(cron: &str) -> Self {
        let mut f = Self {
            kind: "custom".into(),
            n: "1".into(),
            minute: "0".into(),
            time: "09:00".into(),
            days: vec![1],
            cron: cron.into(),
        };
        if cron.is_empty() {
            f.kind = "none".into();
            return f;
        }
        let fields = cron.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 5 {
            return f;
        }
        let (m, h, day, month, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
        if day != "*" || month != "*" {
            return f;
        }
        if h == "*" && dow == "*" && m.starts_with("*/") {
            f.kind = "minutes".into();
            f.n = m[2..].into();
            return f;
        }
        if h.starts_with("*/") && dow == "*" && m.parse::<u8>().is_ok() {
            f.kind = "hours".into();
            f.n = h[2..].into();
            f.minute = m.into();
            return f;
        }
        if let (Ok(h), Ok(m)) = (h.parse::<u8>(), m.parse::<u8>()) {
            f.time = format!("{h:02}:{m:02}");
            if dow == "*" {
                f.kind = "daily".into();
            } else if dow == "1-5" {
                f.kind = "weekly".into();
                f.days = vec![1, 2, 3, 4, 5];
            } else if let Some(days) = dow
                .split(',')
                .map(|d| d.parse::<u8>().ok().filter(|n| *n <= 7).map(|n| n % 7))
                .collect::<Option<Vec<_>>>()
            {
                f.kind = "weekly".into();
                f.days = days;
            }
        }
        f
    }
}
#[component]
fn AutopilotEditor(init: Value, ui: ApUi) -> impl IntoView {
    let id = s(&init, "id");
    let editing = !id.is_empty();
    let name = RwSignal::new(s(&init, "name"));
    let instructions = RwSignal::new(s(&init, "instructions"));
    let ws = RwSignal::new(s(&init, "workspace_id"));
    let who = RwSignal::new(if assignee(&init).is_empty() {
        "r:claude".into()
    } else {
        assignee(&init)
    });
    let mode = RwSignal::new(if s(&init, "mode").is_empty() {
        "task".into()
    } else {
        s(&init, "mode")
    });
    let permission = RwSignal::new(s(&init, "permission_mode"));
    let model = RwSignal::new(s(&init, "model"));
    let template = RwSignal::new(s(&init, "title_template"));
    let concurrency = RwSignal::new(if s(&init, "concurrency").is_empty() {
        "skip".into()
    } else {
        s(&init, "concurrency")
    });
    let browser_tz = js_sys::Reflect::get(
        &js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new())
            .resolved_options(),
        &wasm_bindgen::JsValue::from_str("timeZone"),
    )
    .ok()
    .and_then(|v| v.as_string())
    .unwrap_or_else(|| "UTC".into());
    let timezone = RwSignal::new(if s(&init, "timezone").is_empty() {
        browser_tz
    } else {
        s(&init, "timezone")
    });
    let f = Schedule::parse(if init.get("cron").is_none() {
        "0 9 * * *"
    } else {
        init["cron"].as_str().unwrap_or_default()
    });
    let kind = RwSignal::new(f.kind);
    let n = RwSignal::new(f.n);
    let minute = RwSignal::new(f.minute);
    let time = RwSignal::new(f.time);
    let days = RwSignal::new(f.days);
    let custom = RwSignal::new(f.cron);
    let cron = Memo::new(move |_| {
        let t = time.get();
        let (hour, min) = t.split_once(':').unwrap_or(("09", "00"));
        let hour = hour.parse::<u8>().unwrap_or(9);
        let min = min.parse::<u8>().unwrap_or(0);
        match kind.get().as_str() {
            "none" => String::new(),
            "minutes" => format!("*/{} * * * *", n.get()),
            "hours" => format!("{} */{} * * *", minute.get(), n.get()),
            "daily" => format!("{min} {hour} * * *"),
            "weekly" => {
                let mut d = days.get();
                d.sort();
                format!(
                    "{min} {hour} * * {}",
                    d.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
                )
            }
            _ => custom.get().trim().to_string(),
        }
    });
    let preview_revision = RwSignal::new(0u32);
    let preview = LocalResource::new(move || {
        preview_revision.track();
        let (c, tz) = (cron.get(), timezone.get());
        async move {
            if c.is_empty() {
                return Ok(json!({"ok":true,"upcoming":[]}));
            }
            gloo_timers::future::TimeoutFuture::new(250).await;
            api::send::<Value>(
                "POST",
                "/api/autopilots/preview",
                &json!({"cron":c,"timezone":tz}),
            )
            .await
        }
    });
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let snapshot = move || {
        vec![
            name.get_untracked(),
            instructions.get_untracked(),
            ws.get_untracked(),
            who.get_untracked(),
            mode.get_untracked(),
            permission.get_untracked(),
            model.get_untracked(),
            template.get_untracked(),
            concurrency.get_untracked(),
            timezone.get_untracked(),
            kind.get_untracked(),
            n.get_untracked(),
            minute.get_untracked(),
            time.get_untracked(),
            custom.get_untracked(),
            days.get_untracked()
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ]
    };
    let initial_values = StoredValue::new(snapshot());
    let dirty = RwSignal::new(false);
    Effect::new(move |_| {
        name.track();
        instructions.track();
        ws.track();
        who.track();
        mode.track();
        permission.track();
        model.track();
        template.track();
        concurrency.track();
        timezone.track();
        kind.track();
        n.track();
        minute.track();
        time.track();
        custom.track();
        days.track();
        dirty.set(snapshot() != initial_values.get_value());
    });
    super::agents::guard_unsaved(dirty);
    let confirming_close = RwSignal::new(false);
    let close = Callback::new(move |()| {
        if busy.get_untracked() || confirming_close.get_untracked() {
            return;
        }
        if snapshot() == initial_values.get_value() {
            ui.editor.set(None);
            return;
        }
        confirming_close.set(true);
        spawn_local(async move {
            if dialog::ask(
                "放弃修改",
                "自动化尚未保存。关闭后，本次修改将丢失。",
                vec![Choice::plain("继续编辑"), Choice::danger("放弃修改")],
            )
            .await
                == Some(1)
            {
                ui.editor.try_set(None);
            }
            confirming_close.try_set(false);
        });
    });
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        if name.get_untracked().trim().is_empty()
            || instructions.get_untracked().trim().is_empty()
            || ws.get_untracked().is_empty()
            || who.get_untracked().is_empty()
        {
            error.set("名字、指令、工作区和指派对象都要填".into());
            return;
        }
        if kind.get_untracked() == "weekly" && days.get_untracked().is_empty() {
            error.set("每周至少选择一天".into());
            return;
        }
        let who = who.get_untracked();
        let body = json!({"name":name.get_untracked().trim(),"instructions":instructions.get_untracked(),"workspace_id":ws.get_untracked(),"agent_profile":who.strip_prefix("p:").unwrap_or(""),"runtime":who.strip_prefix("r:").unwrap_or(""),"mode":mode.get_untracked(),"model":model.get_untracked(),"permission_mode":permission.get_untracked(),"title_template":template.get_untracked(),"cron":cron.get_untracked(),"timezone":timezone.get_untracked(),"concurrency":concurrency.get_untracked()});
        let path = if editing {
            format!("/api/autopilots/{}", api::enc(&id))
        } else {
            "/api/autopilots".into()
        };
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            match api::send::<Value>(if editing { "PUT" } else { "POST" }, &path, &body).await {
                Ok(r) => {
                    ui.selected.try_set(s(&r, "id"));
                    ui.editor.try_set(None);
                    refresh(ui.rev);
                    toast(if editing { "已保存" } else { "已创建" });
                }
                Err(e) => {
                    error.try_set(e.to_string());
                }
            }
            busy.try_set(false);
        });
    };
    view! {<Modal label=if editing{"编辑自动化"}else{"新建自动化"} class="dlg work-dialog autopilots-dialog" on_close=close><h3>{if editing{"编辑自动化"}else{"新建自动化"}}</h3><fieldset class="autopilot-fields" disabled=move ||busy.get()><label class="work-field"><span>"名称"</span><input class="input" data-modal-initial-focus="" prop:value=move ||name.get() on:input=move |e|name.set(event_target_value(&e))/></label>{text_field("执行内容",instructions,5)}<p class="muted small">"每次触发都会将这些指令发给智能体。"</p><div class="work-grid"><WorkspaceField value=ws/><AssigneeField value=who/></div>
        {select_field("日程",kind,&[("none","仅手动或外部触发"),("minutes","每 N 分钟"),("hours","每 N 小时"),("daily","每天"),("weekly","每周"),("custom","自定日程")])}
        <Show when=move ||matches!(kind.get().as_str(),"minutes"|"hours")><div class="work-grid">{text_field("间隔",n,0)}<Show when=move ||kind.get()=="hours">{text_field("触发分钟（0–59）",minute,0)}</Show></div></Show>
        <Show when=move ||matches!(kind.get().as_str(),"daily"|"weekly")><label class="work-field"><span>"时间"</span><input class="input" type="time" prop:value=move ||time.get() on:input=move |e|time.set(event_target_value(&e))/></label></Show>
        <Show when=move ||kind.get()=="weekly"><div class="row-actions">{["日","一","二","三","四","五","六"].into_iter().enumerate().map(|(i,d)|view!{<label class="chk"><input type="checkbox" prop:checked=move ||days.with(|d|d.contains(&(i as u8))) on:change=move |e|{let checked=event_target_checked(&e);days.update(|d|{d.retain(|x|*x!=i as u8);if checked{d.push(i as u8);}});}/>{format!("周{d}")}</label>}).collect_view()}</div></Show>
        <Show when=move ||kind.get()=="custom">{text_field("cron（分 时 日 月 周）",custom,0)}</Show><Show when=move ||kind.get()!="none">{text_field("时区（如 Asia/Shanghai 或 UTC）",timezone,0)}<details class="work-more"><summary>"查看表达式"</summary><code>{move ||cron.get()}</code></details><div class="work-preview">{move ||match preview.get(){None=>view!{<LoadingState text="正在计算日程…" class="muted small"/>}.into_any(),Some(r)=>match &r{Err(e)=>view!{<InlineError message=e.to_string() retry=Callback::new(move |()|preview_revision.update(|n|*n=n.wrapping_add(1)))/>}.into_any(),Ok(v)=>{if v["ok"]==false{view!{<InlineError message=s(v,"error")/>}.into_any()}else if arr(v,"upcoming").is_empty(){view!{<span class="warn-tx small">"这条日程没有后续触发时间"</span>}.into_any()}else{view!{<span class="muted small">"接下来（本地时间）："</span>{arr(v,"upcoming").iter().filter_map(Value::as_str).map(|t|view!{<span class="gchip">{local_time(t)}</span>}).collect_view()}}.into_any()}}}}}</div></Show>
        <details class="work-more"><summary>"运行设置"</summary><div class="work-grid">{select_field("方式",mode,&[("task","每次建一条任务"),("run","直接运行")])}{select_field("权限模式",permission,&[("","跟随智能体 / 运行时设置"),("acceptEdits","自动批准改文件"),("bypassPermissions","全部放行"),("plan","只规划"),("default","每一步手动审批")])}</div>{text_field("模型（留空跟随设置）",model,0)}<Show when=move ||mode.get()=="task">{text_field("任务标题模板（可用 {{name}} {{date}} {{time}}）",template,0)}</Show>
        {select_field("工作区正忙时",concurrency,&[("skip","跳过这一次"),("wait","等待空闲（最多 30 分钟）")])}</details></fieldset><p class="muted small">"手动审批模式下，运行会在需要批准时等待，并出现在收件箱中。"</p>{move ||(!error.get().is_empty()).then(||view!{<InlineError message=error.get()/>})}<div class="dlg-foot"><button class="btn" disabled=move ||busy.get() on:click=move |_|close.run(())>"取消"</button><button class="btn primary" disabled=move ||busy.get() on:click=save>{move ||if busy.get(){"保存中…"}else if editing{"保存"}else{"创建"}}</button></div>
    </Modal>}
}
