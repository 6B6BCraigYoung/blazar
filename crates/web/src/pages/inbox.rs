use super::work_shared::*;
use crate::components::toast::toast;
use crate::{api, app_state::use_app, fmt, storage};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
const KINDS: &[(&str, &str)] = &[
    ("", "全部类型"),
    ("run_done", "跑完了"),
    ("run_failed", "没跑成"),
    ("approval", "等你裁决"),
    ("question", "在问你"),
    ("autopilot_paused", "自动化暂停"),
    ("rate_limit", "额度告警"),
];
#[component]
pub fn InboxPage() -> impl IntoView {
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
    view! {<div class="page work-page"><div class="page-head"><h1>"收件箱"</h1><span class="gchip">{move || format!("{} 未读",app.inbox_unread.get())}</span><span class="grow"></span><button class="btn small" disabled=move || busy.get() on:click=move |_| mark.run(("read".into(),vec![]))>"全部已读"</button><button class="btn small" disabled=move || busy.get() on:click=move |_| mark.run(("archive".into(),vec![]))>"归档已读"</button></div>
        <div class="work-filters">{select_field("显示",filter,&[("unread","未读"),("all","全部"),("archived","已归档")])}{select_field("类型",kind,KINDS)}<button class="btn small" on:click=move |_|refresh(rev)>"刷新"</button></div>
        {move || match data.get(){None=>view!{<div class="empty">"加载中…"</div>}.into_any(),Some(r)=>match &r {Err(e)=>view!{<div class="card pad bad" role="alert">{e.to_string()}</div>}.into_any(),Ok(r)=>{let items=arr(r,"items");if items.is_empty(){return view!{<div class="empty">"没有符合筛选条件的通知。运行结果、审批、提问和额度告警会显示在这里。"</div>}.into_any();}view!{<div class="work-inbox">{items.into_iter().map(|it| {let id=s(&it,"id");let read=it["read"].as_bool()==Some(true);let archived=it["archived"].as_bool()==Some(true);let k=s(&it,"kind");let label=KINDS.iter().find(|(key,_)|*key==k).map(|(_,v)|*v).unwrap_or("通知").to_string();let ws=s(&it,"workspace_id");let thread=s(&it,"thread_id");let href=if k=="autopilot_paused"{format!("/autopilots?id={}",api::enc(&s(&it,"ref_id")))}else if k=="rate_limit" {"/runtimes".into()}else if !thread.is_empty() && !ws.is_empty(){thread_link(&ws,&thread)}else if !ws.is_empty(){format!("/w/{}",api::enc(&ws))}else{"/inbox".into()};let (i1,i2,i3)=(id.clone(),id.clone(),id);view!{<article class="card work-inbox-row" class:work-unread=!read><a class="work-inbox-main" href=href on:click=move |_|{if !read{mark.run(("read".into(),vec![i1.clone()]));}}><strong>{s(&it,"title")}</strong><span class="muted">{s(&it,"body")}</span><small class="muted">{format!("{} · {}",label,fmt::ago(&s(&it,"created_at")))}</small></a><div class="row-actions"><button class="btn small ghost" disabled=move ||busy.get() on:click=move |_|mark.run((if read{"unread"}else{"read"}.into(),vec![i2.clone()]))>{if read{"标为未读"}else{"标为已读"}}</button><button class="btn small ghost" disabled=move ||busy.get() on:click=move |_|mark.run((if archived{"unarchive"}else{"archive"}.into(),vec![i3.clone()]))>{if archived{"移回"}else{"归档"}}</button></div></article>}}).collect_view()}</div>}.into_any()}}}}
    </div>}
}
