use super::agents::{confirm, text};
use crate::app_state::use_app;
use crate::components::modal::Modal;
use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::components::toast::toast;
use crate::{api, md, storage};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_query_map};
use serde_json::{Value, json};
use std::collections::HashSet;

fn rows(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}
pub(super) fn parse_kv(source: &str) -> Result<Value, String> {
    let mut map = serde_json::Map::new();
    for (i, line) in source.lines().enumerate() {
        let line = line.trim_start();
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("第 {} 行不是 KEY=value", i + 1));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("第 {} 行缺少键名", i + 1));
        }
        if map.contains_key(key) {
            return Err(format!("键名 {key} 重复"));
        }
        map.insert(key.into(), json!(value));
    }
    Ok(Value::Object(map))
}
fn kv_text(v: &Value) -> String {
    v.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[component]
pub fn SkillsPage() -> impl IntoView {
    let query = use_query_map();
    let app = use_app();
    let tab = RwSignal::new(
        query
            .with_untracked(|q| q.get("tab"))
            .filter(|s| matches!(s.as_str(), "market" | "mcp"))
            .unwrap_or("skills".into()),
    );
    let navigate = StoredValue::new(use_navigate());
    Effect::new(move |_| {
        let next = query
            .with(|q| q.get("tab"))
            .filter(|s| matches!(s.as_str(), "market" | "mcp"))
            .unwrap_or("skills".into());
        if tab.get_untracked() != next {
            tab.set(next);
        }
    });
    let rev = RwSignal::new(0u32);
    let editing = RwSignal::new(None::<String>);
    let importing = RwSignal::new(false);
    let mcp = RwSignal::new(None::<Value>);
    let new_name = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let preview = RwSignal::new(None::<(Value, bool)>);
    let lib = LocalResource::new(move || {
        rev.track();
        let t = tab.get();
        async move {
            api::get::<Vec<Value>>(if t == "mcp" {
                "/api/mcp-servers"
            } else {
                "/api/skills"
            })
            .await
        }
    });
    Effect::new(move |_| {
        if tab.get() == "skills"
            && let Some(Ok(l)) = lib.get()
        {
            app.skills_n.set(l.len());
        }
    });
    let create = move |_| {
        if busy.get_untracked() {
            return;
        }
        let name = new_name.get_untracked().trim().to_owned();
        if name.is_empty() {
            error.set("请输入技能名称".into());
            return;
        }
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            match api::send::<Value>("POST", "/api/skills", &json!({"name":name})).await {
                Ok(v) => {
                    editing.set(Some(text(&v, "id")));
                    new_name.set(String::new());
                    rev.update(|r| *r += 1);
                }
                Err(e) => error.set(e.to_string()),
            }
            busy.set(false);
        });
    };
    view! {<div class="page wide agent-page skills-page"><div class="page-head"><h1>"技能"</h1><span class="sub">"为智能体添加可复用的能力"</span></div>
        <div class="agent-tabs" role="group" aria-label="技能内容">{[("skills","已安装"),("market","发现"),("mcp","MCP 服务器")].into_iter().map(|(k,l)|view!{<button class="btn" aria-pressed=move||tab.get()==k class:active=move||tab.get()==k on:click=move |_|{if tab.get_untracked()!=k{tab.set(k.into());navigate.with_value(|go|go(&format!("/skills?tab={k}"),Default::default()));}}>{l}</button>}).collect_view()}</div>
        {move||if tab.get()=="market"{view!{<SkillMarket preview refresh=rev/>}.into_any()}else{
            let is_mcp=tab.get()=="mcp";
            view!{<p class="muted">{if is_mcp{"添加服务器后，在智能体中启用。密钥保存后只显示键名。"}else{"添加技能后，在智能体中启用，下一轮运行生效。"}}</p>
                {if is_mcp{view!{<div><button class="btn primary" on:click=move |_|mcp.set(Some(json!({"transport":"stdio"})))>"添加 MCP 服务器"</button></div>}.into_any()}else{view!{<div class="agent-toolbar"><input aria-label="新技能名称" placeholder="技能名称" title="使用字母、数字、连字符或下划线" prop:value=move||new_name.get() on:input=move|e|new_name.set(event_target_value(&e))/><button class="btn primary" disabled=move||busy.get() on:click=create>{move || if busy.get() { "创建中…" } else { "新建技能" }}</button><button class="btn" on:click=move |_|importing.set(true)>"从本机导入"</button></div>}.into_any()}}
                {move || (!error.get().is_empty()).then(|| view! { <InlineError message=error.get()/> })}
                {move||match lib.get(){None=>view!{<LoadingState text="正在加载技能库…"/>}.into_any(),Some(Err(e))=>view!{<InlineError message=format!("无法加载技能库：{e}") retry=Callback::new(move |_| rev.update(|r| *r += 1))/>}.into_any(),Some(Ok(lib))=>{
                    if lib.is_empty(){return view!{<EmptyState title=if is_mcp { "添加第一个 MCP 服务器" } else { "添加第一个技能" } detail=if is_mcp { "保存连接配置，再为智能体启用。" } else { "新建、从本机导入，或在「发现」中安装。" }/>}.into_any();}
                    view!{<div class="card agent-table"><table><thead><tr><th>"名称"</th><th>{if is_mcp{"连接方式"}else{"说明"}}</th><th>{if is_mcp{"密钥"}else{"文件"}}</th><th>"用在"</th><th>"操作"</th></tr></thead><tbody>{lib.into_iter().map(|item|{let v=StoredValue::new(item.clone());view!{<tr><td><button class="agent-link" on:click=move |_|{if is_mcp{mcp.set(Some(v.get_value()));}else{editing.set(Some(text(&v.get_value(),"id")));}}>{text(&item,"name")}</button><div class="muted">{if is_mcp{text(&item,"transport")}else{text(&item["origin"],"provider")}}</div></td><td>{if is_mcp{if text(&item,"transport")=="http"{text(&item,"url")}else{format!("{} {}",text(&item,"command"),rows(&item,"args").iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))}}else{text(&item,"description")}}</td><td>{if is_mcp{(rows(&item,"env_keys").len()+rows(&item,"header_keys").len()).to_string()}else{item["files"].as_u64().unwrap_or_default().to_string()}}</td><td>{if text(&item,"used_by").is_empty(){"尚未启用".into()}else{text(&item,"used_by")}}</td><td><div class="agent-actions">{(!is_mcp&&!item["origin"].is_null()).then(||view!{<button class="btn" on:click=move |_|{let item=v.get_value();let o=&item["origin"];let install=if text(o,"provider")=="clawhub"{json!({"kind":"clawhub","owner":o["owner"],"slug":o["slug"]})}else{json!({"kind":"github","repo":o["repo"],"path":o["path"],"ref":o["ref"]})};preview.set(Some((install,true)));}>"更新"</button>})}<button class="btn danger" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return;}busy.set(true);spawn_local(async move{let v=v.get_value();if confirm(&format!("删除「{}」？",text(&v,"name")),&format!("删除后，使用它的智能体将无法继续加载此能力。{}",text(&v,"used_by"))).await{let path=format!("{}/{}",if is_mcp{"/api/mcp-servers"}else{"/api/skills"},api::enc(&text(&v,"id")));match api::send::<Value>("DELETE",&path,&json!({})).await{Ok(_)=>{toast("已删除");rev.update(|r|*r+=1);},Err(e)=>toast(e.to_string())}}busy.set(false);});}>"删除"</button></div></td></tr>}}).collect_view()}</tbody></table></div>}.into_any()
                }}}
            }.into_any()
        }}
        {move||editing.get().map(|id|view!{<SkillEditor id close=Callback::new(move |_|{editing.set(None);rev.update(|r|*r+=1);})/>})}
        {move||importing.get().then(||view!{<SkillImport close=Callback::new(move |_|{importing.set(false);rev.update(|r|*r+=1);})/>})}
        {move||mcp.get().map(|initial|view!{<McpEditor initial close=Callback::new(move |_|{mcp.set(None);rev.update(|r|*r+=1);})/>})}
        {move||preview.get().map(|(install,update)|view!{<MarketPreview install update close=Callback::new(move |_|{preview.set(None);rev.update(|r|*r+=1);})/>})}
    </div>}
}

#[component]
fn SkillEditor(id: String, close: Callback<()>) -> impl IntoView {
    let id = StoredValue::new(id);
    let data = RwSignal::new(None::<Value>);
    let path = RwSignal::new(String::new());
    let content = RwSignal::new(String::new());
    let saved = RwSignal::new(String::new());
    let add = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let dirty = RwSignal::new(false);
    Effect::new(move |_| dirty.set(content.get() != saved.get()));
    super::agents::guard_unsaved(dirty);
    let load = Callback::new(move |_| {
        error.set(String::new());
        spawn_local(async move {
            match api::get::<Value>(&format!("/api/skills/{}", api::enc(&id.get_value()))).await {
                Ok(v) => {
                    if let Some(first) = rows(&v, "files").first() {
                        path.set(text(first, "path"));
                        content.set(text(first, "content"));
                        saved.set(text(first, "content"));
                    }
                    data.set(Some(v));
                }
                Err(e) => error.set(e.to_string()),
            }
        });
    });
    load.run(());
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let p = path.get_untracked();
        let c = content.get_untracked();
        spawn_local(async move {
            match api::send::<Value>(
                "PUT",
                &format!("/api/skills/{}/files", api::enc(&id.get_value())),
                &json!({"path":p,"content":c}),
            )
            .await
            {
                Ok(_) => {
                    saved.set(c.clone());
                    data.update(|d| {
                        if let Some(d) = d {
                            let mut files = rows(d, "files");
                            if let Some(f) = files.iter_mut().find(|f| text(f, "path") == p) {
                                f["content"] = json!(c);
                            }
                            d["files"] = json!(files);
                        }
                    });
                    toast("已保存文件");
                }
                Err(e) => error.set(e.to_string()),
            }
            busy.set(false);
        });
    };
    let closing = RwSignal::new(false);
    let leave = move || {
        if busy.get_untracked() || closing.get_untracked() {
            return;
        }
        closing.set(true);
        spawn_local(async move {
            if content.get_untracked() == saved.get_untracked()
                || confirm("放弃未保存的修改？", "当前文件尚未保存。").await
            {
                close.run(());
            }
            let _ = closing.try_set(false);
        });
    };
    view! {<Modal label="技能文件编辑器" class="dlg agent-modal skill-modal" on_close=Callback::new(move |_| leave())><div class="agent-toolbar"><h3>{move||data.get().map(|d|text(&d,"name")).unwrap_or("加载技能…".into())}</h3><span class="grow"/><button class="btn" disabled=move||busy.get() on:click=move |_| leave()>"关闭"</button></div>
        {move || if error.get().is_empty() { ().into_any() } else if data.get().is_none() { view! { <InlineError message=error.get() retry=load/> }.into_any() } else { view! { <InlineError message=error.get()/> }.into_any() }}
        {move || (data.get().is_none() && error.get().is_empty()).then(|| view! { <LoadingState text="正在加载技能文件…"/> })}
        {move||data.get().map(|d|view!{<div class="skill-editor"><nav aria-label="技能文件">{rows(&d,"files").into_iter().map(|f|{let p=text(&f,"path");let c=text(&f,"content");let p2=p.clone();let p3=p.clone();view!{<button class="agent-pick" aria-current=move || (path.get() == p2).then_some("page") class:active=move||path.get()==p3 disabled=move||busy.get() on:click=move |_|{let p=p.clone();let c=c.clone();spawn_local(async move{if content.get_untracked()!=saved.get_untracked()&&!confirm("放弃未保存的修改？","切换文件会丢弃当前未保存的内容。").await{return;}path.set(p);content.set(c.clone());saved.set(c);});}>{text(&f,"path")}</button>}}).collect_view()}<input aria-label="新文件路径" placeholder="references/example.md" prop:value=move||add.get() on:input=move|e|add.set(event_target_value(&e))/><button class="btn" disabled=move||busy.get()||add.get().trim().is_empty() on:click=move |_|{busy.set(true);let p=add.get_untracked().trim().to_owned();if data.get_untracked().is_some_and(|d|rows(&d,"files").iter().any(|f|text(f,"path")==p)){error.set("这个文件已存在，请在左侧选择它。".into());busy.set(false);return;}spawn_local(async move{if content.get_untracked()!=saved.get_untracked()&&!confirm("放弃未保存的修改？","添加文件前请确认当前修改。").await{busy.set(false);return;}match api::send::<Value>("PUT",&format!("/api/skills/{}/files",api::enc(&id.get_value())),&json!({"path":p,"content":""})).await{Ok(_)=>{match api::get::<Value>(&format!("/api/skills/{}",api::enc(&id.get_value()))).await{Ok(v)=>{data.set(Some(v));path.set(p);content.set(String::new());saved.set(String::new());add.set(String::new());},Err(e)=>error.set(e.to_string())}},Err(e)=>error.set(e.to_string())}busy.set(false);});}>"添加文件"</button></nav><div class="skill-content"><label class="field">{move||path.get()}<textarea class="mono" spellcheck="false" prop:value=move||content.get() on:input=move|e|content.set(event_target_value(&e)) disabled=move||busy.get()/></label></div></div>})}
        <div class="agent-save"><button class="btn danger" disabled=move||busy.get()||path.get().is_empty()||path.get()=="SKILL.md" on:click=move |_|{busy.set(true);spawn_local(async move{if confirm("删除当前文件？",&path.get_untracked()).await{match api::send::<Value>("DELETE",&format!("/api/skills/{}/files?path={}",api::enc(&id.get_value()),api::enc(&path.get_untracked())),&json!({})).await{Ok(_)=>{match api::get::<Value>(&format!("/api/skills/{}",api::enc(&id.get_value()))).await{Ok(v)=>{let f=rows(&v,"files").first().cloned().unwrap_or_default();path.set(text(&f,"path"));content.set(text(&f,"content"));saved.set(text(&f,"content"));data.set(Some(v));},Err(e)=>error.set(e.to_string())}},Err(e)=>error.set(e.to_string())}}busy.set(false);});}>"删除文件"</button><span class="grow"/><span class="muted">{move||if content.get()!=saved.get(){"未保存"}else{""}}</span><button class="btn primary" disabled=move||busy.get()||path.get().is_empty() on:click=save>{move||if busy.get(){"保存中…"}else{"保存文件"}}</button></div>
    </Modal>}
}

#[component]
fn SkillImport(close: Callback<()>) -> impl IntoView {
    let selected = RwSignal::new(HashSet::<String>::new());
    let conflict = RwSignal::new("skip".to_owned());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let report = RwSignal::new(None::<Vec<Value>>);
    let data = LocalResource::new(|| api::get::<Vec<Value>>("/api/skills/import"));
    Effect::new(move |_| {
        if let Some(Ok(v)) = data.get() {
            selected.set(
                v.iter()
                    .filter(|v| v["exists"] != true)
                    .map(|v| text(v, "source"))
                    .collect(),
            );
        }
    });
    view! {<Modal label="从本机导入技能" class="dlg agent-modal" on_close=Callback::new(move |_| { if !busy.get_untracked() { close.run(()); } })><h3>"从本机导入技能"</h3><p class="muted">"选择要加入技能库的技能，原文件保持不变。"</p><details class="skill-source-details"><summary>"导入来源"</summary><p class="muted mono">"~/.claude/skills · ~/.codex/skills"</p></details>{move||match data.get(){None=>view!{<LoadingState text="正在查找本机技能…"/>}.into_any(),Some(Err(e))=>view!{<InlineError message=format!("无法查找本机技能：{e}") retry=Callback::new(move |_| data.refetch())/>}.into_any(),Some(Ok(v))=>{if v.is_empty(){return view!{<EmptyState title="未找到本机技能" detail="可以先新建技能，或在「发现」中安装。"/>}.into_any();}v.into_iter().map(|v|{let source=text(&v,"source");let src=source.clone();view!{<label class="agent-cap"><input type="checkbox" disabled=move||busy.get() prop:checked=move||selected.with(|s|s.contains(&source)) on:change=move|e|selected.update(|s|{if event_target_checked(&e){s.insert(src.clone());}else{s.remove(&src);}})/><span><b>{text(&v,"name")}</b><small class="muted">{format!("{}{}",text(&v,"source"),if v["exists"]==true{" · 已存在"}else{""})}</small></span></label>}}).collect_view().into_any()}}}<label class="field">"同名冲突"<select disabled=move || busy.get() prop:value=move||conflict.get() on:change=move|e|conflict.set(event_target_value(&e))><option value="skip" prop:selected=move||conflict.get()=="skip">"跳过"</option><option value="rename" prop:selected=move||conflict.get()=="rename">"改名导入"</option><option value="overwrite" prop:selected=move||conflict.get()=="overwrite">"覆盖库里的"</option></select></label>{move || (!error.get().is_empty()).then(|| view! { <InlineError message=error.get()/> })}{move||report.get().map(|r|view!{<div class="card pad">{r.into_iter().map(|v|view!{<p>{format!("{}：{}",text(&v,"source"),text(&v,"result"))}</p>}).collect_view()}</div>})}<div class="agent-save"><button class="btn" disabled=move||busy.get() on:click=move |_|close.run(())>"关闭"</button><button class="btn primary" disabled=move||busy.get()||selected.get().is_empty() on:click=move |_|{busy.set(true);error.set(String::new());spawn_local(async move{if conflict.get_untracked() == "overwrite" && !confirm("覆盖同名技能？", "技能库中的同名文件将被替换，无法撤销。").await { busy.set(false); return; } match api::send::<Vec<Value>>("POST","/api/skills/import",&json!({"sources":selected.get_untracked(),"conflict":conflict.get_untracked()})).await{Ok(r)=>{report.set(Some(r));toast("导入完成");},Err(e)=>error.set(e.to_string())}busy.set(false);});}>{move||if busy.get(){"导入中…"}else{"导入选中技能"}}</button></div></Modal>}
}

#[component]
fn McpEditor(initial: Value, close: Callback<()>) -> impl IntoView {
    let editing = !text(&initial, "id").is_empty();
    let draft = RwSignal::new(initial.clone());
    let initial = StoredValue::new(initial);
    let args = RwSignal::new(
        rows(&draft.get_untracked(), "args")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let env = RwSignal::new(String::new());
    let headers = RwSignal::new(String::new());
    let clear_env = RwSignal::new(false);
    let clear_headers = RwSignal::new(false);
    let pasted = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let original_args = StoredValue::new(args.get_untracked());
    let dirty = RwSignal::new(false);
    Effect::new(move |_| {
        dirty.set(
            draft.get() != initial.get_value()
                || args.get() != original_args.get_value()
                || !env.get().is_empty()
                || !headers.get().is_empty()
                || clear_env.get()
                || clear_headers.get(),
        )
    });
    super::agents::guard_unsaved(dirty);
    let closing = RwSignal::new(false);
    let leave = move || {
        if busy.get_untracked() || closing.get_untracked() {
            return;
        }
        closing.set(true);
        spawn_local(async move {
            if !dirty.get_untracked()
                || confirm("放弃未保存的修改？", "MCP 服务器配置尚未保存。").await
            {
                close.run(());
            }
            let _ = closing.try_set(false);
        });
    };
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        let mut body = draft.get_untracked();
        body["args"] = json!(
            args.get_untracked()
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
        for (key, value, clear) in [
            ("env", env.get_untracked(), clear_env.get_untracked()),
            (
                "headers",
                headers.get_untracked(),
                clear_headers.get_untracked(),
            ),
        ] {
            body[key] = if clear {
                json!({})
            } else if value.trim().is_empty() && editing {
                Value::Null
            } else {
                match parse_kv(&value) {
                    Ok(v) => v,
                    Err(e) => {
                        error.set(e);
                        return;
                    }
                }
            };
        }
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            let path = if editing {
                format!("/api/mcp-servers/{}", api::enc(&text(&body, "id")))
            } else {
                "/api/mcp-servers".into()
            };
            match api::send::<Value>(if editing { "PUT" } else { "POST" }, &path, &body).await {
                Ok(_) => {
                    toast("已保存 MCP 服务器");
                    close.run(());
                }
                Err(e) => error.set(e.to_string()),
            }
            busy.set(false);
        });
    };
    view! {<Modal label="MCP 服务器设置" class="dlg agent-modal skill-modal" on_close=Callback::new(move |_| leave())><h3>{if editing{"编辑 MCP 服务器"}else{"添加 MCP 服务器"}}</h3><fieldset disabled=move||busy.get()><div class="agent-form-grid"><LibraryField draft key="name" label="名称（字母、数字、-、_）"/><label class="field">"连接类型"<select prop:value=move||text(&draft.get(),"transport") on:change=move|e|draft.update(|v|v["transport"]=json!(event_target_value(&e)))><option value="stdio" prop:selected=move||text(&draft.get(),"transport")=="stdio">"STDIO"</option><option value="http" prop:selected=move||text(&draft.get(),"transport")=="http">"Streamable HTTP"</option></select></label></div><LibraryField draft key="description" label="说明"/>
    <div hidden=move||text(&draft.get(),"transport")=="http"><LibraryField draft key="command" label="启动命令"/><label class="field">"参数（一行一个）"<textarea rows="3" class="mono" prop:value=move||args.get() on:input=move|e|args.set(event_target_value(&e))/></label><label class="field">{format!("环境变量，现有键：{}",rows(&initial.get_value(),"env_keys").iter().filter_map(Value::as_str).collect::<Vec<_>>().join("、"))}<textarea rows="3" class="mono" placeholder="KEY=value；留空保留原值" prop:value=move||env.get() on:input=move|e|env.set(event_target_value(&e))/></label>{editing.then(||view!{<label class="agent-cap"><input type="checkbox" prop:checked=move||clear_env.get() on:change=move|e|clear_env.set(event_target_checked(&e))/>"清空现有环境变量"</label>})}</div>
    <div hidden=move||text(&draft.get(),"transport")!="http"><LibraryField draft key="url" label="服务器地址（https://…）"/><label class="field">{format!("请求头，现有键：{}",rows(&initial.get_value(),"header_keys").iter().filter_map(Value::as_str).collect::<Vec<_>>().join("、"))}<textarea rows="3" class="mono" placeholder="Authorization=Bearer …；留空保留原值" prop:value=move||headers.get() on:input=move|e|headers.set(event_target_value(&e))/></label>{editing.then(||view!{<label class="agent-cap"><input type="checkbox" prop:checked=move||clear_headers.get() on:change=move|e|clear_headers.set(event_target_checked(&e))/>"清空现有请求头"</label>})}</div>
    <details><summary>"从 MCP JSON 配置填写"</summary><label class="field">"粘贴 .mcp.json 或单个服务器配置"<textarea rows="5" class="mono" prop:value=move||pasted.get() on:input=move|e|pasted.set(event_target_value(&e))/></label><button class="btn" on:click=move |_|{match serde_json::from_str::<Value>(&pasted.get_untracked()){Ok(mut v)=>{let mut name=String::new();if let Some(obj)=v["mcpServers"].as_object(){if let Some((n,server))=obj.iter().next(){name=n.clone();v=server.clone();}else{error.set("mcpServers 为空".into());return;}}
                if !v.is_object(){error.set("请输入服务器配置对象".into());return;}draft.update(|d|{if text(d,"name").is_empty(){d["name"]=json!(name);}d["transport"]=json!(if text(&v,"url").is_empty(){"stdio"}else{"http"});d["command"]=json!(text(&v,"command"));d["url"]=json!(text(&v,"url"));});args.set(rows(&v,"args").iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"));env.set(kv_text(&v["env"]));headers.set(kv_text(&v["headers"]));clear_env.set(false);clear_headers.set(false);error.set(String::new());toast("已填入表单，检查后保存");},Err(e)=>error.set(format!("JSON 无法解析：{e}"))}}>"填入表单"</button></details></fieldset>{move || (!error.get().is_empty()).then(|| view! { <InlineError message=error.get()/> })}<div class="agent-save"><button class="btn" disabled=move||busy.get() on:click=move |_| leave()>"取消"</button><button class="btn primary" disabled=move||busy.get() on:click=save>{move||if busy.get(){"保存中…"}else{"保存"}}</button></div></Modal>}
}

#[component]
fn LibraryField(draft: RwSignal<Value>, key: &'static str, label: &'static str) -> impl IntoView {
    view! {<label class="field">{label}<input data-modal-initial-focus=(key == "name").then_some("") prop:value=move||text(&draft.get(),key) on:input=move|e|draft.update(|v|v[key]=json!(event_target_value(&e)))/></label>}
}

#[component]
fn SkillMarket(preview: RwSignal<Option<(Value, bool)>>, refresh: RwSignal<u32>) -> impl IntoView {
    let prefs: Value = storage::load("blazar.skills.market").unwrap_or(json!({}));
    let provider = RwSignal::new(if text(&prefs, "provider").is_empty() {
        "skills_sh".into()
    } else {
        text(&prefs, "provider")
    });
    let term = RwSignal::new(if text(&prefs, "provider") == "github" {
        text(&prefs, "repo")
    } else {
        text(&prefs, "q")
    });
    let request = RwSignal::new(None::<(String, String)>);
    let key = RwSignal::new(String::new());
    let key_open = RwSignal::new(false);
    let key_busy = RwSignal::new(false);
    let rev = RwSignal::new(0u32);
    let providers = LocalResource::new(move || {
        rev.track();
        api::get::<Vec<Value>>("/api/skills/market/providers")
    });
    let results = LocalResource::new(move || {
        refresh.track();
        let q = request.get();
        async move {
            let Some((p, q)) = q else {
                return Ok::<_, api::ApiError>(json!({"idle":true}));
            };
            api::get::<Value>(&if p == "github" {
                format!("/api/skills/market/repo?source={}", api::enc(&q))
            } else {
                format!(
                    "/api/skills/market/search?provider={}&q={}",
                    api::enc(&p),
                    api::enc(&q)
                )
            })
            .await
        }
    });
    let search = move || {
        let q = term.get_untracked().trim().to_owned();
        if q.chars().count() < 2 {
            toast("请输入至少两个字符");
            return;
        }
        let p = provider.get_untracked();
        storage::save(
            "blazar.skills.market",
            &json!({"provider":p,"q":q,"repo":q}),
        );
        request.set(Some((p, q)));
    };
    view! {<div><div class="agent-toolbar"><select aria-label="技能来源" prop:value=move||provider.get() on:change=move|e|{provider.set(event_target_value(&e));request.set(None);term.set(String::new());}>{move||providers.get().and_then(Result::ok).map(|p|p.into_iter().map(|v|{let id=text(&v,"id");view!{<option value=id.clone() prop:selected=move||provider.get()==id>{text(&v,"label")}</option>}}).collect_view())}</select><input aria-label="搜索技能或仓库" placeholder=move||if provider.get()=="github"{"owner/repo 或 GitHub 地址"}else{"搜索技能：pdf、code review…"} prop:value=move||term.get() on:input=move|e|term.set(event_target_value(&e)) on:keydown=move|e|{if e.key()=="Enter"&&!e.is_composing(){search();}}/><button class="btn primary" disabled=move || results.get().is_none() on:click=move |_|search()>{move || if results.get().is_none() { "搜索中…" } else { "搜索" }}</button>{move||(provider.get()=="skillsmp").then(||view!{<button class="btn" on:click=move |_|key_open.update(|v|*v = !*v)>"设置 API key"</button>})}</div>
        {move||match providers.get(){None=>view!{<LoadingState text="正在加载技能来源…"/>}.into_any(),Some(Err(e))=>view!{<InlineError message=format!("无法加载技能来源：{e}") retry=Callback::new(move |_| rev.update(|r| *r += 1))/>}.into_any(),Some(Ok(p))=>p.into_iter().find(|p|text(p,"id")==provider.get()).map(|p|view!{<p class="muted">{text(&p,"note")}<a href=text(&p,"home") target="_blank" rel="noopener noreferrer">" 打开来源 ↗"</a></p><div class="agent-toolbar">{rows(&p,"featured").into_iter().filter_map(|v|v.as_str().map(str::to_owned)).map(|repo|{let label=repo.clone();view!{<button class="btn" on:click=move |_|{term.set(repo.clone());search();}>{label}</button>}}).collect_view()}</div>}).into_any()}}
        {move||key_open.get().then(||view!{<div class="card pad"><p class="muted">"密钥只写不读。留空保存会删除现有 key。"</p><div class="agent-toolbar"><input type="password" aria-label="SkillsMP API key" autocomplete="off" disabled=move||key_busy.get() prop:value=move||key.get() on:input=move|e|key.set(event_target_value(&e))/><button class="btn" disabled=move||key_busy.get() on:click=move |_|{if key_busy.get_untracked(){return;}let value=key.get_untracked().trim().to_owned();key_busy.set(true);spawn_local(async move{if value.is_empty()&&!confirm("删除 SkillsMP 密钥？","现有密钥将被删除，需要重新填写才能继续使用需要鉴权的搜索。").await{key_busy.set(false);return;}match api::send::<Value>("PUT","/api/skills/market/key",&json!({"provider":"skillsmp","key":value})).await{Ok(_)=>{toast("已更新密钥");key.set(String::new());key_open.set(false);rev.update(|r|*r+=1);},Err(e)=>toast(e.to_string())}key_busy.set(false);});}>{move || if key_busy.get() { "保存中…" } else { "保存密钥" }}</button></div></div>})}
        {move||match results.get(){None=>view!{<LoadingState text="正在搜索技能…"/>}.into_any(),Some(Err(e))=>view!{<InlineError message=format!("搜索失败：{e}") retry=Callback::new(move |_| results.refetch())/>}.into_any(),Some(Ok(v))=>{if v["idle"]==true{return view!{<EmptyState title="查找需要的技能" detail="输入关键词，或选择一个 GitHub 仓库。"/>}.into_any();}let items=rows(&v,"items");if items.is_empty(){return view!{<EmptyState title="没有找到技能" detail="换一个关键词或来源再试。"/>}.into_any();}view!{<div class="card agent-table"><table><thead><tr><th>"技能"</th><th>"说明"</th><th>"来自"</th><th>"热度"</th><th>"操作"</th></tr></thead><tbody>{items.into_iter().map(|it|{let install=it["install"].clone();view!{<tr><td><b>{text(&it,"name")}</b>{(it["suspicious"]==true).then(||view!{<span class="err-line">" · 平台标记可疑"</span>})}</td><td>{text(&it,"description")}</td><td>{text(&it,"by")}</td><td>{it["count"].as_u64().map(|n|n.to_string()).unwrap_or("—".into())}</td><td><button class="btn" on:click=move |_|preview.set(Some((install.clone(),false)))>{if it["installed"]==true{"已安装 · 查看"}else{"预览安装"}}</button></td></tr>}}).collect_view()}</tbody></table></div>{(v["truncated"]==true).then(||view!{<p class="muted">"仓库较大，只展示了部分技能。可在地址中指定子目录。"</p>})}}.into_any()}}}
    </div>}
}

#[component]
fn MarketPreview(install: Value, update: bool, close: Callback<()>) -> impl IntoView {
    let install = StoredValue::new(install);
    let conflict = RwSignal::new(if update { "overwrite" } else { "rename" }.to_owned());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let data = LocalResource::new(move || async move {
        api::send::<Value>("POST", "/api/skills/market/preview", &install.get_value()).await
    });
    view! {<Modal label="技能安装预览" class="dlg agent-modal skill-modal" on_close=Callback::new(move |_| { if !busy.get_untracked() { close.run(()); } })><div class="agent-toolbar"><h3>{if update{"更新技能"}else{"安装技能"}}</h3><span class="grow"/><button class="btn" disabled=move||busy.get() on:click=move |_|close.run(())>"关闭"</button></div>{move||match data.get(){None=>view!{<LoadingState text="正在获取技能内容…"/>}.into_any(),Some(Err(e))=>view!{<InlineError message=format!("无法加载技能内容：{e}") retry=Callback::new(move |_| data.refetch())/>}.into_any(),Some(Ok(p))=>view!{<h3>{text(&p,"name")}</h3><p class="muted">{format!("版本 {} · {}",text(&p,"version"),text(&p["origin"],"provider"))}<a target="_blank" rel="noopener noreferrer" href=text(&p["origin"],"url")>" 查看原始来源 ↗"</a></p><div class="card notice"><b>"第三方技能包含会执行的指令与脚本，请先检查。安装只加入能力库，不会自动启用。"</b>{rows(&p,"warnings").into_iter().filter_map(|v|v.as_str().map(str::to_owned)).map(|w|view!{<p>{w}</p>}).collect_view()}</div><div class="skill-editor"><nav aria-label="安装文件">{rows(&p,"files").into_iter().map(|f|view!{<div class="skill-file">{format!("{}{} · {} B",if f["script"]==true{"⚙ "}else{""},text(&f,"path"),f["size"].as_u64().unwrap_or_default())}</div>}).collect_view()}</nav><article class="md-body skill-preview" inner_html=md::render(&text(&p,"skill_md"),"SKILL.md","")/></div>{(p["exists"]==true&&!update).then(||view!{<label class="field">"已有同名技能"<select disabled=move || busy.get() prop:value=move||conflict.get() on:change=move|e|conflict.set(event_target_value(&e))><option value="rename" prop:selected=move||conflict.get()=="rename">"改名安装"</option><option value="overwrite" prop:selected=move||conflict.get()=="overwrite">"覆盖已有版本（保留智能体关联）"</option><option value="skip" prop:selected=move||conflict.get()=="skip">"跳过"</option></select></label>})}<div class="agent-save"><button class="btn primary" disabled=move||busy.get() on:click=move |_|{if busy.get_untracked(){return;}busy.set(true);error.set(String::new());let exists=p["exists"]==true;spawn_local(async move{let conflict=if exists||update{conflict.get_untracked()}else{"skip".into()};if conflict == "overwrite" && !confirm("覆盖已有技能？", "现有技能文件将被替换，智能体关联保留。").await { busy.set(false); return; } match api::send::<Value>("POST","/api/skills/market/install",&json!({"install":install.get_value(),"conflict":conflict})).await{Ok(r)=>{toast(format!("已安装 {}，{} 个文件；到智能体的能力页启用",text(&r,"name"),r["files"].as_u64().unwrap_or_default()));close.run(());},Err(e)=>error.set(e.to_string())}busy.set(false);});}>{move||if busy.get(){"安装中…"}else if update{"更新技能"}else{"安装技能"}}</button></div>}.into_any()}}{move || (!error.get().is_empty()).then(|| view! { <InlineError message=error.get()/> })}</Modal>}
}
