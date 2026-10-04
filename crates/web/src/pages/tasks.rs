use super::work_shared::*;
use crate::components::{
    dialog::{self, Choice},
    modal::Modal,
    status::{EmptyState, InlineError, LoadingState},
    toast::toast,
};
use crate::{api, fmt, md, storage};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_query_map;
use serde_json::{Value, json};
const STATUSES: &[(&str, &str)] = &[
    ("backlog", "待规划"),
    ("todo", "待办"),
    ("in_progress", "进行中"),
    ("in_review", "待审阅"),
    ("done", "完成"),
    ("cancelled", "已取消"),
];
const PRIORITIES: &[(&str, &str)] = &[
    ("urgent", "紧急"),
    ("high", "高"),
    ("medium", "中"),
    ("low", "低"),
    ("none", "无"),
];
fn priority(v: &Value) -> String {
    PRIORITIES
        .iter()
        .find(|(k, _)| *k == s(v, "priority"))
        .map(|(_, t)| t.to_string())
        .unwrap_or_default()
}
#[derive(Clone, Default, PartialEq, Eq)]
struct CommentDraft {
    body: String,
    note: bool,
}
#[derive(Clone, Copy)]
struct TaskUi {
    rev: RwSignal<u32>,
    selected: RwSignal<String>,
    editor: RwSignal<Option<Value>>,
    busy: RwSignal<bool>,
    editor_dirty: RwSignal<bool>,
    comment_drafts: RwSignal<std::collections::BTreeMap<String, CommentDraft>>,
    comment_sending: RwSignal<std::collections::BTreeSet<String>>,
}
impl TaskUi {
    fn mutate(self, id: String, method: &'static str, suffix: &'static str, body: Value) {
        if self.busy.try_get_untracked().unwrap_or(true) {
            return;
        }
        self.busy.set(true);
        spawn_local(async move {
            match api::send::<Value>(
                method,
                &format!("/api/tasks/{}{suffix}", api::enc(&id)),
                &body,
            )
            .await
            {
                Ok(r) => {
                    if suffix == "/start" {
                        if r["activity"]["started"] == false {
                            toast(s(&r["activity"], "reason"));
                        } else {
                            toast("已开始");
                        }
                    }
                    refresh(self.rev);
                }
                Err(e) => toast(e.to_string()),
            };
            self.busy.try_set(false);
        });
    }
}
#[component]
pub fn TasksPage() -> impl IntoView {
    let query = use_query_map();
    let saved = storage::load::<Value>("blazar.tasks.view").unwrap_or(json!({}));
    let mode = RwSignal::new(if s(&saved, "mode") == "list" {
        "list".into()
    } else {
        "board".into()
    });
    let q = RwSignal::new(String::new());
    let prio = RwSignal::new(s(&saved, "prio"));
    let ws = RwSignal::new(s(&saved, "ws"));
    let hidden = RwSignal::new(saved["showHidden"] == true);
    let ui = TaskUi {
        rev: work_revision("tasks"),
        selected: RwSignal::new(String::new()),
        editor: RwSignal::new(None),
        busy: RwSignal::new(false),
        editor_dirty: RwSignal::new(false),
        comment_drafts: RwSignal::new(std::collections::BTreeMap::new()),
        comment_sending: RwSignal::new(std::collections::BTreeSet::new()),
    };
    let dirty = RwSignal::new(false);
    Effect::new(move |_| {
        dirty.set(
            ui.editor_dirty.get()
                || ui
                    .comment_drafts
                    .with(|drafts| drafts.values().any(|draft| !draft.body.trim().is_empty())),
        );
    });
    super::agents::guard_unsaved(dirty);
    Effect::new(move |_| {
        ui.selected
            .set(query.read().get("task").unwrap_or_default());
    });
    Effect::new(move |_| {
        storage::save(
            "blazar.tasks.view",
            &json!({"mode":mode.get(),"showHidden":hidden.get(),"prio":prio.get(),"ws":ws.get()}),
        )
    });
    let data = LocalResource::new(move || {
        ui.rev.track();
        api::get::<Vec<Value>>("/api/tasks")
    });
    let filtered = move || {
        data.get()
            .and_then(|r| r.as_ref().ok().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| {
                let search = q.get().to_lowercase();
                let p = prio.get();
                let w = ws.get();
                s(t, "parent_id").is_empty()
                    && (search.is_empty()
                        || format!("{} {} {}", s(t, "key"), s(t, "title"), s(t, "description"))
                            .to_lowercase()
                            .contains(&search))
                    && (p.is_empty() || s(t, "priority") == p)
                    && (w.is_empty() || s(t, "workspace_id") == w)
            })
            .collect::<Vec<_>>()
    };
    let move_task = Callback::new(
        move |(id, status, before): (String, String, Option<String>)| {
            let all = data
                .get()
                .and_then(|r| r.as_ref().ok().cloned())
                .unwrap_or_default();
            if !all.iter().any(|t| s(t, "id") == id) {
                return;
            }
            let column = all
                .iter()
                .filter(|t| {
                    s(t, "parent_id").is_empty() && s(t, "status") == status && s(t, "id") != id
                })
                .collect::<Vec<_>>();
            let index = before
                .and_then(|b| column.iter().position(|t| s(t, "id") == b))
                .unwrap_or(column.len());
            let hi = column.get(index).and_then(|t| t["position"].as_f64());
            let lo = index
                .checked_sub(1)
                .and_then(|i| column.get(i))
                .and_then(|t| t["position"].as_f64());
            let position = match (lo, hi) {
                (Some(l), Some(h)) => (l + h) / 2.0,
                (None, Some(h)) => h - 1.0,
                (Some(l), None) => l + 1.0,
                _ => 0.0,
            };
            ui.mutate(id, "PUT", "", json!({"status":status,"position":position}));
        },
    );
    view! {<div class="page work-page tasks-page"><div class="page-head"><h1>"任务"</h1><span class="grow"></span><button class="btn primary" on:click=move |_|ui.editor.set(Some(json!({"status":"todo"})))>"新建任务"</button></div>
        <div class="work-filters"><input class="input" aria-label="搜索任务" placeholder="搜索任务" prop:value=move ||q.get() on:input=move |e|q.set(event_target_value(&e))/>{select_field("视图",mode,&[("board","看板"),("list","列表")])}{select_field("优先级",prio,&[("","全部"),("urgent","紧急"),("high","高"),("medium","中"),("low","低"),("none","无")])}<WorkspaceField value=ws/><label class="chk"><input type="checkbox" prop:checked=move ||hidden.get() on:change=move |e|hidden.set(event_target_checked(&e))/>"包含待规划与已取消"</label><button class="btn small" on:click=move |_|refresh(ui.rev)>"刷新"</button></div>
        <div class="work-layout"><div class="work-main">{move ||match data.get(){None=>view!{<LoadingState text="正在读取任务…"/>}.into_any(),Some(r)=>match &r{Err(e)=>view!{<InlineError message=format!("无法读取任务：{e}") retry=Callback::new(move |()|refresh(ui.rev))/>}.into_any(),Ok(_)=>{
            let list=filtered();let board=mode.get()=="board";let empty=list.is_empty();let narrowed=!q.get().is_empty()||!prio.get().is_empty()||!ws.get().is_empty();view!{{empty.then(||view!{<EmptyState title=if narrowed{"没有匹配的任务"}else{"还没有任务"} detail=if narrowed{"调整筛选条件，或换个关键词。"}else{"新建任务，写下目标，再交给智能体处理。"} class="tasks-empty"/>})}<div class:work-board=board class:work-list=!board>{STATUSES.iter().filter(|(k,_)|hidden.get()||!matches!(*k,"backlog"|"cancelled")).map(|(key,label)|{
                let key=key.to_string();let items=list.iter().filter(|t|s(t,"status")==key).cloned().collect::<Vec<_>>();let (drop_key,add_key)=(key.clone(),key.clone());let column_empty=items.is_empty();
                view!{<section class="work-column" on:dragover=|e:leptos::ev::DragEvent|e.prevent_default() on:drop=move |e:leptos::ev::DragEvent|{e.prevent_default();if let Some(d)=e.data_transfer() && let Ok(id)=d.get_data("text/plain"){move_task.run((id,drop_key.clone(),None));}}>
                    <header><strong>{label.to_string()}</strong><span class="muted">{items.len()}</span><span class="grow"></span><button class="btn small ghost" aria-label=format!("在{label}新建任务") on:click=move |_|ui.editor.set(Some(json!({"status":add_key})))>"＋"</button></header>
                    {column_empty.then(||view!{<EmptyState title="暂无任务" class="tasks-column-empty"/>})}
                    {items.into_iter().map(|t|{let id=s(&t,"id");let (drag_id,drop_id)=(id.clone(),id.clone());let drop_key=key.clone();let selected_id=id.clone();view!{<button class="card work-task" class:work-selected=move ||ui.selected.get()==selected_id draggable="true" on:dragstart=move |e:leptos::ev::DragEvent|{if let Some(d)=e.data_transfer(){let _=d.set_data("text/plain",&drag_id);d.set_effect_allowed("move");}} on:dragover=|e:leptos::ev::DragEvent|e.prevent_default() on:drop=move |e:leptos::ev::DragEvent|{e.prevent_default();e.stop_propagation();if let Some(d)=e.data_transfer() && let Ok(id)=d.get_data("text/plain"){move_task.run((id,drop_key.clone(),Some(drop_id.clone())));}} on:click=move |_|ui.selected.set(id.clone())><strong>{s(&t,"title")}</strong><span class="work-task-key"><span>{s(&t,"key")}</span><span>{priority(&t)}</span>{(t["running"]==true).then(||badge("running"))}</span><small class="muted">{format!("{} · {}",s(&t,"workspace_name"),who(&t))}</small><span class="row-actions">{arr(&t,"labels").into_iter().filter_map(|v|v.as_str().map(str::to_string)).map(|l|view!{<span class="gchip">{l}</span>}).collect_view()}{(t["children"].as_u64().unwrap_or(0)>0).then(||view!{<small class="muted">{format!("子任务 {}/{}",t["children_done"],t["children"])}</small>})}</span></button>}}).collect_view()}
                </section>}
            }).collect_view()}</div>}.into_any()}}}}</div>
        <Show when=move ||!ui.selected.get().is_empty()>{move ||{let id=ui.selected.get();view!{<TaskDetail id ui/>}}}</Show></div>
        {move ||ui.editor.get().map(|init|view!{<TaskEditor init ui/>})}
    </div>}
}
#[component]
fn TaskEditor(init: Value, ui: TaskUi) -> impl IntoView {
    let id = s(&init, "id");
    let editing = !id.is_empty();
    let parent = s(&init, "parent_id");
    let locked = !s(&init, "thread_id").is_empty();
    let title = RwSignal::new(s(&init, "title"));
    let description = RwSignal::new(s(&init, "description"));
    let status = RwSignal::new(if s(&init, "status").is_empty() {
        "todo".into()
    } else {
        s(&init, "status")
    });
    let priority = RwSignal::new(if s(&init, "priority").is_empty() {
        "none".into()
    } else {
        s(&init, "priority")
    });
    let ws = RwSignal::new(s(&init, "workspace_id"));
    let who = RwSignal::new(assignee(&init));
    let labels = RwSignal::new(
        arr(&init, "labels")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    );
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let initial_values = StoredValue::new((
        title.get_untracked(),
        description.get_untracked(),
        status.get_untracked(),
        priority.get_untracked(),
        ws.get_untracked(),
        who.get_untracked(),
        labels.get_untracked(),
    ));
    Effect::new(move |_| {
        ui.editor_dirty.set(
            (
                title.get(),
                description.get(),
                status.get(),
                priority.get(),
                ws.get(),
                who.get(),
                labels.get(),
            ) != initial_values.get_value(),
        );
    });
    on_cleanup(move || {
        ui.editor_dirty.try_set(false);
    });
    let confirming_close = RwSignal::new(false);
    let close = Callback::new(move |()| {
        if busy.get_untracked() || confirming_close.get_untracked() {
            return;
        }
        let current = (
            title.get_untracked(),
            description.get_untracked(),
            status.get_untracked(),
            priority.get_untracked(),
            ws.get_untracked(),
            who.get_untracked(),
            labels.get_untracked(),
        );
        if current == initial_values.get_value() {
            ui.editor.set(None);
            return;
        }
        confirming_close.set(true);
        spawn_local(async move {
            if dialog::ask(
                "放弃修改",
                "任务尚未保存。关闭后，本次修改将丢失。",
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
    let save = Callback::new(move |start: bool| {
        if busy.get_untracked() {
            return;
        }
        if title.get_untracked().trim().is_empty() {
            error.set("标题必填".into());
            return;
        }
        if start && (ws.get_untracked().is_empty() || who.get_untracked().is_empty()) {
            error.set("要开始任务，请先选工作区和指派对象".into());
            return;
        }
        let mut body = assignee_patch(&who.get_untracked());
        body["title"] = json!(title.get_untracked().trim());
        body["description"] = json!(description.get_untracked());
        body["status"] = json!(status.get_untracked());
        body["priority"] = json!(priority.get_untracked());
        body["workspace_id"] = nonempty(ws.get_untracked());
        body["parent_id"] = nonempty(parent.clone());
        body["labels"] = json!(
            labels
                .get_untracked()
                .split([',', '，'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
        let id = id.clone();
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            let path = if editing {
                format!("/api/tasks/{}", api::enc(&id))
            } else {
                "/api/tasks".into()
            };
            match api::send::<Value>(if editing { "PUT" } else { "POST" }, &path, &body).await {
                Ok(t) => {
                    let id = s(&t, "id");
                    ui.selected.try_set(id.clone());
                    ui.editor.try_set(None);
                    refresh(ui.rev);
                    toast(if editing { "已保存" } else { "已创建" });
                    if start {
                        ui.busy.try_set(true);
                        match api::send::<Value>(
                            "POST",
                            &format!("/api/tasks/{}/start", api::enc(&id)),
                            &json!({}),
                        )
                        .await
                        {
                            Ok(r) => toast(if r["activity"]["started"] == false {
                                s(&r["activity"], "reason")
                            } else {
                                "已开始".into()
                            }),
                            Err(e) => toast(e.to_string()),
                        }
                        ui.busy.try_set(false);
                        refresh(ui.rev);
                    }
                }
                Err(e) => {
                    error.try_set(e.to_string());
                }
            }
            busy.try_set(false);
        });
    });
    view! {<Modal label=if editing{"编辑任务"}else{"新建任务"} class="dlg work-dialog tasks-dialog" on_close=close>
        <h3>{if editing{"编辑任务"}else{"新建任务"}}</h3>
        <fieldset class="tasks-fields" disabled=move ||busy.get()>
            <label class="work-field"><span>"标题"</span><input class="input" data-modal-initial-focus="" prop:value=move ||title.get() on:input=move |e|title.set(event_target_value(&e))/></label>
            {text_field("目标与验收标准",description,5)}
            <div class="work-grid"><WorkspaceField value=ws locked/><AssigneeField value=who/></div>
            <details class="work-more"><summary>"更多设置"</summary><div class="work-grid">{select_field("状态",status,STATUSES)}{select_field("优先级",priority,PRIORITIES)}</div>{text_field("标签（逗号分隔）",labels,0)}</details>
        </fieldset>
        {move ||(!error.get().is_empty()).then(||view!{<InlineError message=error.get()/>})}
        <div class="dlg-foot"><button class="btn" disabled=move ||busy.get() on:click=move |_|close.run(())>"取消"</button><button class="btn primary" disabled=move ||busy.get() on:click=move |_|save.run(false)>{move ||if busy.get(){"保存中…"}else if editing{"保存"}else{"创建"}}</button>{(!editing).then(||view!{<button class="btn" disabled=move ||busy.get() on:click=move |_|save.run(true)>"创建并开始"</button>})}</div>
    </Modal>}
}

#[component]
fn TaskDetail(id: String, ui: TaskUi) -> impl IntoView {
    let task_id = StoredValue::new(id);
    let body = Signal::derive(move || {
        ui.comment_drafts.with(|drafts| {
            drafts
                .get(&task_id.get_value())
                .map(|draft| draft.body.clone())
                .unwrap_or_default()
        })
    });
    let note = Signal::derive(move || {
        ui.comment_drafts.with(|drafts| {
            drafts
                .get(&task_id.get_value())
                .is_some_and(|draft| draft.note)
        })
    });
    let sending = Signal::derive(move || {
        ui.comment_sending
            .with(|ids| ids.contains(&task_id.get_value()))
    });
    let export = RwSignal::new(None::<(String, String)>);
    let data = LocalResource::new(move || {
        ui.rev.track();
        let id = task_id.get_value();
        async move { api::get::<Value>(&format!("/api/tasks/{}", api::enc(&id))).await }
    });
    let send = Callback::new(move |(): ()| {
        let text = body.get_untracked();
        if sending.get_untracked() || text.trim().is_empty() {
            return;
        }
        let Some(t) = data.get().and_then(|r| r.as_ref().ok().cloned()) else {
            toast("任务仍在加载，请稍后再发");
            return;
        };
        let can_run = !s(&t, "workspace_id").is_empty() && !assignee(&t).is_empty();
        let draft = CommentDraft {
            body: body.get_untracked(),
            note: note.get_untracked(),
        };
        let only_note = draft.note || !can_run;
        let id = task_id.get_value();
        ui.comment_sending.update(|ids| {
            ids.insert(id.clone());
        });
        spawn_local(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/tasks/{}/comments", api::enc(&id)),
                &json!({"body":text,"note":only_note}),
            )
            .await
            {
                Ok(r) => {
                    ui.comment_drafts.try_update(|drafts| {
                        if let Some(current) = drafts.get_mut(&id)
                            && *current == draft
                        {
                            current.body.clear();
                        }
                    });
                    toast(if r["triggered"] == true {
                        "已发给智能体"
                    } else {
                        "已记下"
                    });
                    refresh(ui.rev);
                }
                Err(e) => toast(e.to_string()),
            }
            ui.comment_sending.try_update(|ids| {
                ids.remove(&id);
            });
        });
    });
    view! {<aside class="card work-detail" aria-label="任务详情"><header class="work-detail-head"><strong>"任务详情"</strong><span class="grow"></span><button class="btn small ghost" aria-label="关闭任务详情" disabled=move ||sending.get()||ui.busy.get() on:click=move |_|ui.selected.set(String::new())>"×"</button></header>
    {move ||match data.get(){None=>view!{<LoadingState text="正在读取任务…"/>}.into_any(),Some(r)=>match &r{Err(e)=>view!{<InlineError message=format!("无法读取任务：{e}") retry=Callback::new(move |()|refresh(ui.rev)) class="pad"/>}.into_any(),Ok(t)=>{
        let t=t.clone();let id=s(&t,"id");let can_run=!s(&t,"workspace_id").is_empty()&&!assignee(&t).is_empty();let running=t["running"]==true;let thread=s(&t,"thread_id");let ws=s(&t,"workspace_id");let edit=t.clone();let child=t.clone();let delete=t.clone();let status_id=id.clone();let copy_key=s(&t,"key");let start_id=id.clone();let detach_id=id.clone();let export_id=id.clone();let obsidian_id=id;
        let run_id=arr(&t,"runs").into_iter().find(|r|s(r,"status")=="running").map(|r|s(&r,"id"));
        view!{<div class="pad"><h2>{s(&t,"title")}</h2><div class="row-actions">{badge(&s(&t,"status"))}{running.then(||badge("running"))}<span class="grow"></span><button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|ui.editor.set(Some(edit.clone()))>"编辑"</button></div>
            {(!s(&t,"parent_id").is_empty()).then(||view!{<a href=format!("/tasks?task={}",api::enc(&s(&t,"parent_id")))>"查看父任务"</a>})}
            <div class="md-body work-description" inner_html=md::render(&s(&t,"description"),"",&ws)></div><p class="muted small">{format!("{} · {} · {}",s(&t,"workspace_name"),who(&t),priority(&t))}</p>
            <div class="row-actions"><button class="btn primary small" disabled=move ||!can_run||running||ui.busy.get() on:click=move |_|ui.mutate(start_id.clone(),"POST","/start",json!({}))>{if thread.is_empty(){"开始"}else{"继续"}}</button>
            {run_id.map(|id|view!{<button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|{if ui.busy.try_get_untracked().unwrap_or(true){return;}ui.busy.set(true);let id=id.clone();spawn_local(async move{match api::send::<Value>("POST",&format!("/api/sessions/{}/interrupt",api::enc(&id)),&json!({})).await{Ok(_)=>{toast("已请求停止");refresh(ui.rev);},Err(e)=>toast(e.to_string())}ui.busy.try_set(false);});}>"停止"</button>})}
            {(!thread.is_empty()).then(||view!{<a class="btn small" href=thread_link(&ws,&thread)>"打开对话"</a>})}
            {(s(&t,"status")=="in_review").then(||view!{<button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|ui.mutate(status_id.clone(),"PUT","",json!({"status":"done"}))>"通过审阅"</button>})}</div>
            {(s(&t,"parent_id").is_empty()).then(||view!{<div class="work-section-head"><h3>"子任务"</h3><span class="grow"></span><button class="btn small" on:click=move |_|ui.editor.set(Some(json!({"parent_id":s(&child,"id"),"workspace_id":child["workspace_id"],"agent_profile":child["agent_profile"],"runtime":child["runtime"],"status":"todo"})))>"添加子任务"</button></div>{arr(&t,"child_list").into_iter().map(|c|view!{<a class="work-child" href=format!("/tasks?task={}",api::enc(&s(&c,"id")))><span class="muted">{s(&c,"key")}</span><strong>{s(&c,"title")}</strong>{badge(&s(&c,"status"))}</a>}).collect_view()}})}
            <details class="work-more"><summary>"执行记录"</summary>{if arr(&t,"runs").is_empty(){view!{<p class="muted small">"还没有运行过。选择工作区与指派对象后点「开始」。"</p>}.into_any()}else{arr(&t,"runs").into_iter().enumerate().map(|(i,r)|view!{<a class="work-run" href=thread_link(&ws,&thread)>{badge(&s(&r,"status"))}<span>{format!("第 {} 次 · {}",i+1,if s(&r,"trigger")=="initial"{"首次"}else{"评论 / 重试"})}</span><small class="muted">{format!("{} · {} 条 · ${:.2} · {}",s(&r,"runtime"),r["events"].as_u64().unwrap_or(0),r["cost_usd"].as_f64().unwrap_or(0.0),fmt::ago(&s(&r,"created_at")))}</small></a>}).collect_view().into_any()}}
            </details>
            <h3>"评论"</h3>{arr(&t,"comment_list").is_empty().then(||view!{<EmptyState title="还没有评论" detail="补充要求，或写一条备注。" class="tasks-inline-empty"/>})}{arr(&t,"comment_list").into_iter().map(|c|view!{<article class="work-comment"><div class="row-actions"><strong>{match s(&c,"author").as_str(){"user"=>"你".to_owned(),"agent"=>who(&t),_=>"系统".into()}}</strong>{(c["note"]==true).then(||view!{<span class="gchip">"仅备注"</span>})}<small class="muted">{fmt::ago(&s(&c,"created_at"))}</small></div><div class="md-body" inner_html=md::render(&s(&c,"body"),"",&ws)></div></article>}).collect_view()}
            <details class="work-more"><summary>"导出与更多"</summary><div class="row-actions"><span class="mono muted">{s(&t,"key")}</span><button class="btn small ghost" on:click=move |_|{let key=copy_key.clone();spawn_local(copy(key));}>"复制编号"</button></div><div class="row-actions"><button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|{let id=export_id.clone();spawn_local(async move{if dialog::ask("存为飞书文档","把任务描述和每轮回复存为一篇飞书文档？会使用本机 lark-cli 的登录身份创建。",vec![Choice::plain("取消"),Choice::plain("创建文档")]).await!=Some(1){return;}
                if ui.busy.try_get_untracked().unwrap_or(true){return;}ui.busy.set(true);match api::send::<Value>("POST",&format!("/api/tasks/{}/lark-doc",api::enc(&id)),&json!({})).await{Ok(r)=>{if r["ok"]==true{let url=s(&r,"url");if !url.is_empty(){export.try_set(Some(("打开飞书文档".into(),url)));}toast("飞书文档已创建");}else{toast(format!("创建失败：{}",s(&r,"detail")));}},Err(e)=>toast(e.to_string())}ui.busy.try_set(false);});}>"存为飞书文档"</button>
            <button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|{let id=obsidian_id.clone();if ui.busy.try_get_untracked().unwrap_or(true){return;}ui.busy.set(true);spawn_local(async move{let path=format!("/api/tasks/{}/obsidian-note",api::enc(&id));let mut r=api::send::<Value>("POST",&path,&json!({"overwrite":false})).await;if r.as_ref().err().is_some_and(|e|e.to_string().contains("同名")){if dialog::ask("覆盖 Obsidian 笔记","库中已有同名笔记，使用当前任务内容覆盖？",vec![Choice::plain("取消"),Choice::danger("覆盖")]).await==Some(1){r=api::send::<Value>("POST",&path,&json!({"overwrite":true})).await;}else{ui.busy.try_set(false);return;}}match r{Ok(r)=>{toast(format!("已存到 {}",s(&r,"path")));export.try_set(Some(("在 Obsidian 中打开".into(),s(&r,"open"))));},Err(e)=>toast(e.to_string())}ui.busy.try_set(false);});}>"存到 Obsidian"</button>
            {(!s(&t,"parent_id").is_empty()).then(||view!{<button class="btn small" disabled=move ||ui.busy.get() on:click=move |_|ui.mutate(detach_id.clone(),"PUT","",json!({"parent_id":null}))>"变成独立任务"</button>})}
            <button class="btn small danger" disabled=move ||ui.busy.get() on:click=move |_|{let t=delete.clone();spawn_local(async move{if dialog::ask("删除任务",&format!("删除 {}「{}」？对话记录保留；子任务变为独立任务。未发送的评论将丢失。",s(&t,"key"),s(&t,"title")),vec![Choice::plain("取消"),Choice::danger("删除")]).await!=Some(1){return;}
                if ui.busy.try_get_untracked().unwrap_or(true){return;}ui.busy.set(true);match api::send::<Value>("DELETE",&format!("/api/tasks/{}",api::enc(&s(&t,"id"))),&json!({})).await{Ok(_)=>{ui.comment_drafts.try_update(|drafts|{drafts.remove(&s(&t,"id"));});ui.selected.try_update(|current|{if *current==s(&t,"id"){current.clear();}});refresh(ui.rev);toast("已删除");},Err(e)=>toast(e.to_string())}ui.busy.try_set(false);});}>"删除任务"</button></div></details>
        </div>}.into_any()
    }}}}
    <div class="pad work-compose">{move ||export.get().map(|(label,href)|view!{<p><a href=href target="_blank" rel="noopener noreferrer">{label}</a></p>})}<textarea class="input" rows="3" placeholder="补充要求或写评论…" disabled=move ||sending.get() aria-label="任务评论" prop:value=move ||body.get() on:input=move |e|ui.comment_drafts.update(|drafts|drafts.entry(task_id.get_value()).or_default().body=event_target_value(&e)) on:keydown=move |e:leptos::ev::KeyboardEvent|{if e.key()=="Enter"&&(e.meta_key()||e.ctrl_key()){e.prevent_default();send.run(());}}></textarea><div class="row-actions"><label class="chk"><input type="checkbox" disabled=move ||sending.get() prop:checked=move ||note.get() on:change=move |e|ui.comment_drafts.update(|drafts|drafts.entry(task_id.get_value()).or_default().note=event_target_checked(&e))/>"仅备注"</label><span class="grow"></span><button class="btn primary small" disabled=move ||sending.get()||body.get().trim().is_empty() on:click=move |_|send.run(())>{move ||if sending.get(){"发送中…"}else{"发送"}}</button></div><small class="muted">"仅备注或尚未指派时，不会触发智能体。⌘ / Ctrl + Enter 发送。"</small></div>
    </aside>}
}
