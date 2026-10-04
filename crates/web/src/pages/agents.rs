use std::collections::HashSet;

use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, closure::Closure};

use crate::app_state::use_app;
use crate::components::dialog::{self, Choice};
use crate::components::modal::Modal;
use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::components::toast::toast;
use crate::{api, fmt, storage};

pub(super) fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
fn list(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
fn n(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or_default()
}
fn archived(v: &Value) -> bool {
    v["archived_at"].as_str().is_some()
}
fn availability(a: &Value, runtimes: &[api::Runtime]) -> &'static str {
    if archived(a) {
        return "archived";
    }
    match runtimes.iter().find(|r| r.id == text(a, "runtime")) {
        None => "unbound",
        Some(r) if !r.installed => "unbound",
        Some(r) if r.authed == Some(false) => "offline",
        _ => "online",
    }
}
fn status_label(status: &str) -> &str {
    match status {
        "archived" => "已归档",
        "unbound" => "待配置",
        "offline" => "需登录",
        "online" => "可用",
        "running" => "进行中",
        "done" => "成功",
        "failed" => "失败",
        "interrupted" => "已取消",
        _ => status,
    }
}
pub(super) async fn confirm(title: &str, body: &str) -> bool {
    dialog::ask(
        title,
        body,
        vec![
            Choice::plain("取消"),
            Choice::danger(title.trim_end_matches('？').trim_end_matches('?')),
        ],
    )
    .await
        == Some(1)
}

pub(super) fn guard_unsaved(dirty: RwSignal<bool>) {
    let listener = Closure::<dyn Fn(web_sys::Event)>::new(move |e: web_sys::Event| {
        if e.default_prevented() || !dirty.get_untracked() {
            return;
        }
        if !window()
            .confirm_with_message("有尚未保存的修改。离开会丢失这些修改，确定离开？")
            .unwrap_or(false)
        {
            e.prevent_default();
        }
    });
    let _ = window().add_event_listener_with_callback(
        "blazar:before-navigate",
        listener.as_ref().unchecked_ref(),
    );
    let listener = StoredValue::new_local(listener);
    let unload = window_event_listener(ev::beforeunload, move |e| {
        if dirty.get_untracked() {
            e.prevent_default();
        }
    });
    on_cleanup(move || {
        listener.with_value(|l| {
            let _ = window().remove_event_listener_with_callback(
                "blazar:before-navigate",
                l.as_ref().unchecked_ref(),
            );
        });
        listener.dispose();
        unload.remove();
    });
}

#[component]
pub fn AgentsPage() -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0u32);
    let pref: Value = storage::load("blazar.agents.view").unwrap_or(json!({}));
    let scope = RwSignal::new(
        if text(&pref, "scope") == "archived" {
            "archived"
        } else {
            "all"
        }
        .to_owned(),
    );
    let query = RwSignal::new(String::new());
    let rt = RwSignal::new(text(&pref, "runtime"));
    let avail = RwSignal::new(if text(&pref, "avail").is_empty() {
        "all".into()
    } else {
        text(&pref, "avail")
    });
    let sort = RwSignal::new(if text(&pref, "sort").is_empty() {
        "recent".into()
    } else {
        text(&pref, "sort")
    });
    let asc = RwSignal::new(text(&pref, "dir") == "asc");
    let selected = RwSignal::new(HashSet::<String>::new());
    let busy = RwSignal::new(false);
    let runtimes = LocalResource::new(|| api::get::<api::Runtimes>("/api/runtimes"));
    let profiles = LocalResource::new(move || {
        rev.track();
        let scope = scope.get();
        async move {
            api::get::<Vec<Value>>(if scope == "archived" {
                "/api/agent-profiles?scope=archived"
            } else {
                "/api/agent-profiles"
            })
            .await
        }
    });
    Effect::new(move |_| {
        storage::save(
            "blazar.agents.view",
            &json!({ "scope":scope.get(), "runtime":rt.get(), "avail":avail.get(), "sort":sort.get(), "dir":if asc.get() {"asc"} else {"desc"} }),
        )
    });
    Effect::new(move |_| {
        if scope.get() == "all"
            && let Some(Ok(p)) = profiles.get()
        {
            app.agents_n.set(p.len());
        }
    });
    let batch = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let action = if scope.get_untracked() == "archived" {
                "restore"
            } else {
                "archive"
            };
            let ids = selected.get_untracked();
            if action == "archive"
                && !confirm(
                    "归档选中的智能体？",
                    "进行中的运行会被取消，配置与历史保留，可以稍后恢复。",
                )
                .await
            {
                busy.set(false);
                return;
            }
            let mut failures = 0;
            for id in &ids {
                if api::send::<Value>(
                    "POST",
                    &format!("/api/agent-profiles/{}/{action}", api::enc(id)),
                    &json!({}),
                )
                .await
                .is_err()
                {
                    failures += 1;
                }
            }
            toast(format!(
                "已处理 {} 个，失败 {} 个",
                ids.len() - failures,
                failures
            ));
            selected.set(HashSet::new());
            rev.update(|v| *v += 1);
            busy.set(false);
        });
    };
    view! {
        <div class="page wide agent-page">
            <div class="page-head"><h1>"智能体"</h1><span class="sub">"保存指令与能力，重复使用"</span><span class="grow"/><a class="btn primary" href="/agents/new">"新建智能体"</a></div>
            <div class="agent-toolbar">
                <select disabled=move || busy.get() aria-label="范围" prop:value=move || scope.get() on:change=move |e| { scope.set(event_target_value(&e)); selected.set(HashSet::new()); }><option value="all" prop:selected=move || scope.get()=="all">"全部"</option><option value="archived" prop:selected=move || scope.get()=="archived">"已归档"</option></select>
                <input aria-label="搜索智能体" placeholder="搜索名称或描述…" prop:value=move || query.get() on:input=move |e| query.set(event_target_value(&e))/>
                <details class="agent-filter-details"><summary>{move || if !rt.get().is_empty() || avail.get() != "all" { "筛选与排序 · 已筛选" } else { "筛选与排序" }}</summary><div class="agent-toolbar"><select aria-label="可用性" prop:value=move || avail.get() on:change=move |e| avail.set(event_target_value(&e))><option value="all" prop:selected=move || avail.get()=="all">"全部状态"</option><option value="online" prop:selected=move || avail.get()=="online">"可用"</option><option value="offline" prop:selected=move || avail.get()=="offline">"需登录"</option><option value="unbound" prop:selected=move || avail.get()=="unbound">"待配置"</option></select>
                <select aria-label="运行时" prop:value=move || rt.get() on:change=move |e| rt.set(event_target_value(&e))><option value="" prop:selected=move || rt.get().is_empty()>"全部运行时"</option>{move || runtimes.get().and_then(Result::ok).map(|r| r.runtimes.into_iter().map(|r| {let id=r.id.clone();view! { <option value=r.id prop:selected=move || rt.get()==id>{r.label}</option> }}).collect_view())}</select>
                <select aria-label="排序" prop:value=move || sort.get() on:change=move |e| sort.set(event_target_value(&e))><option value="recent" prop:selected=move || sort.get()=="recent">"最近活跃"</option><option value="name" prop:selected=move || sort.get()=="name">"名称"</option><option value="runs" prop:selected=move || sort.get()=="runs">"运行次数"</option><option value="created" prop:selected=move || sort.get()=="created">"创建时间"</option></select>
                <button class="btn" on:click=move |_| asc.update(|v| *v = !*v)>{move || if asc.get() { "↑ 升序" } else { "↓ 降序" }}</button></div></details><button class="btn ghost" disabled=move || profiles.get().is_none() on:click=move |_| rev.update(|v| *v += 1)>"刷新"</button>
            </div>
            {move || runtimes.get().and_then(Result::err).map(|e| view! { <InlineError message=format!("无法检查运行时：{e}") retry=Callback::new(move |_| runtimes.refetch())/> })}
            {move || (!selected.get().is_empty()).then(|| view! { <div class="agent-toolbar"><b>{move || format!("已选 {} 个",selected.get().len())}</b><button class="btn" disabled=move || busy.get() on:click=move |_| selected.set(HashSet::new())>"清除选择"</button><button class="btn" disabled=move || busy.get() on:click=batch>{move || if busy.get() { "处理中…" } else if scope.get()=="archived" { "恢复" } else { "归档" }}</button></div> })}
            {move || match profiles.get() {
                None => view! { <LoadingState text="正在加载智能体…"/> }.into_any(),
                Some(Err(e)) => view! { <InlineError message=format!("无法加载智能体：{e}") retry=Callback::new(move |_| rev.update(|v| *v += 1))/> }.into_any(),
                Some(Ok(mut profiles)) => {
                    let has_profiles = !profiles.is_empty();
                    let q = query.get().to_lowercase(); let r = rt.get(); let av = avail.get();
                    let rts_ready = matches!(runtimes.get(), Some(Ok(_)));
                    let rts = runtimes.get().and_then(Result::ok).map(|v| v.runtimes).unwrap_or_default();
                    profiles.retain(|a| (q.is_empty() || format!("{} {}",text(a,"name"),text(a,"description")).to_lowercase().contains(&q)) && (r.is_empty() || text(a,"runtime")==r) && (av=="all" || availability(a,&rts)==av));
                    let order = sort.get();
                    profiles.sort_by(|a,b| { let cmp = match order.as_str() { "runs" => n(a,"runs_30d").cmp(&n(b,"runs_30d")), "name" => text(a,"name").cmp(&text(b,"name")), "created" => text(a,"created_at").cmp(&text(b,"created_at")), _ => text(a,"last_used_at").cmp(&text(b,"last_used_at")) }; if asc.get() { cmp } else { cmp.reverse() } });
                    if profiles.is_empty() { return view! { <EmptyState title=if has_profiles { "没有匹配的智能体" } else if scope.get() == "archived" { "暂无归档的智能体" } else { "创建第一个智能体" } detail=if has_profiles { "调整搜索或筛选条件后再试。" } else if scope.get() == "archived" { "归档后可在这里恢复。" } else { "为常用工作保存指令，之后直接开始对话。" }/> }.into_any(); }
                    view! { <div class="card agent-table"><table><thead><tr><th>"选择"</th><th>"智能体"</th><th>"状态"</th><th>"运行时 / 模型"</th><th>"近 30 天"</th><th>"最近活跃"</th></tr></thead><tbody>{profiles.into_iter().map(|a| {
                        let id = text(&a,"id"); let check_id = id.clone(); let toggle_id = id.clone(); let status=availability(&a,&rts);
                        view! { <tr><td><input type="checkbox" disabled=move || busy.get() aria-label=format!("选择 {}",text(&a,"name")) prop:checked=move || selected.with(|s| s.contains(&check_id)) on:change=move |e| selected.update(|s| { if event_target_checked(&e) { s.insert(toggle_id.clone()); } else { s.remove(&toggle_id); } })/></td><td><a class="agent-name" href=format!("/agent/{}",api::enc(&id))><span>{if text(&a,"avatar").is_empty(){"🤖".into()} else {text(&a,"avatar")}}</span><b>{text(&a,"name")}</b></a><div class="muted">{text(&a,"description")}</div></td><td><span class=if !rts_ready { "state muted" } else if status == "online" { "state ok" } else if status == "offline" { "state warn" } else { "state muted" }>{if rts_ready { status_label(status) } else { "待检查" }}</span>{(n(&a,"running") > 0).then(|| view! { <div class="muted">{format!("{} 次进行中",n(&a,"running"))}</div> })}</td><td>{text(&a,"runtime_label")}<div class="muted mono">{if text(&a,"model").is_empty(){"默认".into()}else{text(&a,"model")}}</div></td><td>{format!("{} 次运行",n(&a,"runs_30d"))}{(n(&a,"failed_30d") > 0).then(|| view! { <div class="muted">{format!("{} 次失败",n(&a,"failed_30d"))}</div> })}</td><td>{if text(&a,"last_used_at").is_empty(){"尚未运行".into()}else{fmt::ago(&text(&a,"last_used_at"))}}</td></tr> }
                    }).collect_view()}</tbody></table></div> }.into_any()
                }
            }}
        </div>
    }
}

#[component]
pub fn AgentCreatePage() -> impl IntoView {
    let query = use_query_map();
    let data = LocalResource::new(move || {
        let template = query.with(|q| q.get("template"));
        let runtime = query.with(|q| q.get("runtime")).unwrap_or_default();
        async move {
            if let Some(id) = template {
                let mut v =
                    api::get::<Value>(&format!("/api/agent-profiles/{}", api::enc(&id))).await?;
                v["name"] = json!(format!("{}（副本）", text(&v, "name")));
                v["env"] = json!({});
                v["id"] = json!("");
                v["archived_at"] = Value::Null;
                Ok::<Value, api::ApiError>(v)
            } else {
                Ok(
                    json!({"name":"","runtime":runtime,"env":{},"custom_args":[],"starters":[],"max_concurrent":1}),
                )
            }
        }
    });
    view! { <div class="page agent-page"><div class="page-head"><a class="crumb" href="/agents">"智能体"</a><span>"/"</span><h1>"创建智能体"</h1></div>{move || match data.get() {
        None=>view!{<LoadingState text="正在加载配置…"/>}.into_any(), Some(Err(e))=>view!{<InlineError message=format!("无法加载配置：{e}") retry=Callback::new(move |_| data.refetch())/>}.into_any(), Some(Ok(a))=>view!{<AgentEditor initial=a create=true on_saved=Callback::new(|_|{})/>}.into_any()
    }}</div> }
}

#[component]
pub fn AgentProfilePage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let rev = RwSignal::new(0u32);
    let dirty = RwSignal::new(false);
    let tab = RwSignal::new(query.with_untracked(|q| match q.get("view").as_deref() {
        Some("settings") => "settings".into(),
        Some("work") => "work".into(),
        Some("capabilities") => q.get("tab").unwrap_or("instructions".into()),
        _ => "overview".into(),
    }));
    let navigate = StoredValue::new(use_navigate());
    Effect::new(move |_| {
        let next = query.with(|q| match q.get("view").as_deref() {
            Some("settings") => "settings".to_owned(),
            Some("work") => "work".to_owned(),
            Some("capabilities") => q.get("tab").unwrap_or("instructions".into()),
            _ => "overview".to_owned(),
        });
        if !dirty.get_untracked() && tab.get_untracked() != next {
            tab.set(next);
        }
    });
    let data = LocalResource::new(move || {
        rev.track();
        let id = params.with(|p| p.get("id")).unwrap_or_default();
        async move { api::get::<Value>(&format!("/api/agent-profiles/{}", api::enc(&id))).await }
    });
    view! { <div class="page wide agent-page">{move || match data.get() {
        None=>view!{<LoadingState text="正在加载智能体…"/>}.into_any(),
        Some(Err(e))=>view!{<InlineError message=format!("无法加载智能体：{e}") retry=Callback::new(move |_| rev.update(|v| *v += 1))/><a href="/agents">"返回智能体列表"</a>}.into_any(),
        Some(Ok(a))=>{
            let a=StoredValue::new(a); let closed=archived(&a.get_value());
            view!{
                <div class="page-head"><a class="crumb" href="/agents">"智能体"</a><span>"/"</span><h1>{text(&a.get_value(),"name")}</h1><span class="grow"/><AgentActions agent=a.get_value() rev blocked=dirty/></div>
                {closed.then(||view!{<div class="card notice">"已归档，恢复后可以再次选择这个智能体。"</div>})}
                <div class="card pad agent-summary"><span class="agent-avatar">{if text(&a.get_value(),"avatar").is_empty(){"🤖".into()}else{text(&a.get_value(),"avatar")}}</span><div><p class="muted">{text(&a.get_value(),"description")}</p><span class="muted">{format!("{} · {} · {} 次进行中",text(&a.get_value(),"runtime_label"),if text(&a.get_value(),"model").is_empty(){"默认模型".into()}else{text(&a.get_value(),"model")},n(&a.get_value(),"running"))}</span></div></div>
                <div class="agent-tabs" role="group" aria-label="智能体内容">{[("overview","概览"),("work","工作记录"),("instructions","指令与开场建议"),("skills","技能"),("mcp","MCP"),("settings","设置")].into_iter().map(|(key,label)|view!{<button class="btn" aria-pressed=move||tab.get()==key class:active=move||tab.get()==key on:click=move |_| { if tab.get_untracked()==key{return;} spawn_local(async move { if !dirty.get_untracked() || confirm("放弃未保存的修改？", "当前智能体配置尚未保存。").await { dirty.set(false); tab.set(key.into()); let view = if matches!(key,"instructions"|"skills"|"mcp"){format!("capabilities&tab={key}")}else{key.to_owned()}; let id=params.with_untracked(|p|p.get("id")).unwrap_or_default(); if !crate::files_js::confirm_navigation(){return;} navigate.with_value(|go|go(&format!("/agent/{}?view={view}",api::enc(&id)),Default::default())); } }); }>{label}</button>}).collect_view()}</div>
                {move || match tab.get().as_str(){
                    "overview"=>view!{<AgentOverview agent=a.get_value()/>}.into_any(),
                    "work"=>view!{<AgentRuns runs=list(&a.get_value(),"runs")/>}.into_any(),
                    "skills"=>view!{<AgentCapabilities agent=a.get_value() kind="skills"/>}.into_any(),
                    "mcp"=>view!{<AgentCapabilities agent=a.get_value() kind="mcp"/>}.into_any(),
                    _=>view!{<AgentEditor initial=a.get_value() create=false dirty section=if tab.get()=="instructions"{"instructions"}else{"settings"} on_saved=Callback::new(move |_|rev.update(|r|*r+=1))/>}.into_any()
                }}
            }.into_any()
        }
    }}</div> }
}

#[component]
fn AgentActions(agent: Value, rev: RwSignal<u32>, blocked: RwSignal<bool>) -> impl IntoView {
    let a = StoredValue::new(agent);
    let busy = RwSignal::new(false);
    let navigate = use_navigate();
    let nav = StoredValue::new(navigate);
    let app = use_app();
    let pick = RwSignal::new(false);
    let runtimes = LocalResource::new(|| api::get::<api::Runtimes>("/api/runtimes"));
    let act = move |action: &'static str| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let agent = a.get_value();
            let id = text(&agent, "id");
            if matches!(action, "archive" | "delete" | "cancel-runs")
                && !confirm(
                    match action {
                        "archive" => "归档智能体？",
                        "delete" => "彻底删除智能体？",
                        _ => "取消全部运行？",
                    },
                    match action {
                        "archive" => "进行中的运行会取消，所有历史保留，可以稍后恢复。",
                        "delete" => "删除后无法恢复；已有对话记录保留。",
                        _ => "进行中的运行将被取消，最多需要 5 秒停止。",
                    },
                )
                .await
            {
                busy.set(false);
                return;
            }
            let path = if action == "delete" {
                format!("/api/agent-profiles/{}", api::enc(&id))
            } else {
                format!("/api/agent-profiles/{}/{action}", api::enc(&id))
            };
            match api::send::<Value>(
                if action == "delete" { "DELETE" } else { "POST" },
                &path,
                &json!({}),
            )
            .await
            {
                Ok(_) => {
                    toast("操作完成");
                    if action == "delete" {
                        if crate::files_js::confirm_navigation() {
                            nav.with_value(|n| n("/agents", Default::default()));
                        }
                    } else {
                        rev.update(|r| *r += 1);
                    }
                }
                Err(e) => toast(e.to_string()),
            }
            busy.set(false);
        });
    };
    view! {<div class="agent-actions">
        {(!archived(&a.get_value())).then(||view!{<button class="btn primary" disabled=move ||busy.get()||blocked.get() on:click=move |_|{match runtimes.get_untracked(){Some(Ok(r)) if availability(&a.get_value(),&r.runtimes)!="unbound"=>pick.set(true),Some(Err(e))=>toast(e.to_string()),Some(Ok(_))=>toast("请先在设置中绑定已安装的运行时"),None=>toast("正在读取运行时，请稍后再试")}}>"开始对话"</button><a class="btn ghost" href=format!("/agents/new?template={}",api::enc(&text(&a.get_value(),"id")))>"复制"</a>})}
        {(n(&a.get_value(),"running")>0).then(||view!{<button class="btn" disabled=move||busy.get()||blocked.get() on:click=move |_|act("cancel-runs")>"取消全部运行"</button>})}
        {if archived(&a.get_value()){view!{<button class="btn" disabled=move||busy.get()||blocked.get() on:click=move |_|act("restore")>"恢复"</button><button class="btn danger" disabled=move||busy.get()||blocked.get() on:click=move |_|act("delete")>"删除"</button>}.into_any()}else{view!{<button class="btn" disabled=move||busy.get()||blocked.get() on:click=move |_|act("archive")>"归档"</button>}.into_any()}}
        {move||pick.get().then(||view!{<Modal label="选择工作区" class="dlg agent-modal" on_close=Callback::new(move |_| pick.set(false))><h3>"在哪个工作区对话？"</h3>{if app.workspaces().is_empty(){view!{<EmptyState title="先创建工作区" detail="选择项目后，即可和智能体开始对话。"/><a class="btn primary" href="/workspaces">"打开工作区列表"</a>}.into_any()}else{app.workspaces().into_iter().map(|w|view!{<button class="agent-pick" on:click=move |_|{if !crate::files_js::confirm_navigation(){return;} pick.set(false);nav.with_value(|n|n(&format!("/w/{}?agent={}&new=1",api::enc(&w.id),api::enc(&text(&a.get_value(),"id"))),Default::default()));}><b>{w.name}</b><span class="muted" title=w.path>{w.node}</span></button>}).collect_view().into_any()}}<button class="btn" on:click=move |_|pick.set(false)>"取消"</button></Modal>})}
    </div>}
}

#[component]
fn AgentOverview(agent: Value) -> impl IntoView {
    let total = n(&agent, "succeeded_30d") + n(&agent, "failed_30d");
    view! {<div class="agent-overview"><div class="card pad"><h3>"近 30 天"</h3><div class="agent-stats"><div><b>{n(&agent,"runs_30d")}</b><span>"次运行"</span></div><div><b>{if total==0{"—".into()}else{format!("{}%",n(&agent,"succeeded_30d")*100/total)}}</b><span>"成功率"</span></div><div><b>{format!("{:.0}s",agent["avg_secs_30d"].as_f64().unwrap_or_default())}</b><span>"平均耗时"</span></div><div><b>{format!("${:.2}",agent["cost_30d"].as_f64().unwrap_or_default())}</b><span>"费用"</span></div></div><p class="muted">{format!("{} 次失败 · {} 次取消",n(&agent,"failed_30d"),n(&agent,"cancelled_30d"))}</p></div><details class="card pad agent-runtime-details"><summary>"执行详情"</summary><p>{format!("运行时：{}",text(&agent,"runtime_label"))}</p><p>{format!("并发：{} · 思考：{}",n(&agent,"max_concurrent"),text(&agent,"thinking_level"))}</p><p class="muted">{format!("创建于 {} · 更新于 {}",text(&agent,"created_at"),text(&agent,"updated_at"))}</p></details></div><AgentRuns runs=list(&agent,"runs")/>}
}
#[component]
fn AgentRuns(runs: Vec<Value>) -> impl IntoView {
    let runs = StoredValue::new(runs);
    let filter = RwSignal::new("all".to_owned());
    let nav = use_navigate();
    let nav = StoredValue::new(nav);
    view! {<div class="card pad"><div class="agent-toolbar"><h3>"运行记录"</h3><select aria-label="运行状态" prop:value=move||filter.get() on:change=move|e|filter.set(event_target_value(&e))>{[("all","全部"),("running","进行中"),("done","成功"),("failed","失败"),("interrupted","已取消")].into_iter().map(|(v,l)|view!{<option value=v prop:selected=move||filter.get()==v>{l}</option>}).collect_view()}</select></div>{move||{
        let rows:Vec<_>=runs.get_value().into_iter().filter(|r|filter.get()=="all"||text(r,"status")==filter.get()).collect();
        if rows.is_empty(){return view!{<EmptyState title="暂无运行记录" detail="开始对话后，可在这里查看运行结果。"/>}.into_any();}
        rows.into_iter().map(|r|{let row=StoredValue::new(r.clone());view!{<button class="agent-run" disabled=r["workspace"].is_null() on:click=move |_|{let r=row.get_value();let ws=text(&r,"workspace_id");let thread=text(&r,"thread");if !crate::files_js::confirm_navigation(){return;} nav.with_value(|n|n(&format!("/w/{}?thread={}",api::enc(&ws),api::enc(&thread)),Default::default()));}><span class="badge">{status_label(&text(&r,"status")).to_owned()}</span><span class="grow">{if text(&r,"title").is_empty(){"聊天会话".into()}else{text(&r,"title")}}<span class="muted">{format!(" · {}",if text(&r,"workspace").is_empty(){"工作区已删除".into()}else{text(&r,"workspace")})}</span></span><span class="muted">{format!("{} · {} 条",fmt::ago(&text(&r,"created_at")),n(&r,"events"))}</span></button>}}).collect_view().into_any()
    }}</div>}
}

#[component]
fn AgentCapabilities(agent: Value, kind: &'static str) -> impl IntoView {
    let id = StoredValue::new(text(&agent, "id"));
    let closed = archived(&agent);
    let rev = RwSignal::new(0u32);
    let busy = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.track();
        async move {
            let lib = api::get::<Vec<Value>>(if kind == "skills" {
                "/api/skills"
            } else {
                "/api/mcp-servers"
            })
            .await?;
            let caps = api::get::<Value>(&format!(
                "/api/agent-profiles/{}/capabilities",
                api::enc(&id.get_value())
            ))
            .await?;
            Ok::<_, api::ApiError>((lib, caps))
        }
    });
    view! {<div class="card pad"><div class="agent-toolbar"><h3>{if kind=="skills"{"技能"}else{"MCP 服务器"}}</h3><a href=if kind=="skills"{"/skills"}else{"/skills?tab=mcp"}>"管理能力库 →"</a></div><p class="muted">{if kind=="skills"{"勾选的技能在下一轮运行时生效，不会改动运行时自己的技能目录。"}else if matches!(text(&agent,"runtime").as_str(),"claude"|"codex"){"勾选的 MCP 服务器在下一轮启动时合并进配置。"}else{"当前运行时不支持加载 MCP 服务器；Claude Code 与 Codex 支持此能力。"}}</p>{move||match data.get(){None=>view!{<LoadingState text="正在加载能力…"/>}.into_any(),Some(Err(e))=>view!{<InlineError message=format!("无法加载能力：{e}") retry=Callback::new(move |_| rev.update(|v| *v += 1))/>}.into_any(),Some(Ok((lib,caps)))=>{
        if lib.is_empty(){return view!{<EmptyState title="还没有可选能力" detail="先在能力库添加技能或 MCP 服务器。"/>}.into_any();}let enabled=StoredValue::new(list(&caps,kind));
        lib.into_iter().map(|v|{let cid=text(&v,"id");let checked=enabled.with_value(|e|e.contains(&json!(cid)));view!{<label class="agent-cap"><input type="checkbox" prop:checked=checked disabled=move||busy.get()||closed on:change=move|e|{let checked=event_target_checked(&e);let mut ids=enabled.get_value();ids.retain(|v|v.as_str()!=Some(&cid));if checked{ids.push(json!(cid));}busy.set(true);spawn_local(async move{let mut body=json!({});body[kind]=json!(ids);match api::send::<Value>("PUT",&format!("/api/agent-profiles/{}/capabilities",api::enc(&id.get_value())),&body).await{Ok(_)=>toast("已保存，下一轮生效"),Err(e)=>toast(e.to_string())}busy.set(false);rev.update(|r|*r+=1);});}/><span><b>{text(&v,"name")}</b><small class="muted">{text(&v,"description")}</small></span></label>}}).collect_view().into_any()
    }}}</div>}
}

#[component]
fn AgentEditor(
    initial: Value,
    create: bool,
    on_saved: Callback<Value>,
    #[prop(optional)] dirty: Option<RwSignal<bool>>,
    #[prop(default = "all")] section: &'static str,
) -> impl IntoView {
    let dirty = dirty.unwrap_or_else(|| RwSignal::new(false));
    let original = RwSignal::new(initial.clone());
    let draft = RwSignal::new(initial);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let closed = archived(&draft.get_untracked());
    let navigate = use_navigate();
    let args = RwSignal::new(
        list(&draft.get_untracked(), "custom_args")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let env = RwSignal::new(
        draft.get_untracked()["env"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
    );
    let saved_args = RwSignal::new(args.get_untracked());
    let saved_env = RwSignal::new(env.get_untracked());
    Effect::new(move |_| {
        dirty.set(
            draft.get() != original.get()
                || args.get() != saved_args.get()
                || env.get() != saved_env.get(),
        )
    });
    guard_unsaved(dirty);
    on_cleanup(move || {
        let _ = dirty.try_set(false);
    });
    let starter_count = Memo::new(move |_| list(&draft.get(), "starters").len());
    let runtime = Memo::new(move |_| text(&draft.get(), "runtime"));
    let rts = LocalResource::new(|| api::get::<api::Runtimes>("/api/runtimes"));
    let accounts = LocalResource::new(|| api::get::<api::Accounts>("/api/accounts"));
    Effect::new(move |_| {
        if create && let Some(Ok(r)) = rts.get() {
            let current = text(&draft.get_untracked(), "runtime");
            if !r.runtimes.iter().any(|r| r.installed && r.id == current)
                && let Some(r) = r.runtimes.iter().find(|r| r.installed)
            {
                draft.update(|v| {
                    v["runtime"] = json!(r.id);
                    for key in ["model", "thinking_level", "permission_mode", "account"] {
                        v[key] = Value::Null;
                    }
                });
                if !current.is_empty() {
                    toast("原运行时未安装，已改用可用运行时并重置模型与权限。 ");
                }
            }
        }
    });
    let models = LocalResource::new(move || {
        let rt = runtime.get();
        async move {
            if rt.is_empty() {
                return Ok::<Value, api::ApiError>(json!({ "models": [] }));
            }
            api::get::<Value>(&format!("/api/runtimes/{}/models", api::enc(&rt))).await
        }
    });
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        let mut body = draft.get_untracked();
        if env.get_untracked() != saved_env.get_untracked() {
            match super::skills::parse_kv(&env.get_untracked()) {
                Ok(v) => body["env"] = v,
                Err(e) => {
                    error.set(e);
                    return;
                }
            }
        }
        body["custom_args"] = json!(
            args.get_untracked()
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
        if text(&body, "name").trim().is_empty() {
            error.set("名称必填".into());
            return;
        }
        if text(&body, "runtime").is_empty() {
            error.set("请先安装一个运行时".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        let navigate = navigate.clone();
        spawn_local(async move {
            let path = if create {
                "/api/agent-profiles".into()
            } else {
                format!("/api/agent-profiles/{}", api::enc(&text(&body, "id")))
            };
            match api::send::<Value>(if create { "POST" } else { "PUT" }, &path, &body).await {
                Ok(v) => {
                    toast("已保存智能体");
                    if create {
                        dirty.set(false);
                        if !crate::files_js::confirm_navigation() {
                            busy.set(false);
                            return;
                        }
                        navigate(
                            &format!("/agent/{}", api::enc(&text(&v, "id"))),
                            Default::default(),
                        );
                    } else {
                        draft.set(v.clone());
                        original.set(v.clone());
                        saved_args.set(args.get_untracked());
                        saved_env.set(env.get_untracked());
                        dirty.set(false);
                        on_saved.run(v);
                    }
                }
                Err(e) => error.set(e.to_string()),
            }
            busy.set(false);
        });
    };
    view! {<div class="agent-editor">
        {create.then(||view!{<p class="muted">"复制时会保留指令、开场建议和参数；环境变量需要重新填写。"</p>})}
        {move || rts.get().and_then(Result::err).map(|e| view! { <InlineError message=format!("无法加载运行时：{e}") retry=Callback::new(move |_| rts.refetch())/> })}
        {move || accounts.get().and_then(Result::err).map(|e| view! { <InlineError message=format!("无法加载账号：{e}") retry=Callback::new(move |_| accounts.refetch())/> })}
        <fieldset disabled=move||busy.get()||closed>
            <div class="card pad" hidden=section=="instructions"><h3>"身份"</h3><AgentField draft key="avatar" label="头像"/><AgentField draft key="name" label="名称（1–40 字）"/><AgentField draft key="description" label="描述（最多 255 字）" multiline=true/></div>
            <div class="card pad" hidden=section=="settings"><h3>"指令与开场建议"</h3><AgentField draft key="instructions" label="工作指令（最多 8000 字）" multiline=true/>
                {move||(0..starter_count.get()).map(|i|view!{<div class="agent-starter"><label>"标题"<input prop:value=move||draft.with(|d|text(&d["starters"][i],"label")) maxlength="80" on:input=move|e|draft.update(|d|d["starters"][i]["label"]=json!(event_target_value(&e)))/></label><label>"提示词"<textarea rows="3" maxlength="4000" prop:value=move||draft.with(|d|text(&d["starters"][i],"prompt")) on:input=move|e|draft.update(|d|d["starters"][i]["prompt"]=json!(event_target_value(&e)))/></label><button class="btn" on:click=move |_|draft.update(|d|{if let Some(s)=d["starters"].as_array_mut(){s.remove(i);}})>"移除"</button></div>}).collect_view()}
                <button class="btn" disabled=move||list(&draft.get(),"starters").len()>=3 on:click=move |_|draft.update(|d|{let mut starters=list(d,"starters");starters.push(json!({"label":"","prompt":""}));d["starters"]=json!(starters);})>"添加开场建议（最多 3 条）"</button>
            </div>
            <div class="card pad" hidden=section=="instructions"><h3>"执行配置"</h3><label class="field">"运行时"<select prop:value=move||text(&draft.get(),"runtime") on:change=move|e|draft.update(|d|{d["runtime"]=json!(event_target_value(&e));for k in ["model","thinking_level","permission_mode","account"]{d[k]=Value::Null;}})><option value="" prop:selected=move||runtime.get().is_empty()>"选择运行时"</option>{move||rts.get().and_then(Result::ok).map(|r|r.runtimes.into_iter().map(|r|{let id=r.id.clone();view!{<option value=r.id prop:selected=move||runtime.get()==id>{format!("{}{}",r.label,if r.installed{""}else{"（未安装）"})}</option>}}).collect_view())}</select></label>
                <label class="field">"账号"<select prop:value=move||text(&draft.get(),"account") on:change=move|e|draft.update(|d|{let v=event_target_value(&e);d["account"]=if v.is_empty(){Value::Null}else{json!(v)};})><option value="" prop:selected=move||text(&draft.get(),"account").is_empty()>"跟随运行时"</option><option value="auto" prop:selected=move||text(&draft.get(),"account")=="auto">"自动选择"</option>{move||accounts.get().and_then(Result::ok).map(|a|a.accounts.into_iter().filter(|a|a.provider==runtime.get()).map(|a|{let id=a.id.clone();view!{<option value=a.id prop:selected=move||text(&draft.get(),"account")==id>{format!("{}{}",a.label,if a.disabled{"（已停用）"}else{""})}</option>}}).collect_view())}</select></label>
                <label class="field">"模型（留空使用默认值）"<input list="agent-model-options" prop:value=move||text(&draft.get(),"model") on:input=move|e|draft.update(|d|d["model"]=json!(event_target_value(&e)))/><datalist id="agent-model-options">{move||models.get().and_then(Result::ok).map(|v|list(&v,"models").into_iter().map(|m|view!{<option value=text(&m,"id")>{text(&m,"label")}</option>}).collect_view())}</datalist></label>
                <label class="field">"思考等级"<select prop:value=move||text(&draft.get(),"thinking_level") on:change=move|e|draft.update(|d|d["thinking_level"]=json!(event_target_value(&e)))><option value="" prop:selected=move||text(&draft.get(),"thinking_level").is_empty()>"跟随 CLI 配置"</option>{["minimal","low","medium","high","xhigh","max","ultra"].into_iter().map(|s|view!{<option value=s prop:selected=move||text(&draft.get(),"thinking_level")==s>{s}</option>}).collect_view()}</select></label>
                <label class="field">"权限模式"<select prop:value=move||text(&draft.get(),"permission_mode") on:change=move|e|draft.update(|d|d["permission_mode"]=json!(event_target_value(&e)))><option value="" prop:selected=move||text(&draft.get(),"permission_mode").is_empty()>"按运行时设置"</option>{move||{let modes:Vec<&str>=match runtime.get().as_str(){"codex"=>vec!["read-only","workspace-write","danger-full-access"],"claude"|"grok"=>vec!["default","acceptEdits","auto","plan","bypassPermissions"],_=>vec![]};modes.into_iter().map(|m|view!{<option value=m prop:selected=move||text(&draft.get(),"permission_mode")==m>{m}</option>}).collect_view()}}</select></label>
                <label class="field">"最大并发（1–50）"<input type="number" min="1" max="50" prop:value=move||n(&draft.get(),"max_concurrent").max(1).to_string() on:change=move|e|draft.update(|d|d["max_concurrent"]=json!(event_target_value(&e).parse::<u64>().unwrap_or(1).clamp(1,50)))/></label>
            </div>
            <div class="card pad" hidden=section=="instructions"><h3>"环境变量"</h3><details><summary>"查看或编辑环境变量（显示明文）"</summary><p class="muted">"每行 KEY=value。"</p><textarea class="mono" rows="5" aria-label="环境变量" prop:value=move||env.get() on:input=move|e|env.set(event_target_value(&e))/></details></div>
            <details class="card pad agent-runtime-details" hidden=section=="instructions"><summary>"自定义启动参数"</summary><p class="muted">"每行是一个完整参数，按顺序传入 CLI。"</p><textarea class="mono" rows="4" aria-label="自定义参数" prop:value=move||args.get() on:input=move|e|args.set(event_target_value(&e))/><pre class="agent-command">{move||format!("{} {}",text(&draft.get(),"runtime"),args.get().lines().map(|s|if s.contains(' '){format!("{s:?}")}else{s.to_owned()}).collect::<Vec<_>>().join(" "))}</pre></details>
        </fieldset>
        {move || (!error.get().is_empty()).then(|| view! { <InlineError message=error.get()/> })}
        {(!closed).then(||view!{<div class="agent-save"><span class="muted">{move||if dirty.get(){"有未保存的修改"}else{""}}</span><button class="btn primary" disabled=move||busy.get() on:click=save>{move||if busy.get(){"保存中…"}else if create{"创建并打开"}else{"保存修改"}}</button></div>})}
    </div>}
}

#[component]
fn AgentField(
    draft: RwSignal<Value>,
    key: &'static str,
    label: &'static str,
    #[prop(default = false)] multiline: bool,
) -> impl IntoView {
    view! {<label class="field">{label}{if multiline{view!{<textarea rows=if key=="instructions"{"9"}else{"3"} prop:value=move||text(&draft.get(),key) on:input=move|e|draft.update(|v|v[key]=json!(event_target_value(&e)))/>}.into_any()}else{view!{<input prop:value=move||text(&draft.get(),key) on:input=move|e|draft.update(|v|v[key]=json!(event_target_value(&e)))/>}.into_any()}}</label>}
}
