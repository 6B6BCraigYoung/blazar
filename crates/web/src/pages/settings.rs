//! 设置保留旧版的存储键，便于用户直接继续使用已有偏好。
use leptos::prelude::*;
use leptos_router::hooks::use_query_map;
use serde_json::{Value, json};

use crate::components::{
    dialog::{Choice, ask},
    toast::toast,
};
use crate::{alerts, api, app_state::use_app, storage};

pub(super) fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().into()
}
pub(super) fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
pub(super) fn rows(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

pub fn apply_ui_preferences() {
    let prefs = ui_prefs();
    if let Some(root) = document().document_element() {
        let _ = root.set_attribute("data-theme", prefs["theme"].as_str().unwrap_or("system"));
    }
    if let Ok(event) = web_sys::Event::new("blazar:preferences") {
        let _ = window().dispatch_event(&event);
    }
}

fn ui_prefs() -> Value {
    let mut defaults =
        json!({"theme":"system","fontSize":12.5,"minimap":true,"wordWrap":false,"sendKey":"enter"});
    if let Some(Value::Object(saved)) = storage::load::<Value>("blazar.ui") {
        defaults.as_object_mut().unwrap().extend(saved);
    }
    defaults
}

fn set_ui(key: &str, value: Value) {
    let mut p = ui_prefs();
    p[key] = value;
    storage::save("blazar.ui", &p);
    apply_ui_preferences();
    toast("已保存");
}

pub(super) async fn save(method: &str, path: &str, body: Value) -> bool {
    match api::send::<Value>(method, path, &body).await {
        Ok(_) => {
            toast("已保存");
            true
        }
        Err(e) => {
            toast(e.to_string());
            false
        }
    }
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    let query = use_query_map();
    let refresh = RwSignal::new(0u32);
    let section = move || query.with(|q| q.get("s").unwrap_or_else(|| "appearance".into()));
    view! {
        <div class="page"><div class="page-head"><h1>"设置"</h1><span class="grow"></span><button class="btn" on:click=move |_| refresh.update(|n| *n += 1)>"刷新"</button></div><div class="settings-layout">
            <nav class="settings-nav" aria-label="设置分类">{[("appearance","外观"),("chat","对话与编辑"),("notify","提醒"),("shortcuts","快捷键"),("rules","自动批准"),("snippets","片段"),("hooks","生命周期钩子"),("git","Git 与 PR"),("platforms","Skill 平台"),("about","数据与关于")].into_iter().map(move |(id,label)| view! {
                <a href=format!("/v2/settings?s={id}") aria-current=move || (section() == id).then_some("page")>{label}</a>
            }).collect_view()}</nav>
            <div class="settings-body">{move || { refresh.get(); match section().as_str() {
                "chat" => view! { <ChatSettings/> }.into_any(),
                "notify" => view! { <NotifySettings/> }.into_any(),
                "shortcuts" => view! { <ShortcutSettings/> }.into_any(),
                "rules" => view! { <RuleSettings/> }.into_any(),
                "snippets" => view! { <SnippetSettings/> }.into_any(),
                "hooks" => view! { <HookSettings/> }.into_any(),
                "git" => view! { <GitSettings/> }.into_any(),
                "platforms" => view! { <PlatformSettings/> }.into_any(),
                "about" => view! { <AboutSettings/> }.into_any(),
                "lark" => view! { <a class="btn" href="/v2/apps/lark">"打开飞书 / Lark 设置"</a> }.into_any(),
                _ => view! { <AppearanceSettings/> }.into_any(),
            }}}</div>
        </div></div>
    }
}

#[component]
fn AppearanceSettings() -> impl IntoView {
    let p = RwSignal::new(ui_prefs());
    let change = move |k: &'static str, v: Value| {
        set_ui(k, v.clone());
        p.update(|p| p[k] = v);
    };
    view! {
        <section class="card settings-card"><h3>"外观"</h3><p class="muted">"存在这台设备上。"</p>
            <div class="settings-row"><label for="settings-theme">"主题"</label><select id="settings-theme" class="settings-input" prop:value=move || text(&p.get(),"theme") on:change=move |e| change("theme",json!(event_target_value(&e)))><option value="system" selected=move || text(&p.get(),"theme")=="system">"跟随系统"</option><option value="light" selected=move || text(&p.get(),"theme")=="light">"浅色"</option><option value="dark" selected=move || text(&p.get(),"theme")=="dark">"深色"</option></select></div>
        </section>
        <section class="card settings-card"><h3>"编辑器"</h3>
            <div class="settings-row"><label for="settings-font">"字号"</label><div class="settings-actions"><input id="settings-font" type="range" min="11" max="18" step="0.5" prop:value=move || p.get()["fontSize"].as_f64().unwrap_or(12.5) on:input=move |e| { if let Ok(n) = event_target_value(&e).parse::<f64>() { change("fontSize",json!(n)); } }/><span>{move || p.get()["fontSize"].to_string()}</span></div></div>
            <div class="settings-row"><label for="settings-minimap">"小地图"</label><input id="settings-minimap" type="checkbox" prop:checked=move || flag(&p.get(),"minimap") on:change=move |e| change("minimap",json!(event_target_checked(&e)))/></div>
            <div class="settings-row"><label for="settings-wrap">"自动换行"</label><input id="settings-wrap" type="checkbox" prop:checked=move || flag(&p.get(),"wordWrap") on:change=move |e| change("wordWrap",json!(event_target_checked(&e)))/></div>
        </section>
    }
}

#[component]
fn ChatSettings() -> impl IntoView {
    let ui = ui_prefs();
    let diff =
        storage::load::<Value>("blazar.diffprefs").unwrap_or(json!({"view":"unified","w":false}));
    let md = storage::load_raw("blazar.md.mode").unwrap_or_else(|| "preview".into());
    let set_diff = |k: &str, v: Value| {
        let mut d = storage::load::<Value>("blazar.diffprefs").unwrap_or(json!({}));
        d[k] = v;
        storage::save("blazar.diffprefs", &d);
        toast("已保存");
    };
    view! {
        <section class="card settings-card"><h3>"对话与编辑"</h3>
            <div class="settings-row"><label for="settings-send">"发送键"</label><select id="settings-send" class="settings-input" prop:value=text(&ui,"sendKey") on:change=|e| set_ui("sendKey",json!(event_target_value(&e)))><option value="enter" selected=text(&ui,"sendKey")=="enter">"Enter 发送，Shift+Enter 换行"</option><option value="mod" selected=text(&ui,"sendKey")=="mod">"⌘ / Ctrl + Enter 发送，Enter 换行"</option></select></div>
            <div class="settings-row"><label for="settings-md">"打开 Markdown 时"</label><select id="settings-md" class="settings-input" prop:value=md.clone() on:change=|e| { storage::save_raw("blazar.md.mode",&event_target_value(&e)); toast("已保存"); }><option value="preview" selected=md=="preview">"先看预览"</option><option value="source" selected=md=="source">"先看源码"</option></select></div>
            <div class="settings-row"><label for="settings-diff">"差异默认视图"</label><select id="settings-diff" class="settings-input" prop:value=text(&diff,"view") on:change=move |e| set_diff("view",json!(event_target_value(&e)))><option value="unified" selected=text(&diff,"view")=="unified">"统一"</option><option value="split" selected=text(&diff,"view")=="split">"并排"</option></select></div>
            <div class="settings-row"><label for="settings-whitespace">"差异里忽略空白"</label><input id="settings-whitespace" type="checkbox" prop:checked=flag(&diff,"w") on:change=move |e| set_diff("w",json!(event_target_checked(&e)))/></div>
        </section>
    }
}

pub(super) const NOTIFY_KINDS: &[(&str, &str)] = &[
    ("run_done", "运行完成"),
    ("run_failed", "运行失败"),
    ("approval", "待审批"),
    ("question", "需要回答"),
    ("autopilot_paused", "自动化暂停"),
    ("rate_limit", "额度告警"),
];

#[component]
fn NotifySettings() -> impl IntoView {
    let app = use_app();
    let sound = RwSignal::new(alerts::sound_prefs());
    let system = RwSignal::new(alerts::notify_on());
    let busy = RwSignal::new(false);
    let server = LocalResource::new(|| api::get::<Value>("/api/settings"));
    let update_sound = move |p: alerts::SoundPrefs| {
        alerts::set_sound_prefs(&p);
        sound.set(p);
        app.alerts_rev.update(|n| *n += 1);
        toast("已保存");
    };
    view! {
        <section class="card settings-card"><h3>"这台设备上怎么提醒"</h3>
            <div class="settings-row"><label for="settings-system-notify">"系统通知"</label><input id="settings-system-notify" type="checkbox" disabled=move || busy.get() || !alerts::notify_supported() prop:checked=move || system.get() on:change=move |e| { let enabled=event_target_checked(&e); system.set(enabled); busy.set(true); leptos::task::spawn_local(async move { let on = !enabled || alerts::notify_granted() || alerts::request_permission().await == "granted"; alerts::set_notify(enabled && on); system.set(enabled && on); app.alerts_rev.update(|n| *n+=1); if enabled && !on { toast("浏览器没有授予通知权限"); } busy.set(false); }); }/></div>
            <div class="settings-row"><label for="settings-sound">"提示音"</label><input id="settings-sound" type="checkbox" prop:checked=move || sound.get().on on:change=move |e| { let mut p=sound.get_untracked(); p.on=event_target_checked(&e); update_sound(p); }/></div>
            <div class="settings-row"><label for="settings-tone">"音色"</label><select id="settings-tone" class="settings-input" prop:value=move || sound.get().tone on:change=move |e| { let mut p=sound.get_untracked(); p.tone=event_target_value(&e); update_sound(p); alerts::play("done",true); }><option value="soft" selected=move || sound.get().tone=="soft">"柔和"</option><option value="bright" selected=move || sound.get().tone=="bright">"明亮"</option><option value="wood" selected=move || sound.get().tone=="wood">"木质"</option></select></div>
            <div class="settings-row"><label for="settings-volume">"音量"</label><input id="settings-volume" type="range" min="0.05" max="1" step="0.05" prop:value=move || sound.get().volume on:change=move |e| { let mut p=sound.get_untracked(); p.volume=event_target_value(&e).parse().unwrap_or(0.5); update_sound(p); }/></div>
            <div class="settings-actions"><button class="btn" on:click=|_| alerts::play("done",true)>"试听：跑完"</button><button class="btn" on:click=|_| alerts::play("attention",true)>"试听：需要处理"</button></div>
        </section>
        {move || match server.get() { None=> view! { <div class="empty">"加载收件箱设置…"</div> }.into_any(), Some(Err(e))=>view! { <p class="err-line">{e.to_string()}</p> }.into_any(),Some(Ok(p))=>view! { <InboxSettings initial=p/> }.into_any() }}
        <section class="card settings-card"><h3>"转发到飞书"</h3><a href="/v2/apps/lark">"配置飞书提醒转发 →"</a></section>
    }
}

#[component]
fn InboxSettings(initial: Value) -> impl IntoView {
    let muted = RwSignal::new(
        rows(&initial["inbox"]["muted"])
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect::<Vec<_>>(),
    );
    let busy = RwSignal::new(false);
    view! { <section class="card settings-card"><h3>"收件箱里哪些算未读"</h3><p class="muted">"静音类型仍会记入收件箱，但不计未读、不响，也不转发。"</p>{NOTIFY_KINDS.iter().map(move |&(key,label)| view! {
        <div class="settings-row"><label for=format!("notify-kind-{key}")>{label}</label><input id=format!("notify-kind-{key}") type="checkbox" disabled=move || busy.get() prop:checked=move || !muted.get().iter().any(|s| s==key) on:change=move |e| { let enabled=event_target_checked(&e); let old=muted.get_untracked(); let mut next=old.clone(); next.retain(|x| x!=key); if !enabled { next.push(key.into()); } muted.set(next.clone()); busy.set(true); leptos::task::spawn_local(async move { if save("PUT","/api/settings",json!({"inbox":{"muted":next}})).await { muted.set(next); } else { muted.set(old); } busy.set(false); }); }/></div>
    }).collect_view()}</section> }
}

pub const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("palette", "命令面板", "Mod+K"),
    ("side", "收起 / 展开侧栏", "Mod+\\"),
    ("explorer", "资源管理器", "Mod+B"),
    ("chat", "对话栏", "Mod+Alt+B"),
    ("panel", "底部面板", "Mod+J"),
    ("newchat", "新对话", "Mod+Shift+N"),
    ("diff", "差异面板", "Mod+Shift+D"),
    ("git", "Git 面板", "Mod+Shift+G"),
    ("preview", "预览面板", "Mod+Shift+P"),
    ("inbox", "收件箱", "Alt+I"),
    ("tasks", "任务", "Alt+T"),
    ("workspaces", "全部工作区", "Alt+W"),
];

#[component]
fn ShortcutSettings() -> impl IntoView {
    let map = RwSignal::new(storage::load::<Value>("blazar.shortcuts").unwrap_or(json!({})));
    let search = RwSignal::new(String::new());
    let recording = RwSignal::new(String::new());
    view! { <section class="card settings-card"><h3>"快捷键"</h3><p class="muted">"点击改键后按下组合；Esc 取消。不带 ⌘ / Ctrl 的组合在输入框内不生效。"</p><input class="settings-input" aria-label="搜索快捷键" placeholder="搜索动作…" on:input=move |e| search.set(event_target_value(&e))/>
            {SHORTCUTS.iter().map(move |&(id,label,default)| view! { <div class="settings-row" style:display=move || if label.contains(&search.get()) { "flex" } else { "none" }><span>{label}</span><div class="settings-actions"><kbd>{move || map.get()[id].as_str().unwrap_or(default).to_owned()}</kbd>
                <button class="btn" on:click=move |_| recording.set(id.into()) on:keydown=move |e| { if recording.get_untracked()!=id { return; } e.prevent_default(); e.stop_propagation(); if e.key()=="Escape" { recording.set(String::new()); return; } let mut parts=Vec::new(); if e.ctrl_key()||e.meta_key(){parts.push("Mod".to_owned());}
    if e.alt_key(){parts.push("Alt".to_owned());}
    if e.shift_key(){parts.push("Shift".to_owned());} let code=e.code(); let key=code.strip_prefix("Key").or_else(||code.strip_prefix("Digit")).unwrap_or(match code.as_str(){"Backslash"=>"\\","Slash"=>"/","Comma"=>",","Period"=>".","Semicolon"=>";","Quote"=>"'","BracketLeft"=>"[","BracketRight"=>"]","Minus"=>"-","Equal"=>"=","Backquote"=>"`","Space"=>"Space",_=>code.as_str()}); if ["ShiftLeft","ShiftRight","MetaLeft","MetaRight","ControlLeft","ControlRight","AltLeft","AltRight"].contains(&key){return;}
    let supported = code.starts_with("Key") || code.starts_with("Digit") || matches!(code.as_str(),"Backslash"|"Slash"|"Comma"|"Period"|"Semicolon"|"Quote"|"BracketLeft"|"BracketRight"|"Minus"|"Equal"|"Backquote"|"Space"|"Enter") || (code.starts_with('F') && code[1..].parse::<u8>().is_ok());
    if !supported { return; }
    if parts.is_empty()&&!key.starts_with('F'){toast("请加上 ⌘ / Ctrl / Alt");return;} parts.push(key.into()); let combo=parts.join("+"); if SHORTCUTS.iter().any(|&(other,_,def)|other!=id&&map.get_untracked()[other].as_str().unwrap_or(def)==combo){toast("这个组合已被其他动作使用");return;} map.update(|m|m[id]=json!(combo));storage::save("blazar.shortcuts",&map.get_untracked());recording.set(String::new());toast("已保存"); }>{move || if recording.get()==id { "按下组合…" }else{"改键"}}</button>
                <button class="btn ghost" on:click=move |_| {map.update(|m|m[id]=json!(""));storage::save("blazar.shortcuts",&map.get_untracked());}>"清除"</button></div></div> }).collect_view()}
            <button class="btn" on:click=move |_| { storage::remove("blazar.shortcuts"); map.set(json!({})); toast("已恢复默认"); }>"全部恢复默认"</button>
        </section> }
}

#[component]
fn GitSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/settings"));
    move || match data.get() {
        None => view! { <div class="empty">"加载中…"</div> }.into_any(),
        Some(Err(e)) => view! { <p class="err-line">{e.to_string()}</p> }.into_any(),
        Some(Ok(p)) => view! { <GitForm initial=p["git"].clone()/> }.into_any(),
    }
}

#[component]
fn GitForm(initial: Value) -> impl IntoView {
    let prefix = RwSignal::new(text(&initial, "branch_prefix"));
    let draft = RwSignal::new(flag(&initial, "pr_draft"));
    let ai = RwSignal::new(flag(&initial, "ai_draft"));
    let busy = RwSignal::new(false);
    view! { <section class="card settings-card"><h3>"Git 与 Pull Request"</h3><form class="settings-form" on:submit=move |e| {e.prevent_default();if busy.get_untracked(){return;}busy.set(true);leptos::task::spawn_local(async move {save("PUT","/api/settings",json!({"git":{"branch_prefix":prefix.get_untracked().trim(),"pr_draft":draft.get_untracked(),"ai_draft":ai.get_untracked()}})).await;busy.set(false);});}>
        <fieldset disabled=move || busy.get()><label class="settings-field">"隔离工作区分支前缀"<input class="settings-input" prop:value=move || prefix.get() on:input=move |e|prefix.set(event_target_value(&e))/></label><span class="muted">{move ||format!("示例：{}fix-login",prefix.get())}</span>
        <label><input type="checkbox" prop:checked=move ||draft.get() on:change=move |e|draft.set(event_target_checked(&e))/>" PR 默认建成草稿"</label><label><input type="checkbox" prop:checked=move ||ai.get() on:change=move |e|ai.set(event_target_checked(&e))/>" 显示 AI 起草提交信息和 PR 描述"</label><div><button type="submit" class="btn primary">"保存"</button></div></fieldset>
    </form></section> }
}

#[component]
fn RuleSettings() -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let tool = RwSignal::new(String::new());
    let pattern = RwSignal::new(String::new());
    let ws = RwSignal::new(String::new());
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/approval-rules")
    });
    view! { <section class="card settings-card"><h3>"自动批准规则"</h3><p class="muted">"命中规则的操作会直接放行，并在对话中注明。默认没有规则；含 shell 控制符的命令永远不会自动批准。"</p>
            {move || match data.get(){None=>view! { <p>"加载中…"</p> }.into_any(),Some(Err(e))=>view! { <p class="err-line">{e.to_string()}</p> }.into_any(),Some(Ok(v))=>{let list=rows(&v);if list.is_empty(){view! { <p class="muted">"还没有规则。"</p> }.into_any()}else{list.into_iter().map(move |r|{let id=text(&r,"id");let del=id.clone();view! { <div class="settings-row"><label class="settings-actions"><input type="checkbox" aria-label="启用规则" prop:checked=flag(&r,"enabled") disabled=move ||busy.get() on:change=move |e|{let enabled=event_target_checked(&e);let id=id.clone();busy.set(true);leptos::task::spawn_local(async move {save("PUT",&format!("/api/approval-rules/{}",api::enc(&id)),json!({"enabled":enabled})).await;rev.update(|n|*n+=1);busy.set(false);});}/><code>{format!("{} {}",text(&r,"tool"),text(&r,"pattern"))}</code><span class="muted">{format!("{} · 命中 {} 次",r["workspace_name"].as_str().unwrap_or(if r["workspace_id"].is_null(){"全局"}else{"工作区已删除"}),r["hits"])}</span></label><button class="btn danger" disabled=move ||busy.get() on:click=move |_|{let id=del.clone();busy.set(true);leptos::task::spawn_local(async move {if save("DELETE",&format!("/api/approval-rules/{}",api::enc(&id)),json!({})).await{rev.update(|n|*n+=1);}busy.set(false);});}>"删除"</button></div> }}).collect_view().into_any()}}}}
            <form class="settings-form" on:submit=move |e|{e.prevent_default();if busy.get_untracked(){return;}
    if tool.get_untracked().trim().is_empty(){toast("请填写工具名称");return;}busy.set(true);leptos::task::spawn_local(async move {let workspace=ws.get_untracked();if save("POST","/api/approval-rules",json!({"tool":tool.get_untracked().trim(),"pattern":pattern.get_untracked().trim(),"workspace_id":if workspace.is_empty(){None}else{Some(workspace)}})).await{tool.set(String::new());pattern.set(String::new());rev.update(|n|*n+=1);}busy.set(false);});}><fieldset disabled=move ||busy.get()><input class="settings-input" aria-label="工具名称" placeholder="Read / Edit / Bash / mcp__x__*" required prop:value=move ||tool.get() on:input=move |e|tool.set(event_target_value(&e))/><input class="settings-input" aria-label="路径或命令前缀" placeholder="路径或命令前缀（可空）" prop:value=move ||pattern.get() on:input=move |e|pattern.set(event_target_value(&e))/><select class="settings-input" aria-label="规则范围" prop:value=move ||ws.get() on:change=move |e|ws.set(event_target_value(&e))><option value="" selected=move ||ws.get().is_empty()>"全局"</option>{move ||app.state.get().map(|s|s.workspaces.iter().map(|w|{let id=w.id.clone();view! { <option value=w.id.clone() selected=move ||ws.get()==id>{w.name.clone()}</option> }}).collect_view())}</select><div><button class="btn primary" type="submit">"添加规则"</button></div></fieldset></form>
        </section> }
}

#[component]
fn SnippetSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let edit = RwSignal::new(None::<String>);
    let name = RwSignal::new(String::new());
    let body = RwSignal::new(String::new());
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/snippets")
    });
    view! { <section class="card settings-card"><h3>"片段"</h3><p class="muted">"把验收清单、代码规范等常用内容存下来，在对话里输入 @ 选择片段。"</p>
        {move ||match data.get(){None=>view! { <p>"加载中…"</p> }.into_any(),Some(Err(e))=>view! { <p class="err-line">{e.to_string()}</p> }.into_any(),Some(Ok(v))=>{let list=rows(&v);if list.is_empty(){view! { <p class="muted">"还没有片段。"</p> }.into_any()}else{list.into_iter().map(move |s|{let id=text(&s,"id");let editing=s.clone();view! { <div class="settings-row"><div class="settings-label"><b>{format!("@{}",text(&s,"name"))}</b><span>{text(&s,"body").chars().take(100).collect::<String>()}</span></div><button class="btn" disabled=move ||busy.get() on:click=move |_|{edit.set(Some(text(&editing,"id")));name.set(text(&editing,"name"));body.set(text(&editing,"body"));}>"编辑"</button><button class="btn danger" disabled=move ||busy.get() on:click=move |_|{let id=id.clone();busy.set(true);leptos::task::spawn_local(async move {if ask("删除片段","删除后不能恢复。",vec![Choice::plain("取消"),Choice::danger("删除")]).await==Some(1)&&save("DELETE",&format!("/api/snippets/{}",api::enc(&id)),json!({})).await{if edit.get_untracked().as_ref()==Some(&id){edit.set(None);name.set(String::new());body.set(String::new());}rev.update(|n|*n+=1);}busy.set(false);});}>"删除"</button></div> }}).collect_view().into_any()}}}}
        <form class="settings-form" on:submit=move |e|{e.prevent_default();if busy.get_untracked(){return;}busy.set(true);leptos::task::spawn_local(async move {let id=edit.get_untracked();let path=id.as_ref().map(|id|format!("/api/snippets/{}",api::enc(id))).unwrap_or_else(||"/api/snippets".into());if save(if id.is_some(){"PUT"}else{"POST"},&path,json!({"name":name.get_untracked().trim(),"body":body.get_untracked()})).await{edit.set(None);name.set(String::new());body.set(String::new());rev.update(|n|*n+=1);}busy.set(false);});}><fieldset disabled=move ||busy.get()><label class="settings-field">"名称"<input class="settings-input" required maxlength="40" prop:value=move ||name.get() on:input=move |e|name.set(event_target_value(&e))/></label><label class="settings-field">"内容"<textarea class="settings-textarea" required prop:value=move ||body.get() on:input=move |e|body.set(event_target_value(&e))></textarea></label><div class="settings-actions"><button type="submit" class="btn primary">{move ||if edit.get().is_some(){"保存片段"}else{"新建片段"}}</button><button type="button" class="btn" on:click=move |_|{edit.set(None);name.set(String::new());body.set(String::new());}>"清空 / 取消编辑"</button></div></fieldset></form>
    </section> }
}

#[component]
fn HookSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let busy = RwSignal::new(false);
    let event = RwSignal::new("turn_start".to_owned());
    let target = RwSignal::new("hub".to_owned());
    let command = RwSignal::new(String::new());
    let timeout = RwSignal::new(30u32);
    let blocking = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/hooks")
    });
    view! { <section class="card settings-card"><h3>"生命周期钩子"</h3><p class="muted">"在关键时点执行脚本。一轮开始前的钩子可在失败时否决执行。变量：BLAZAR_EVENT、BLAZAR_WORKSPACE、BLAZAR_NODE、BLAZAR_CWD、BLAZAR_SESSION、BLAZAR_TOOL。"</p>
            {move ||match data.get(){None=>view! { <p>"加载中…"</p> }.into_any(),Some(Err(e))=>view! { <p class="err-line">{e.to_string()}</p> }.into_any(),Some(Ok(v))=>{let list=rows(&v);if list.is_empty(){view! { <p class="muted">"还没有钩子。"</p> }.into_any()}else{list.into_iter().map(move |h|{let id=text(&h,"id");view! { <div class="settings-row"><div class="settings-label"><code>{text(&h,"command")}</code><span>{format!("{} · {}{}",text(&h,"event"),text(&h,"target"),if flag(&h,"blocking"){" · 失败则否决"}else{""})}</span></div><button class="btn danger" disabled=move ||busy.get() on:click=move |_|{let id=id.clone();busy.set(true);leptos::task::spawn_local(async move {if save("DELETE",&format!("/api/hooks/{}",api::enc(&id)),json!({})).await{rev.update(|n|*n+=1);}busy.set(false);});}>"删除"</button></div> }}).collect_view().into_any()}}}}
            <form class="settings-form" on:submit=move |e|{e.prevent_default();if busy.get_untracked(){return;}
    if command.get_untracked().trim().is_empty(){toast("请填写命令");return;}busy.set(true);leptos::task::spawn_local(async move {if save("POST","/api/hooks",json!({"event":event.get_untracked(),"target":target.get_untracked(),"command":command.get_untracked().trim(),"blocking":blocking.get_untracked()&&event.get_untracked()=="turn_start","timeout_secs":timeout.get_untracked()})).await{command.set(String::new());rev.update(|n|*n+=1);}busy.set(false);});}><fieldset disabled=move ||busy.get()><label class="settings-field">"时点"<select class="settings-input" prop:value=move ||event.get() on:change=move |e|event.set(event_target_value(&e))><option value="turn_start" selected=move ||event.get()=="turn_start">"一轮开始前"</option><option value="turn_end" selected=move ||event.get()=="turn_end">"一轮结束"</option><option value="session_start" selected=move ||event.get()=="session_start">"会话建立"</option><option value="session_end" selected=move ||event.get()=="session_end">"会话结束"</option><option value="error" selected=move ||event.get()=="error">"出错"</option></select></label><label class="settings-field">"执行位置"<select class="settings-input" prop:value=move ||target.get() on:change=move |e|target.set(event_target_value(&e))><option value="hub" selected=move ||target.get()=="hub">"中心"</option><option value="node" selected=move ||target.get()=="node">"干活那台机器"</option></select></label><label class="settings-field">"命令"<input class="settings-input" required prop:value=move ||command.get() on:input=move |e|command.set(event_target_value(&e))/></label><label class="settings-field">"超时（秒）"<input class="settings-input" type="number" min="1" max="600" value="30" on:input=move |e|timeout.set(event_target_value(&e).parse().unwrap_or(30))/></label><label><input type="checkbox" disabled=move ||event.get()!="turn_start" prop:checked=move ||blocking.get() on:change=move |e|blocking.set(event_target_checked(&e))/>" 失败则否决"</label><div><button class="btn primary" type="submit">"添加钩子"</button></div></fieldset></form>
        </section> }
}

#[component]
fn PlatformSettings() -> impl IntoView {
    let rev = RwSignal::new(0);
    let key = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let data = LocalResource::new(move || {
        rev.get();
        api::get::<Value>("/api/skills/market/providers")
    });
    view! { <section class="card settings-card"><h3>"Skill 平台"</h3><p><a href="/v2/skills?tab=market">"搜索和安装技能 →"</a></p>{move ||match data.get(){None=>view! { <p>"加载中…"</p> }.into_any(),Some(Err(e))=>view! { <p class="err-line">{e.to_string()}</p> }.into_any(),Some(Ok(v))=>rows(&v).into_iter().map(|p|view! { <div class="settings-row"><div class="settings-label"><b>{text(&p,"label")}</b><span>{text(&p,"note")}</span></div><span>{if flag(&p,"has_key"){"API key 已填"}else{"匿名"}}</span><a href=text(&p,"home") target="_blank" rel="noopener noreferrer">"打开 ↗"</a></div> }).collect_view().into_any()}}
        <form class="settings-form" on:submit=move |e|{e.prevent_default();if busy.get_untracked(){return;}busy.set(true);leptos::task::spawn_local(async move {if save("PUT","/api/skills/market/key",json!({"provider":"skillsmp","key":key.get_untracked().trim()})).await{key.set(String::new());rev.update(|n|*n+=1);}busy.set(false);});}><label class="settings-field">"SkillsMP API key（只写不读，留空删除）"<input class="settings-input" type="password" autocomplete="new-password" prop:value=move ||key.get() disabled=move ||busy.get() on:input=move |e|key.set(event_target_value(&e))/></label><div><button class="btn primary" type="submit" disabled=move ||busy.get()>"保存 API key"</button></div></form>
    </section> }
}

#[component]
fn AboutSettings() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<Value>("/api/about"));
    view! {
            {move ||match data.get(){None=>view! { <p>"加载中…"</p> }.into_any(),Some(Err(e))=>view! { <p class="err-line">{e.to_string()}</p> }.into_any(),Some(Ok(a))=>view! {
                <section class="card settings-card"><h3>"Blazar"</h3><div class="settings-row"><span>"版本"</span><code>{text(&a,"version")}</code></div><div class="settings-row"><span>"Hub 地址"</span><code>{a["hub_url"].as_str().filter(|s|!s.is_empty()).map(str::to_owned).unwrap_or_else(||window().location().origin().unwrap_or_default())}</code></div><p class="muted">"文档在仓库 README.md。Blazar 代码使用 MIT 许可证。"</p></section>
                <section class="card settings-card"><h3>"第三方许可证"</h3><p>{format!("组网引擎：{} {} · {}",text(&a["engine"],"name"),text(&a["engine"],"version"),text(&a["engine"],"license"))}</p><a href=text(&a["engine"],"source") target="_blank" rel="noopener noreferrer">"对应版本源码 ↗"</a><p class="muted">"Monaco Editor、xterm.js：MIT；IBM Plex Sans、JetBrains Mono：SIL OFL 1.1。完整声明见 THIRD-PARTY-NOTICES.md。"</p></section>
                <section class="card settings-card"><h3>"数据"</h3><p>{format!("本机 SQLite：{:.1} MB",a["db_bytes"].as_f64().unwrap_or_default()/1048576.0)}</p>{a["counts"].as_object().map(|counts|counts.iter().map(|(k,v)|view! { <div class="settings-row"><span>{k.clone()}</span><span>{v.to_string()}</span></div> }).collect_view())}</section>
            }.into_any()}}
            <section class="card settings-card"><h3>"这台设备上的偏好"</h3><p class="muted">"恢复主题、布局、快捷键、提醒和视图默认值。工作区、对话和任务数据保留。"</p><button class="btn danger" on:click=move |_|leptos::task::spawn_local(async move {if ask("恢复默认偏好","恢复这台设备上的界面偏好？",vec![Choice::plain("取消"),Choice::danger("恢复默认")]).await!=Some(1){return;}
    if let Ok(Some(store))=window().local_storage(){let keys=(0..store.length().unwrap_or(0)).filter_map(|i|store.key(i).ok().flatten()).filter(|k|["blazar.ui","blazar.shortcuts","blazar.sound","blazar.notify","blazar.md.mode","blazar.diffprefs","blazar.preview","blazar.an.days","blazar.v2.ws.panel","blazar.v2.ws.layout","blazar.todosOpen","blazar.editor"].contains(&k.as_str())).collect::<Vec<_>>();for k in keys {let _=store.remove_item(&k);}}apply_ui_preferences();toast("已恢复默认，刷新页面后布局和快捷键全部生效");})>"恢复默认"</button></section>
        }
}
