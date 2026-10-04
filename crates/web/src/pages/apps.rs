use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::{Value, json};

use super::settings::{NOTIFY_KINDS, flag, rows, save, text};
use crate::components::{
    dialog::{Choice, ask},
    toast::toast,
};
use crate::{api, app_state::use_app};

mod job_poll;
use job_poll::JobPoll;

const APPS: &[(&str, &str, &str)] = &[
    (
        "lark",
        "飞书 / Lark",
        "消息、文档、表格、日历、邮件、任务和会议。让智能体查和办，也能转发提醒。",
    ),
    (
        "github",
        "GitHub",
        "创建 Pull Request，查询 PR 状态和检查结果，连接技能市场。",
    ),
    (
        "obsidian",
        "Obsidian",
        "把本机笔记库开成工作区，让智能体读写 Markdown 笔记。",
    ),
];

fn state(id: &str, v: &Value) -> String {
    if !flag(v, "installed") {
        return "未安装".into();
    }
    match id {
        "github" => if flag(v, "authed") {
            "已连接"
        } else {
            "未登录"
        }
        .into(),
        "lark" => {
            if flag(&v["auth"]["user"], "available") || flag(&v["auth"]["bot"], "available") {
                format!(
                    "已连接 · {}",
                    match v["settings"]["agent_access"].as_str() {
                        Some("write") => "智能体读写",
                        Some("read") => "智能体只读",
                        _ => "智能体已关闭",
                    }
                )
            } else if flag(v, "configured") {
                "未登录".into()
            } else {
                "未配置".into()
            }
        }
        _ => format!(
            "{} 个库",
            rows(&v["vaults"])
                .iter()
                .filter(|v| flag(v, "exists"))
                .count()
        ),
    }
}

#[component]
pub fn AppsPage() -> impl IntoView {
    view! { <div class="page"><div class="page-head"><h1>"办公"</h1></div><p class="muted">"连接本机软件自己的命令行和登录态，让智能体替你查和办。凭据由软件自己管理。"</p><div class="ws-list">{APPS.iter().map(|&(id,name,desc)|view! { <AppCard id name desc/> }).collect_view()}</div></div> }
}

#[component]
fn AppCard(id: &'static str, name: &'static str, desc: &'static str) -> impl IntoView {
    let data =
        LocalResource::new(
            move || async move { api::get::<Value>(&format!("/api/office/{id}")).await },
        );
    view! { <a class="ws-card" href=format!("/apps/{id}")><div class="top"><b>{name}</b></div><p>{desc}</p><span class="muted">{move ||match data.get(){None=>"加载中…".into(),Some(Err(e))=>format!("状态读取失败：{e}"),Some(Ok(v))=>state(id,&v)}}</span><span>"管理连接 →"</span></a> }
}

#[component]
pub fn AppDetailPage() -> impl IntoView {
    let params = use_params_map();
    view! { <div class="page"><For each=move ||vec![params.with(|p|p.get("id").unwrap_or_default())] key=|id|id.clone() children=move |id|{
        if let Some(&(app,name,_))=APPS.iter().find(|&&(app,_,_)|app==id){view! { <AppDetail id=app name/> }.into_any()}else{view! { <h1>"应用不存在"</h1><a href="/apps">"返回办公"</a> }.into_any()}
    }/></div> }
}

#[component]
fn AppDetail(id: &'static str, name: &'static str) -> impl IntoView {
    let rev = RwSignal::new(0u32);
    let data = LocalResource::new(move || {
        rev.get();
        async move { api::get::<Value>(&format!("/api/office/{id}")).await }
    });
    view! { <div class="page-head"><a href="/apps">"办公"</a><span class="muted">"/"</span><h1>{name}</h1><span class="grow"></span><button class="btn" on:click=move |_|rev.update(|n|*n+=1)>"刷新"</button></div>
        {move ||match data.get(){None=>view! { <div class="empty">"正在读取本机连接状态…"</div> }.into_any(),Some(Err(e))=>view! { <p class="err-line" role="alert">{e.to_string()}</p> }.into_any(),Some(Ok(v))=>{
            if id=="obsidian"{view! { <Obsidian initial=v rev/> }.into_any()}else{view! { <ConnectApp id initial=v.clone() rev/>{if id=="lark"&&flag(&v,"installed"){view! { <LarkSettings initial=v/> }.into_any()}else if id=="github"{view! { <section class="card settings-card"><h3>"用在哪"</h3><p>"工作区的 Git 面板可以创建 PR、查看状态和检查结果。远端工作区使用远端机器上的 gh 登录态。"</p><div class="settings-actions"><a href="/settings?s=git">"PR 默认设置"</a><a href="/skills?tab=market">"发现技能"</a></div></section> }.into_any()}else{().into_any()}} }.into_any()}
        }}}
    }
}

const DOMAINS: &[(&str, &str)] = &[
    ("im", "消息"),
    ("docs", "文档"),
    ("drive", "云盘"),
    ("base", "多维表格"),
    ("sheets", "表格"),
    ("slides", "幻灯片"),
    ("calendar", "日历"),
    ("mail", "邮箱"),
    ("task", "任务"),
    ("vc", "会议"),
    ("minutes", "妙记"),
    ("wiki", "知识库"),
    ("contact", "通讯录"),
    ("approval", "审批"),
    ("okr", "OKR"),
    ("attendance", "考勤"),
];

#[component]
fn ConnectApp(id: &'static str, initial: Value, rev: RwSignal<u32>) -> impl IntoView {
    let tick = RwSignal::new(0u32);
    let pending = RwSignal::new(false);
    let fetching = RwSignal::new(false);
    let poll = RwSignal::new(JobPoll::default());
    let last_job = RwSignal::new(None::<Value>);
    let job = LocalResource::new(move || {
        tick.get();
        fetching.set(true);
        async move {
            let result = api::get::<Value>(&format!("/api/office/{id}/job")).await;
            let _ = fetching.try_set(false);
            result
        }
    });
    let interval = StoredValue::new_local(gloo_timers::callback::Interval::new(1100, move || {
        if !pending.get_untracked()
            && !fetching.get_untracked()
            && poll.get_untracked().should_poll()
        {
            tick.update(|n| *n += 1);
        }
    }));
    on_cleanup(move || interval.dispose());
    Effect::new(move |_| match job.get() {
        Some(Ok(value)) => {
            let mut finished = false;
            poll.update(|poll| finished = poll.observe(Ok(text(&value, "status") == "running")));
            last_job.set((!value.is_null()).then_some(value));
            if finished {
                rev.update(|n| *n += 1);
            }
        }
        Some(Err(_)) => poll.update(|poll| {
            poll.observe(Err(()));
        }),
        None => {}
    });
    let busy = move || pending.get() || fetching.get() || poll.get().busy();
    let start = Callback::new(move |body: Value| {
        if busy() {
            return;
        }
        pending.set(true);
        leptos::task::spawn_local(async move {
            let confirm = if body["step"] == "logout" {
                ask(
                    "退出登录",
                    "退出本机应用登录后，相关功能需要重新登录才能继续使用。",
                    vec![Choice::plain("取消"), Choice::danger("退出登录")],
                )
                .await
                    == Some(1)
            } else if flag(&body, "force") {
                ask(
                    "重新创建飞书应用",
                    "这会替换本机应用配置，需要重新登录；原应用仍保留在开放平台。",
                    vec![Choice::plain("取消"), Choice::danger("重新创建")],
                )
                .await
                    == Some(1)
            } else {
                true
            };
            if confirm {
                match api::send::<Value>("POST", &format!("/api/office/{id}/connect"), &body).await
                {
                    Ok(value) => {
                        poll.update(JobPoll::started);
                        last_job.set(Some(value));
                        job.set(None);
                        tick.update(|n| *n += 1);
                    }
                    Err(e) => {
                        toast(e.to_string());
                        poll.update(JobPoll::start_failed);
                        job.set(None);
                        tick.update(|n| *n += 1);
                    }
                }
            }
            pending.set(false);
        });
    });
    let brand = RwSignal::new(
        if initial["auth"]["brand"] == "lark" {
            "lark"
        } else {
            "feishu"
        }
        .to_owned(),
    );
    let scope = RwSignal::new("recommend".to_owned());
    let domains = RwSignal::new(Vec::<String>::new());
    let installed = flag(&initial, "installed");
    let authed = if id == "lark" {
        flag(&initial["auth"]["user"], "available") || flag(&initial["auth"]["bot"], "available")
    } else {
        flag(&initial, "authed")
    };
    let configured = flag(&initial, "configured") || authed;
    view! { <section class="card settings-card"><h3>"连接"</h3><p class="muted">"安装与登录在本机执行，需要授权时打开下方链接完成。令牌由应用自己的命令行保管。"</p>
        <div class="settings-row"><div class="settings-label"><b>{if id=="lark"{"1. 安装 lark-cli"}else{"1. 安装 gh"}}</b><span>{text(&initial,"version")}</span></div>{if installed{view! { <span class="state ok">"已安装"</span> }.into_any()}else if flag(&initial,"can_install"){view! { <button class="btn primary" disabled=busy on:click=move |_|start.run(json!({"step":"install"}))>"安装"</button> }.into_any()}else{view! { <a href=if id=="lark"{"https://nodejs.org"}else{"https://cli.github.com"} target="_blank" rel="noopener noreferrer">"查看安装说明 ↗"</a> }.into_any()}}</div>
        {(id=="lark").then(||view! { <div class="settings-row"><div class="settings-label"><b>"2. 创建飞书应用"</b><span>{if configured{"已配置"}else{"先安装，再创建应用"}}</span></div><div class="settings-actions"><select class="settings-input" aria-label="飞书品牌" prop:value=move ||brand.get() disabled=move ||busy()||!installed on:change=move |e|brand.set(event_target_value(&e))><option value="feishu" selected=move ||brand.get()=="feishu">"飞书"</option><option value="lark" selected=move ||brand.get()=="lark">"Lark"</option></select><button class="btn" disabled=move ||busy()||!installed on:click=move |_|start.run(json!({"step":"init","brand":brand.get_untracked(),"force":configured}))>{if configured{"重新创建…"}else{"创建应用"}}</button></div></div>})}
        <div class="settings-row"><div class="settings-label"><b>{if id=="lark"{"3. 登录授权"}else{"2. 登录授权"}}</b><span>{if authed{"已登录，可以再次授权增加权限"}else{"在浏览器完成设备授权"}}</span></div><div class="settings-actions">{(id=="lark").then(||view! { <select class="settings-input" aria-label="授权范围" prop:value=move ||scope.get() disabled=busy on:change=move |e|scope.set(event_target_value(&e))><option value="recommend" selected=move ||scope.get()=="recommend">"推荐权限"</option><option value="all" selected=move ||scope.get()=="all">"全部权限"</option><option value="domains" selected=move ||scope.get()=="domains">"自选业务域"</option></select> })}<button class="btn primary" disabled=move ||busy()||!installed||(id=="lark"&&!configured) on:click=move |_|{if id=="lark"&&scope.get_untracked()=="domains"&&domains.get_untracked().is_empty(){toast("至少选择一个业务域");return;}start.run(json!({"step":"login","scope":scope.get_untracked(),"domains":domains.get_untracked()}));}>{if authed{"加权限 / 重新授权"}else{"登录"}}</button>{authed.then(||view! { <button class="btn danger" disabled=busy on:click=move |_|start.run(json!({"step":"logout"}))>"退出登录"</button> })}</div></div>
        <Show when=move ||id=="lark"&&scope.get()=="domains"><div class="settings-checks">{DOMAINS.iter().map(move |&(domain,label)|view! { <label><input type="checkbox" disabled=busy prop:checked=move ||domains.get().iter().any(|d|d==domain) on:change=move |e|{let checked=event_target_checked(&e);domains.update(|d|{d.retain(|s|s!=domain);if checked{d.push(domain.into());}});}/>{label}</label> }).collect_view()}</div></Show>
        {move ||job.get().and_then(Result::err).map(|error| view! {
            <p class="err-line" role="alert">{format!("连接任务状态读取失败：{error}，正在重试")}</p>
            <button class="btn" disabled=move || fetching.get() || pending.get() on:click=move |_| tick.update(|n| *n += 1)>"重试"</button>
        })}
        {move ||last_job.get().map(|data| view! { <ConnectJob id data tick/> })}
        {move ||(last_job.get().is_none() && fetching.get()).then(|| view! { <p class="muted">"读取连接任务…"</p> })}
        <details><summary>"也可以在终端里做"</summary><p class="muted">"绑定已有飞书应用需要 App ID 和 Secret，请直接在终端配置。"</p><pre class="settings-pre">{format!("安装：{}\n配置：{}\n登录：{}",text(&initial,"install"),initial["init"].as_str().unwrap_or("—"),initial["login"].as_str().unwrap_or(if id=="lark"{"lark-cli auth login --recommend"}else{"gh auth login"}))}</pre></details>
    </section> }
}

#[component]
fn ConnectJob(id: &'static str, data: Value, tick: RwSignal<u32>) -> impl IntoView {
    let cancel = RwSignal::new(false);
    let running = text(&data, "status") == "running";
    let url = data["url"]
        .as_str()
        .filter(|s| s.starts_with("https://") || s.starts_with("http://"))
        .map(str::to_owned);
    view! { <div class="app-connect-job" aria-live="polite"><div class="settings-actions"><b>{format!("{} · {}",text(&data,"step"),text(&data,"status"))}</b><span>{text(&data,"message")}</span><span class="grow"></span>{running.then(||view! { <button class="btn" disabled=move ||cancel.get() on:click=move |_|{cancel.set(true);leptos::task::spawn_local(async move {match api::send::<Value>("POST",&format!("/api/office/{id}/job/cancel"),&json!({})).await{Ok(_)=>tick.update(|n|*n+=1),Err(e)=>toast(e.to_string())}cancel.set(false);});}>"取消"</button> })}</div>
        {data["code"].as_str().map(|code|view! { <p>"一次性验证码："<strong>{code.to_owned()}</strong></p> })}
        {url.map(|url|view! { <div class="settings-actions"><a class="btn primary" href=url.clone() target="_blank" rel="noopener noreferrer">"在浏览器里打开授权链接 ↗"</a><code>{url}</code></div> })}
        {data["qr"].as_str().filter(|s|s.starts_with("data:image/")).map(|qr|view! { <img class="app-qr" src=qr.to_owned() alt="授权二维码"/> })}
        {(running&&!flag(&data,"url_trusted")&&data["url"].is_string()).then(||view! { <p class="err-line">"此链接域名不在应用官方域名列表中，请核对后打开。"</p> })}
        {(!rows(&data["lines"]).is_empty()).then(||view! { <pre class="settings-pre">{rows(&data["lines"]).into_iter().filter_map(|s|s.as_str().map(str::to_owned)).collect::<Vec<_>>().join("\n")}</pre> })}
    </div> }
}

#[component]
fn LarkSettings(initial: Value) -> impl IntoView {
    let access = RwSignal::new(text(&initial["settings"], "agent_access"));
    let busy = RwSignal::new(false);
    let enabled = RwSignal::new(flag(&initial["settings"]["notify"], "enabled"));
    let target = RwSignal::new(text(&initial["settings"]["notify"], "target"));
    let identity = RwSignal::new(text(&initial["settings"]["notify"], "as"));
    let kinds = RwSignal::new(
        rows(&initial["settings"]["notify"]["kinds"])
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect::<Vec<_>>(),
    );
    let output = RwSignal::new(String::new());
    let patch = move || json!({"notify":{"enabled":enabled.get_untracked(),"target":target.get_untracked().trim(),"as":identity.get_untracked(),"kinds":kinds.get_untracked()}});
    let test = Callback::new(move |dry: bool| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        output.set("处理中…".into());
        leptos::task::spawn_local(async move {
            if save("PUT", "/api/office/lark/settings", patch()).await {
                match api::send::<Value>(
                    "POST",
                    "/api/office/lark/test-notify",
                    &json!({"dry_run":dry}),
                )
                .await
                {
                    Ok(r) => output.set(if flag(&r, "ok") {
                        if dry {
                            format!("预览（没有发送）：\n{}", text(&r, "detail"))
                        } else {
                            "已发出测试消息。".into()
                        }
                    } else {
                        format!("没发成：\n{}", text(&r, "detail"))
                    }),
                    Err(e) => output.set(e.to_string()),
                }
            } else {
                output.set("设置保存失败，测试未执行，请检查提示后重试。".into());
            }
            busy.set(false);
        });
    });
    view! {
        <section class="card settings-card"><h3>"让智能体用飞书"</h3><p class="muted">"飞书操作在本机执行。高风险写操作以及登录、配置命令始终被限制。"</p><div class="settings-row"><label for="lark-access">"权限"</label><select id="lark-access" class="settings-input" prop:value=move ||access.get() disabled=move ||busy.get() on:change=move |e|{let next=event_target_value(&e);let previous=access.get_untracked();access.set(next.clone());busy.set(true);leptos::task::spawn_local(async move {let allowed=next!="write"||ask("允许智能体读写飞书","智能体将能发送消息、创建文档和修改日程。需批准模式仍逐次确认；绕过权限模式下直接执行。",vec![Choice::plain("取消"),Choice::plain("允许读写")]).await==Some(1);if allowed&&save("PUT","/api/office/lark/settings",json!({"agent_access":next})).await{access.set(next);}else{access.set(previous);}busy.set(false);});}><option value="off" selected=move ||access.get()=="off">"关闭"</option><option value="read" selected=move ||access.get()=="read">"只读"</option><option value="write" selected=move ||access.get()=="write">"读写"</option></select></div>
            <div class="settings-row"><div class="settings-label"><b>"飞书技能包"</b><span>{format!("本机 {} 个，已导入 {} 个",initial["skills_local"],initial["skills_imported"])}</span></div><button class="btn" disabled=move ||busy.get() on:click=move |_|{busy.set(true);leptos::task::spawn_local(async move{let result=async{let found=api::get::<Value>("/api/skills/import").await?;let sources=rows(&found).into_iter().filter(|s|text(s,"name").starts_with("lark-")).map(|s|s["source"].clone()).collect::<Vec<_>>();api::send::<Value>("POST","/api/skills/import",&json!({"sources":sources,"conflict":"overwrite"})).await}.await;match result{Ok(r)=>{output.set(serde_json::to_string_pretty(&r).unwrap_or_default());toast("飞书技能导入完成，可在智能体能力页配置");},Err(e)=>toast(e.to_string())}busy.set(false);});}>"导入 / 更新 SKILLs"</button></div>
        </section>
        <section class="card settings-card"><h3>"把提醒转发到飞书"</h3><p class="muted">"提醒内容包含标题、正文摘要，可能带工作区名和智能体回复。"</p><form class="settings-form" on:submit=move |e|{e.prevent_default();if busy.get_untracked(){return;}busy.set(true);leptos::task::spawn_local(async move{save("PUT","/api/office/lark/settings",patch()).await;busy.set(false);});}><fieldset disabled=move ||busy.get()>
            <label><input type="checkbox" prop:checked=move ||enabled.get() on:change=move |e|enabled.set(event_target_checked(&e))/>" 开启转发"</label>
            <label class="settings-field">"目标群 chat_id 或用户 open_id"<input class="settings-input" placeholder="oc_… 或 ou_…" prop:value=move ||target.get() on:input=move |e|target.set(event_target_value(&e))/></label>
            <label class="settings-field">"发送身份"<select class="settings-input" prop:value=move ||identity.get() on:change=move |e|identity.set(event_target_value(&e))><option value="" selected=move ||identity.get()=="">"lark-cli 默认身份"</option><option value="bot" selected=move ||identity.get()=="bot">"机器人"</option><option value="user" selected=move ||identity.get()=="user">"我自己"</option></select></label>
            <div class="settings-checks">{NOTIFY_KINDS.iter().map(move |&(kind,label)|view! { <label><input type="checkbox" prop:checked=move ||kinds.get().iter().any(|s|s==kind) on:change=move |e|{let checked=event_target_checked(&e);kinds.update(|ks|{ks.retain(|s|s!=kind);if checked{ks.push(kind.into());}});}/>{label}</label> }).collect_view()}</div><div class="settings-actions"><button class="btn primary" type="submit">"保存"</button><button class="btn" type="button" on:click=move |_|test.run(true)>"只预览"</button><button class="btn" type="button" on:click=move |_|test.run(false)>"发一条测试消息"</button></div>
        </fieldset></form><Show when=move ||!output.get().is_empty()><pre class="settings-pre" aria-live="polite">{move ||output.get()}</pre></Show></section>
        <LarkCalls/>
    }
}

#[component]
fn LarkCalls() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/office/lark/calls"));
    view! { <section class="card settings-card"><h3>"智能体调用记录"</h3><p class="muted">"只记录命令，不记录消息或文档正文等参数。"</p>{move ||match data.get(){None=>view! { <p>"加载中…"</p> }.into_any(),Some(Err(e))=>view! { <p class="err-line">{e.to_string()}</p> }.into_any(),Some(Ok(v))=>{let list=rows(&v);if list.is_empty(){view! { <p class="muted">"还没有调用记录。"</p> }.into_any()}else{list.into_iter().map(|c|view! { <div class="settings-row"><code>{format!("lark-cli {}",text(&c,"command"))}</code><span>{format!("{} · {} · {}",text(&c,"verdict"),text(&c,"risk"),text(&c,"created_at"))}</span></div> }).collect_view().into_any()}}}}</section> }
}

#[component]
fn Obsidian(initial: Value, rev: RwSignal<u32>) -> impl IntoView {
    let navigate = StoredValue::new_local(use_navigate());
    let app = use_app();
    let busy = RwSignal::new(false);
    let output = RwSignal::new(None::<Value>);
    let vaults = rows(&initial["vaults"]);
    let live = vaults
        .iter()
        .filter(|v| flag(v, "exists"))
        .cloned()
        .collect::<Vec<_>>();
    let first = live.first().map(|v| text(v, "path")).unwrap_or_default();
    let vault = RwSignal::new(
        initial["prefs"]["vault"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or(first),
    );
    let folder = RwSignal::new(text(&initial["prefs"], "folder"));
    let has_live = !live.is_empty();
    let persist =
        move || json!({"vault":vault.get_untracked(),"folder":folder.get_untracked().trim()});
    view! { <section class="card settings-card"><h3>"Obsidian 笔记库"</h3><p class="muted">"笔记库是本机 Markdown 文件夹，不需要插件或登录。列表来自 Obsidian 自己的记录。"</p>
        {if !flag(&initial,"installed"){view! { <a href="https://obsidian.md" target="_blank" rel="noopener noreferrer">"安装 Obsidian ↗"</a> }.into_any()}else if vaults.is_empty(){view! { <p class="muted">"在 Obsidian 新建或打开一个库，然后刷新本页。"</p> }.into_any()}else{vaults.into_iter().map(move |v|{let path=text(&v,"path");let ws=text(&v,"workspace_id");view! { <div class="settings-row"><div class="settings-label"><b>{text(&v,"name")}</b><span>{format!("{} · {}",path,if flag(&v,"exists"){format!("{} 篇笔记",v["notes"])}else{"文件夹不在了".into()})}</span></div>{if !flag(&v,"exists"){().into_any()}else if !ws.is_empty(){view! { <a class="btn" href=format!("/w/{}",api::enc(&ws))>"打开工作区"</a> }.into_any()}else{view! { <button class="btn primary" disabled=move ||busy.get() on:click=move |_|{let path=path.clone();busy.set(true);leptos::task::spawn_local(async move{match api::send::<Value>("POST","/api/workspaces",&json!({"node":"local","path":path,"name":null,"project":null})).await{Ok(r)=>{app.load_state();navigate.with_value(|go|go(&format!("/w/{}",text(&r,"id")),Default::default()));rev.update(|n|*n+=1);},Err(e)=>toast(e.to_string())}busy.set(false);});}>"开成工作区"</button> }.into_any()}}</div> }}).collect_view().into_any()}}
    </section>
    {has_live.then(||view! { <section class="card settings-card"><h3>"存到 Obsidian"</h3><p class="muted">"任务详情里的「存到 Obsidian」使用这里的默认位置。文件夹填写库内相对路径。"</p><form class="settings-form" on:submit=move |e|{e.prevent_default();if busy.get_untracked(){return;}busy.set(true);leptos::task::spawn_local(async move{save("PUT","/api/office/obsidian/settings",persist()).await;busy.set(false);});}><fieldset disabled=move ||busy.get()><label class="settings-field">"默认笔记库"<select class="settings-input" prop:value=move ||vault.get() on:change=move |e|vault.set(event_target_value(&e))>{live.into_iter().map(|v|{let path=text(&v,"path");view! { <option value=text(&v,"path") selected=move ||vault.get()==path>{text(&v,"name")}</option> }}).collect_view()}</select></label><label class="settings-field">"文件夹"<input class="settings-input" placeholder="Blazar" prop:value=move ||folder.get() on:input=move |e|folder.set(event_target_value(&e))/></label><div class="settings-actions"><button class="btn primary" type="submit">"保存"</button><button class="btn" type="button" on:click=move |_|{busy.set(true);leptos::task::spawn_local(async move{if save("PUT","/api/office/obsidian/settings",persist()).await{match api::send::<Value>("POST","/api/office/obsidian/note",&json!({"title":"Blazar 测试笔记","content":"# Blazar 测试笔记\n\n这篇测试笔记可以删除。\n","overwrite":true})).await{Ok(v)=>output.set(Some(v)),Err(e)=>toast(e.to_string())}}busy.set(false);});}>"写一篇测试笔记"</button></div></fieldset></form>{move ||output.get().map(|v|view! { <p aria-live="polite">{format!("已写到 {} ",text(&v,"path"))}<a href=text(&v,"open")>"在 Obsidian 里打开"</a></p> })}</section>})}
    }
}
