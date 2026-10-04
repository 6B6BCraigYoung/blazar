use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{self, Accounts};
use crate::components::dialog::{self, Choice};
use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::components::toast::toast;
use crate::realtime::use_bus;
use crate::runtime_config::{AgentConfig, ConfigDrafts, DraftField};

use super::accounts::provider_label;
use super::{AccountsSection, WITH_ACCOUNTS};

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct AgentSpec {
    id: String,
    label: String,
    #[serde(default)]
    program: String,
    #[serde(default)]
    structured: bool,
    #[serde(default)]
    resume: bool,
}

#[component]
pub(super) fn RuntimeDetail(id: String) -> impl IntoView {
    let bus = use_bus();
    let app = crate::app_state::use_app();
    let multi = WITH_ACCOUNTS.iter().any(|x| x.0 == id);
    let view_tab = RwSignal::new(if multi { "accounts" } else { "general" });
    let rev = RwSignal::new(0u32);
    let sid = id.clone();
    let spec = LocalResource::new(move || {
        let id = sid.clone();
        async move {
            api::get::<Vec<AgentSpec>>("/api/agents")
                .await
                .map(|l| l.into_iter().find(|a| a.id == id))
        }
    });
    let cid = id.clone();
    let configs = LocalResource::new(move || {
        rev.track();
        let id = cid.clone();
        async move {
            api::get::<Vec<AgentConfig>>("/api/agent-configs")
                .await
                .map(|l| {
                    l.into_iter()
                        .filter(|c| c.agent_id == id)
                        .collect::<Vec<_>>()
                })
        }
    });
    let accounts = LocalResource::new(move || {
        bus.accounts.track();
        api::get::<Accounts>("/api/accounts")
    });
    let nodes = move || {
        let mut v = vec![String::new()];
        v.extend(app.state.with(|s| {
            s.as_ref()
                .map(|s| s.nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        }));
        v
    };
    let id_s = StoredValue::new(id.clone());
    let node = RwSignal::new(String::new());
    let drafts = StoredValue::new(ConfigDrafts::default());
    let saving = RwSignal::new(false);
    let save = move |node: String, f: Box<dyn FnOnce(&mut AgentConfig)>| {
        if saving.get_untracked() {
            return;
        }
        let Some(Ok(mine)) = configs.get_untracked() else {
            toast("配置尚未加载，请重试后保存");
            return;
        };
        let mut c = mine
            .into_iter()
            .find(|c| c.node.clone().unwrap_or_default() == node)
            .unwrap_or(AgentConfig {
                max_concurrent: 1,
                enabled: true,
                ..AgentConfig::default()
            });
        c.agent_id = id_s.get_value();
        c.node = (!node.is_empty()).then_some(node.clone());
        c.enabled = true;
        f(&mut c);
        let section = view_tab.get_untracked();
        saving.set(true);
        spawn_local(async move {
            let body = json!({
                "node": c.node, "agent_id": c.agent_id, "program_path": c.program_path, "model": c.model,
                "permission_mode": c.permission_mode, "custom_args": c.custom_args, "custom_env": c.custom_env,
                "max_concurrent": c.max_concurrent.max(1), "enabled": true,
            });
            let result = api::send::<Value>("POST", "/api/agent-configs", &body).await;
            match result {
                Ok(_) => {
                    let _ = drafts.try_update_value(|drafts| drafts.saved(&node, section));
                    let _ = configs.try_set(None);
                    toast("已保存");
                    let _ = rev.try_update(|n| *n += 1);
                }
                Err(e) => toast(format!("保存失败：{e}")),
            }
            let _ = saving.try_set(false);
        });
    };
    let save = StoredValue::new_local(save);

    view! {
        <div class="page wide runtimes-page">
            <div class="page-head">
                <a href="/runtimes" class="crumb">"运行时"</a><span class="sep">"/"</span>
                <h1>{move || spec.get().and_then(Result::ok).flatten().map(|s| s.label).unwrap_or_else(|| provider_label(&id_s.get_value()).to_owned())}</h1>
            </div>
            {move || match spec.get() {
                None => view! { <LoadingState text="正在加载运行时…"/> }.into_any(),
                Some(Err(error)) => view! { <InlineError message=format!("无法加载运行时：{error}") retry=Callback::new(move |_| spec.refetch())/> }.into_any(),
                Some(Ok(None)) => view! { <EmptyState title="未找到运行时" detail="返回运行时列表重新扫描。"/> }.into_any(),
                Some(Ok(Some(s))) => view! {
                    <details class="runtime-details"><summary>"运行时详情"</summary><p class="muted small mono">{s.program}</p>
                        {(!s.structured).then(|| view! { <span class="gchip warn">"纯文本输出"</span> })}
                        {(!s.resume).then(|| view! { <span class="gchip">"不支持续接"</span> })}
                    </details>
                }.into_any(),
            }}
            <div class="vt-wrap">
                <nav class="vtabs" aria-label="运行时设置">
                    {multi.then(|| view! { <button class="vtab" aria-pressed=move || view_tab.get() == "accounts" disabled=move || saving.get() data-active=move || (view_tab.get() == "accounts").to_string() on:click=move |_| view_tab.set("accounts")>"账号"</button> })}
                    <button class="vtab" aria-pressed=move || view_tab.get() == "general" disabled=move || saving.get() data-active=move || (view_tab.get() == "general").to_string() on:click=move |_| view_tab.set("general")>"配置"</button>
                    <button class="vtab" aria-pressed=move || view_tab.get() == "env" disabled=move || saving.get() data-active=move || (view_tab.get() == "env").to_string() on:click=move |_| view_tab.set("env")>"环境变量"</button>
                    <button class="vtab" aria-pressed=move || view_tab.get() == "args" disabled=move || saving.get() data-active=move || (view_tab.get() == "args").to_string() on:click=move |_| view_tab.set("args")>"自定义参数"</button>
                </nav>
                <div class="vt-body">
                    {move || {
                        let tab = view_tab.get();
                        if tab == "accounts" {
                            return view! {
                            <details class="card pad runtime-details">
                                <summary>"账号与凭据"</summary>
                                <div class="muted small">
                                    "每个账号是一个独立的 CLI 配置目录。浏览器登录的账号由官方 CLI 自己保管凭据，Blazar 不读取；长期 token 存在账号目录里只有你能读的文件中，启动时才交给 CLI。点「使用」切换这个运行时用的账号；对话里也可以从运行时选择器单独换。设置、技能和会话历史在各账号之间共享，换账号后上下文不变。"
                                    {move || accounts.get().and_then(Result::ok).map(|a| format!(" 账号目录在 {}。", a.root))}
                                </div>
                            </details>
                            <section class="card"><AccountsSection provider=id_s.get_value() accounts/></section>
                        }.into_any();
                        }
                        let configs_value = match configs.get() {
                            None => return view! { <LoadingState text="正在加载配置…"/> }.into_any(),
                            Some(Err(error)) => return view! {
                                <InlineError message=format!("无法加载配置：{error}") retry=Callback::new(move |_| rev.update(|n| *n += 1))/>
                            }.into_any(),
                            Some(Ok(configs)) => configs,
                        };
                        let selected_node = node.get();
                        let draft = RwSignal::new(drafts.with_value(|drafts| drafts.get(&configs_value, &selected_node, tab)));
                        let draft_key = StoredValue::new((selected_node, tab.to_owned()));
                        let change = Callback::new(move |(field, value): (DraftField, String)| {
                            draft.update(|draft| draft.set(field, value));
                            let (node, section) = draft_key.get_value();
                            drafts.update_value(|drafts| drafts.remember(node, section, draft.get_untracked()));
                        });
                        match tab {
                        "general" => {
                            let modes = crate::pages::workspace::chat::state::modes_for(&id_s.get_value());
                            let program = spec.get().and_then(Result::ok).flatten().map(|s| s.program).unwrap_or_default();
                            view! {
                                <div class="card pad">
                                    <h3>"配置"</h3>
                                    <div class="muted small">"按机器单独设置，或保留全局默认。"</div>
                                    <div class="grid2">
                                        <label class="field">"适用机器"<select disabled=move || saving.get() prop:value=move || node.get() on:change=move |e| node.set(event_target_value(&e))>
                                            {move || nodes().into_iter().map(|n| { let l = if n.is_empty() { "（全局默认）".to_owned() } else { n.clone() }; let selected = n == node.get_untracked(); view! { <option value=n selected=selected>{l}</option> } }).collect_view()}
                                        </select></label>
                                        <label class="field">"程序路径"<input placeholder=program disabled=move || saving.get() prop:value=move || draft.get().path on:input=move |e| change.run((DraftField::Path, event_target_value(&e)))/><span class="muted small">"安装在自定义位置时填写"</span></label>
                                        <label class="field">"模型"<input placeholder="默认" disabled=move || saving.get() prop:value=move || draft.get().model on:input=move |e| change.run((DraftField::Model, event_target_value(&e)))/></label>
                                        <label class="field">"最大并发"<input type="number" min="1" max="16" disabled=move || saving.get() prop:value=move || draft.get().concurrency on:input=move |e| change.run((DraftField::Concurrency, event_target_value(&e)))/></label>
                                    </div>
                                    <label class="field">"权限模式"
                                        <select disabled=move || saving.get() prop:value=move || draft.get().permission on:change=move |e| change.run((DraftField::Permission, event_target_value(&e)))>
                                            <option value="" selected=move || draft.get().permission.is_empty()>"默认"</option>
                                            {modes.iter().map(|m| view! { <option value=m.0 selected=move || draft.get().permission == m.0>{m.1}</option> }).collect_view()}
                                        </select>
                                        {modes.is_empty().then(|| view! { <span class="muted small">"此运行时不支持权限模式"</span> })}
                                    </label>
                                    <div class="row-actions"><button class="btn primary" disabled=move || saving.get() on:click=move |_| {
                                        let draft = draft.get_untracked();
                                        let Some(concurrency) = draft.concurrency.trim().parse::<i64>().ok().filter(|n| (1..=16).contains(n)) else {
                                            toast("最大并发需为 1 到 16 的整数");
                                            return;
                                        };
                                        save.with_value(|f| f(node.get_untracked(), Box::new(move |x| {
                                            x.program_path = Some(draft.path.trim().to_owned()).filter(|s| !s.is_empty());
                                            x.model = Some(draft.model.trim().to_owned()).filter(|s| !s.is_empty());
                                            x.permission_mode = Some(draft.permission).filter(|s| !s.is_empty());
                                            x.max_concurrent = concurrency;
                                        })));
                                    }>{move || if saving.get() { "保存中…" } else { "保存配置" }}</button></div>
                                </div>
                                {move || configs.get().and_then(Result::ok).filter(|l| !l.is_empty()).map(|l| view! {
                                    <div class="card pad">
                                        <h3>"已有配置"</h3>
                                        <table class="tb">
                                            <thead><tr><th>"机器"</th><th>"路径"</th><th>"模型"</th><th>"并发"</th><th></th></tr></thead>
                                            <tbody>
                                                {l.into_iter().map(|c| {
                                                    let cid = c.id.clone();
                                                    view! {
                                                        <tr>
                                                            <td>{c.node.clone().unwrap_or_else(|| "（全局默认）".into())}</td>
                                                            <td class="mono">{c.program_path.clone().unwrap_or_else(|| "—".into())}</td>
                                                            <td class="mono">{c.model.clone().unwrap_or_else(|| "—".into())}</td>
                                                            <td class="mono">{c.max_concurrent}</td>
                                                            <td class="runtime-row-action"><button class="btn small danger" disabled=move || saving.get() on:click=move |_| {
                                                                if saving.get_untracked() { return; }
                                                                saving.set(true);
                                                                let cid = cid.clone();
                                                                spawn_local(async move {
                                                                    if dialog::ask("删除配置", "删除这项已保存的配置？此操作无法撤销。", vec![Choice::plain("保留配置"), Choice::danger("删除配置")]).await != Some(1) {
                                                                        let _ = saving.try_set(false);
                                                                        return;
                                                                    }
                                                                    match api::send::<Value>("DELETE", &format!("/api/agent-configs/{cid}"), &json!({})).await {
                                                                        Ok(_) => { toast("已删除"); rev.update(|n| *n += 1); }
                                                                        Err(e) => toast(format!("删除失败：{e}")),
                                                                    }
                                                                    let _ = saving.try_set(false);
                                                                });
                                                            }>"删除"</button></td>
                                                        </tr>
                                                    }
                                                }).collect_view()}
                                            </tbody>
                                        </table>
                                    </div>
                                })}
                            }.into_any()
                        }
                        "env" => {
                            view! {
                                <div class="card pad">
                                    <h3>"环境变量"</h3>
                                    <div class="muted small">"需要代理时可填写 HTTPS_PROXY。请勿在这里保存账号凭据。"</div>
                                    <label class="field">"每行一个 KEY=value"<textarea class="mono" rows="8" placeholder="HTTPS_PROXY=http://127.0.0.1:9527" disabled=move || saving.get() prop:value=move || draft.get().env on:input=move |e| change.run((DraftField::Env, event_target_value(&e)))></textarea></label>
                                    <label class="field">"适用机器"<select disabled=move || saving.get() prop:value=move || node.get() on:change=move |e| node.set(event_target_value(&e))>
                                        {move || nodes().into_iter().map(|n| { let l = if n.is_empty() { "（全局默认）".to_owned() } else { n.clone() }; let sel = n == node.get_untracked(); view! { <option value=n selected=sel>{l}</option> } }).collect_view()}
                                    </select></label>
                                    <div class="row-actions"><button class="btn primary" disabled=move || saving.get() on:click=move |_| {
                                        let env: std::collections::BTreeMap<String, String> = draft.get_untracked().env.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
                                            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))).filter(|(k, _)| !k.is_empty()).collect();
                                        save.with_value(|f| f(node.get_untracked(), Box::new(move |x| x.custom_env = env)));
                                    }>{move || if saving.get() { "保存中…" } else { "保存配置" }}</button></div>
                                </div>
                            }.into_any()
                        }
                        _ => {
                            view! {
                                <div class="card pad">
                                    <h3>"自定义参数"</h3>
                                    <div class="muted small">"每行一个参数，启动时追加到命令行。"</div>
                                    <label class="field">"参数"<textarea class="mono" rows="6" placeholder="--verbose" disabled=move || saving.get() prop:value=move || draft.get().args on:input=move |e| change.run((DraftField::Args, event_target_value(&e)))></textarea></label>
                                    <label class="field">"适用机器"<select disabled=move || saving.get() prop:value=move || node.get() on:change=move |e| node.set(event_target_value(&e))>
                                        {move || nodes().into_iter().map(|n| { let l = if n.is_empty() { "（全局默认）".to_owned() } else { n.clone() }; let selected = n == node.get_untracked(); view! { <option value=n selected=selected>{l}</option> } }).collect_view()}
                                    </select></label>
                                    <div class="row-actions"><button class="btn primary" disabled=move || saving.get() on:click=move |_| {
                                        let args: Vec<String> = draft.get_untracked().args.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
                                        save.with_value(|f| f(node.get_untracked(), Box::new(move |x| x.custom_args = args)));
                                    }>{move || if saving.get() { "保存中…" } else { "保存配置" }}</button></div>
                                </div>
                            }.into_any()
                        }
                    }}}
                </div>
            </div>
        </div>
    }
}
