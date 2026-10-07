use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;
use serde_json::{Value, json};

use crate::api;
use crate::app_state::{AppData, use_app};
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::components::{
    modal::Modal,
    status::{EmptyState, InlineError, LoadingState},
};
use crate::files_js;
use wasm_bindgen::closure::Closure;

use super::workspace::activity_label;

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn latency(ms: Option<f64>) -> AnyView {
    match ms {
        None => view! { <span class="muted">"—"</span> }.into_any(),
        Some(ms) => {
            let cls = if ms < 60.0 {
                "ok"
            } else if ms < 150.0 {
                "warn"
            } else {
                "bad"
            };
            view! { <span class=format!("lat {cls}")>{format!("{ms:.0} ms")}</span> }.into_any()
        }
    }
}

fn date_of(t: i64) -> String {
    if t == 0 {
        return "—".into();
    }
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0));
    format!(
        "{}/{}/{}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date()
    )
}

fn days_left(t: i64) -> i64 {
    ((t as f64 * 1000.0 - js_sys::Date::now()) / 86_400_000.0)
        .round()
        .max(0.0) as i64
}

#[derive(Clone, Copy)]
struct Mesh {
    app: AppData,
    refreshing: RwSignal<bool>,
    operation: RwSignal<bool>,
    local: RwSignal<u32>,
    issuer: RwSignal<u32>,
    invites: RwSignal<u32>,
    config: RwSignal<Option<(String, Option<String>)>>,
    preview: RwSignal<Option<(Value, String)>>,
    join: RwSignal<Option<(Value, Option<String>)>>,
}

pub fn provide_mesh() {
    provide_context(Mesh {
        app: use_app(),
        refreshing: RwSignal::new(false),
        operation: RwSignal::new(false),
        local: RwSignal::new(0),
        issuer: RwSignal::new(0),
        invites: RwSignal::new(0),
        config: RwSignal::new(None),
        preview: RwSignal::new(None),
        join: RwSignal::new(None),
    });
}

async fn refresh_mesh(m: Mesh) {
    if m.refreshing.get_untracked() {
        return;
    }
    m.refreshing.set(true);
    match api::send::<Value>("POST", "/api/mesh/refresh", &json!({})).await {
        Ok(r) => toast(format!(
            "发现 {} 个节点",
            r["discovered"].as_u64().unwrap_or(0)
        )),
        Err(e) => toast(format!("刷新失败：{e}")),
    }
    m.refreshing.set(false);
    m.local.update(|n| *n += 1);
    m.invites.update(|n| *n += 1);
    m.app.load_state();
}

async fn pending_invite(m: Mesh) {
    if let Ok(v) = api::get::<Value>("/api/mesh/local").await {
        if v["pending"].is_object() {
            m.join.set(Some((v["pending"].clone(), None)));
        } else if let Some(e) = v["pending_error"].as_str() {
            toast(format!("无法使用这个邀请：{e}"));
        }
    }
}

#[component]
pub fn MeshHost() -> impl IntoView {
    let m = expect_context::<Mesh>();
    spawn_local(pending_invite(m));
    let callback = Closure::<dyn FnMut()>::new(move || spawn_local(pending_invite(m)));
    let _ = js_sys::Reflect::set(&window(), &"blazarInvite".into(), callback.as_ref());
    let callback = StoredValue::new_local(callback);
    let over = window_event_listener(leptos::ev::dragover, |e| {
        if e.data_transfer().is_some_and(|d| {
            (0..d.items().length())
                .filter_map(|i| d.items().get(i))
                .any(|i| i.kind() == "file")
        }) {
            e.prevent_default();
        }
    });
    let dropped = window_event_listener(leptos::ev::drop, move |e| {
        let file = e.data_transfer().and_then(|d| d.files()).and_then(|l| {
            (0..l.length())
                .filter_map(|i| l.get(i))
                .find(|f| f.name().to_lowercase().ends_with(".blazar"))
        });
        if let Some(f) = file {
            e.prevent_default();
            spawn_local(open_mesh_file(m, f));
        }
    });
    on_cleanup(move || {
        let _ = js_sys::Reflect::delete_property(&window(), &"blazarInvite".into());
        over.remove();
        dropped.remove();
        callback.dispose();
    });
    view! { {move || m.join.get().map(|(p, text)| view! { <JoinDialog m p text/> })} }
}

async fn open_mesh_file(m: Mesh, f: web_sys::File) {
    if m.operation.get_untracked() {
        toast("组网操作正在进行，请等待完成");
        return;
    }
    let name = f.name();
    if name.to_lowercase().ends_with(".toml") {
        if f.size() > 262_144.0 {
            toast("配置文件过大");
            return;
        }
        let Some(text) = files_js::read_text(&f).await else {
            toast("无法读取文件，请重新选择");
            return;
        };
        preview_config(m, text).await;
    } else {
        if f.size() > 16384.0 {
            toast("这不是 Blazar 邀请文件");
            return;
        }
        let Some(text) = files_js::read_text(&f).await else {
            toast("无法读取文件，请重新选择");
            return;
        };
        match api::send::<Value>("POST", "/api/mesh/join/preview", &json!({ "invite": text })).await
        {
            Ok(p) => m.join.set(Some((p, Some(text)))),
            Err(e) => toast(format!("无法使用这个邀请：{e}")),
        }
    }
}

async fn preview_config(m: Mesh, text: String) {
    match api::send::<Value>("POST", "/api/mesh/config/preview", &json!({ "toml": text })).await {
        Ok(p) => {
            m.config.set(None);
            m.preview.set(Some((p, text)));
        }
        Err(e) => m.config.set(Some((text, Some(e.to_string())))),
    }
}

const BLANK_TOML: &str = "# EasyTier 配置（完整字段见 https://easytier.cn/guide/network/configurations.html）\nhostname = \"my-machine\"\ndhcp = true\n\n[network_identity]\nnetwork_name = \"my-mesh\"\nnetwork_secret = \"\"\n\n# 枢纽（公网可达的那台）的入网地址\n[[peer]]\nuri = \"tcp://203.0.113.10:11010\"\n\n# 子网代理：把本机所在的局域网暴露给组网\n# [[proxy_network]]\n# cidr = \"192.168.1.0/24\"\n\n# 端口转发\n# [[port_forward]]\n# bind_addr = \"0.0.0.0:8080\"\n# dst_addr = \"10.99.0.11:80\"\n# proto = \"tcp\"\n\n[flags]\n# latency_first = true\n";

#[component]
pub fn NodesPage() -> impl IntoView {
    let app = use_app();
    let q = RwSignal::new(String::new());
    let m = expect_context::<Mesh>();
    let invite_in = NodeRef::<leptos::html::Input>::new();
    let toml_in = NodeRef::<leptos::html::Input>::new();
    let ssh_pick = RwSignal::new(false);
    let nodes = move || {
        app.state
            .with(|s| s.as_ref().map(|s| s.nodes.clone()).unwrap_or_default())
    };
    let bus = crate::realtime::use_bus();
    let topo = LocalResource::new(move || {
        m.local.track();
        bus.nodes.track();
        api::get::<Value>("/api/mesh/topology")
    });
    let on_file = move |e: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&e);
        if let Some(f) = input.files().and_then(|l| l.get(0)) {
            spawn_local(open_mesh_file(m, f));
        }
        input.set_value("");
    };
    view! {
        <div class="page nodes-page" on:dragover=|e: leptos::ev::DragEvent| e.prevent_default()
            on:drop=move |e: leptos::ev::DragEvent| {
                let files = e.data_transfer().and_then(|d| d.files());
                let f = files.and_then(|l| (0..l.length()).filter_map(|i| l.get(i)).find(|f| { let n = f.name().to_lowercase(); n.ends_with(".blazar") || n.ends_with(".toml") }));
                if let Some(f) = f { e.prevent_default(); e.stop_propagation(); spawn_local(open_mesh_file(m, f)); }
            }>
            <div class="page-head">
                <h1>"机器与组网"</h1>
                <span class="muted small">{move || { let n = nodes(); format!("{} 台开发机在线", n.iter().filter(|x| x.is_usable() && x.status == "online").count()) }}</span>
                <span class="grow"></span>
                <button class="btn primary" on:click=move |_| ssh_pick.set(true)>"添加机器"</button>
                <input type="file" node_ref=invite_in accept=".blazar" hidden on:change=on_file/>
                <input type="file" node_ref=toml_in accept=".toml,text/plain" hidden on:change=on_file/>
            </div>
            <p class="muted nodes-intro">"选择一台机器，创建或打开工作区。"</p>
            <PendingMachines nodes=Signal::derive(nodes)/>
            <section class="card pad">
                <div class="card-title">
                    <h3>"开发机"</h3><span class="muted small">{move || format!("{} 台", nodes().iter().filter(|n| n.is_usable()).count())}</span>
                    <span class="grow"></span>
                    <input class="page-filter" aria-label="筛选机器" placeholder="搜索机器" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
                    <button class="btn small" disabled=move || m.refreshing.get() on:click=move |_| spawn_local(refresh_mesh(m))>{move || if m.refreshing.get() { "发现中…" } else { "发现组网机器" }}</button>
                </div>
                <details class="nodes-details"><summary>"机器从哪里来"</summary>
                    <p class="muted small">"组网机器会自动出现；也可以从本机 SSH 配置中选择机器。SSH 机器无需加入组网。"</p>
                    <div class="muted small">{move || topo.get().and_then(Result::ok).map(|t| s(&t, "source")).filter(|x| !x.is_empty()).map(|x| format!("发现来源：{x}"))}</div>
                </details>
                {move || topo.get().and_then(Result::err).map(|e| view! { <InlineError message=format!("无法读取组网机器：{e}") retry=Callback::new(move |()|m.local.update(|n|*n+=1))/> })}
                {move || app.state_error.get().map(|message| view! { <InlineError message retry=Callback::new(move |_| app.load_state())/> })}
                {move || topo.get().and_then(Result::ok).filter(|t| s(t, "source").is_empty()).map(|_| view! { <TopoFix m/> })}
                <div class="ws-list nodes">
                    {move || {
                        if app.state.with(|state| state.is_none()) {
                            return if app.state_error.with(Option::is_some) {
                                ().into_any()
                            } else {
                                view! { <LoadingState text="正在读取机器…"/> }.into_any()
                            };
                        }
                        let k = q.get().to_lowercase();
                        let list: Vec<_> = nodes().into_iter().filter(|n| n.is_usable()).filter(|n| k.is_empty() || n.name.to_lowercase().contains(&k) || (n.name == "local" && "本机".contains(&k))).collect();
                        if list.is_empty() {
                            return view! { <EmptyState title=if k.is_empty() { "还没有可用机器" } else { "没有匹配的机器" } detail=if k.is_empty() { "添加 SSH 机器，或发现已加入组网的机器。" } else { "换个名称搜索，或清空筛选。" }/> }.into_any();
                        }
                        list.into_iter().map(|n| {
                            let name2 = n.name.clone();
                            let name3 = n.name.clone();
                            let is_ssh = n.network.as_deref() == Some("ssh");
                            let is_mesh = n.network.as_deref() == Some("easytier");
                            let removable = is_ssh && n.workspace_count == 0;
                            let name4 = n.name.clone();
                            view! {
                                <article class="ws-card node-card">
                                    <div class="top"><a class="node-name" href=format!("/nodes/{}", api::enc(&n.name))>{if n.name == "local" { "本机".to_owned() } else { n.name.clone() }}</a><span class="state-pill" data-act=if n.status == "online" { "completed" } else { "idle" }>{if n.status == "online" { "在线" } else { "离线" }}</span></div>
                                    <div class="meta"><span>{if n.name == "local" { "当前电脑" } else if is_ssh { "SSH 连接" } else { "组网连接" }}</span><span>{format!("{} 个工作区", n.workspace_count)}</span></div>
                                    <div class="nodes-details">
                                        <div class="kv"><span class="k">"标识"</span><span class="v mono">{n.name.clone()}</span></div>
                                        <div class="kv"><span class="k">"地址"</span><span class="v mono" title=n.ipv4.clone().unwrap_or_default()>{n.ipv4.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| "—".into())}</span></div>
                                        <div class="kv"><span class="k">"连接方式"</span><span class="v">{if n.name == "local" { "本机".to_owned() } else if is_ssh { "SSH".to_owned() } else { n.cost.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "组网".into()) }}</span></div>
                                        <div class="kv"><span class="k">"延迟"</span><span class="v">{latency(if is_mesh { n.latency_ms } else { None })}</span></div>
                                    </div>
                                    <div class="act">
                                        <a class="btn small" href=format!("/nodes/{}", api::enc(&n.name))>"查看机器"</a>
                                        {removable.then(|| view! { <button class="btn small danger" title="从 Blazar 移除，不修改 SSH 配置" on:click=move |_| remove_ssh(name3.clone())>"移除"</button> })}
                                        {is_mesh.then(|| view! { <button class="btn small ghost" title="改为个人电脑：不再用来运行任务" on:click=move |_| set_roles(vec![(name4.clone(), Some("personal"))])>"不是开发机"</button> })}
                                        <button class="btn small primary" on:click=move |_| { app.new_ws_node.set(Some(name2.clone())); app.new_ws.set(true); }>"新建工作区"</button>
                                    </div>
                                </article>
                            }
                        }).collect_view().into_any()
                    }}
                </div>
            </section>
            <PersonalMachines nodes=Signal::derive(nodes)/>
            <div class="nodes-section-heading"><h2>"团队组网"</h2><span class="muted small">"由 EasyTier 提供"</span></div>
            <MeshLocal m invite_in toml_in/>
            <Issuer m/>
            <Invites m/>
            {move || ssh_pick.get().then(|| view! { <SshPicker on_close=move || ssh_pick.set(false)/> })}
            {move || m.config.get().map(|(text, err)| view! { <ConfigEditor m text err/> })}
            {move || m.preview.get().map(|(p, text)| view! { <ConfigPreview m p text/> })}
        </div>
    }
}

fn set_roles(roles: Vec<(String, Option<&'static str>)>) {
    let roles: serde_json::Map<String, Value> = roles
        .into_iter()
        .map(|(name, role)| (name, json!(role)))
        .collect();
    spawn_local(async move {
        if let Err(e) =
            api::send::<Value>("PUT", "/api/nodes/roles", &json!({ "roles": roles })).await
        {
            toast(format!("设置机器类型失败：{e}"));
        }
    });
}

#[component]
fn PendingMachines(nodes: Signal<Vec<blazar_core_types::api::NodeView>>) -> impl IntoView {
    let pending = move || {
        nodes
            .get()
            .into_iter()
            .filter(|n| n.is_pending())
            .collect::<Vec<_>>()
    };
    move || {
        let list = pending();
        (!list.is_empty()).then(|| {
            let all: Vec<String> = list.iter().map(|n| n.name.clone()).collect();
            view! {
                <section class="card pad node-pending" aria-label="新发现的组网机器">
                    <div class="card-title">
                        <h3>"新发现的组网机器"</h3><span class="muted small">{format!("{} 台待确认", list.len())}</span>
                        <span class="grow"></span>
                        <button class="btn small" on:click=move |_| set_roles(all.iter().map(|n| (n.clone(), Some("personal"))).collect())>"其余都是个人电脑"</button>
                    </div>
                    <p class="muted small">"选出可以运行任务的开发机。个人电脑只显示在线状态，不会出现在选择机器的地方。"</p>
                    <div class="pending-list">
                        {list.into_iter().map(|n| {
                            let (dev, personal) = (n.name.clone(), n.name.clone());
                            view! {
                                <div class="pending-row">
                                    <span class="state-pill" data-act=if n.status == "online" { "completed" } else { "idle" }>{if n.status == "online" { "在线" } else { "离线" }}</span>
                                    <span class="pending-name">{n.name.clone()}</span>
                                    <span class="muted small mono">{n.ipv4.clone().unwrap_or_default()}</span>
                                    <span class="muted small">{latency(n.latency_ms)}</span>
                                    <span class="grow"></span>
                                    <button class="btn small primary" on:click=move |_| set_roles(vec![(dev.clone(), Some("dev"))])>"开发机"</button>
                                    <button class="btn small" on:click=move |_| set_roles(vec![(personal.clone(), Some("personal"))])>"个人电脑"</button>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                </section>
            }
        })
    }
}

#[component]
fn PersonalMachines(nodes: Signal<Vec<blazar_core_types::api::NodeView>>) -> impl IntoView {
    move || {
        let list: Vec<_> = nodes
            .get()
            .into_iter()
            .filter(|n| n.is_personal())
            .collect();
        (!list.is_empty()).then(|| {
            let online = list.iter().filter(|n| n.status == "online").count();
            view! {
                <details class="card pad nodes-personal">
                    <summary><b>"个人电脑"</b><span class="muted small">{format!("{} 台 · {} 台在线", list.len(), online)}</span></summary>
                    <p class="muted small">"这些电脑只显示在线状态，不会用来运行任务。"</p>
                    <div class="pending-list">
                        {list.into_iter().map(|n| {
                            let (dev, reset) = (n.name.clone(), n.name.clone());
                            view! {
                                <div class="pending-row">
                                    <span class="state-pill" data-act=if n.status == "online" { "completed" } else { "idle" }>{if n.status == "online" { "在线" } else { "离线" }}</span>
                                    <span class="pending-name">{n.name.clone()}</span>
                                    <span class="muted small mono">{n.ipv4.clone().unwrap_or_default()}</span>
                                    <span class="grow"></span>
                                    <button class="btn small" on:click=move |_| set_roles(vec![(dev.clone(), Some("dev"))])>"改为开发机"</button>
                                    {(n.network.as_deref() == Some("easytier")).then(|| view! { <button class="btn small ghost" title="放回待确认" on:click=move |_| set_roles(vec![(reset.clone(), None)])>"重新确认"</button> })}
                                </div>
                            }
                        }).collect_view()}
                    </div>
                </details>
            }
        })
    }
}

fn remove_ssh(name: String) {
    spawn_local(async move {
        let body = "只是从 Blazar 的机器列表里去掉，~/.ssh/config 不动，以后还能再加回来。";
        let choices = vec![Choice::plain("取消"), Choice::danger("移除")];
        if dialog::ask(&format!("移除 {name}？"), body, choices).await != Some(1) {
            return;
        }
        match api::send::<Value>(
            "DELETE",
            &format!("/api/ssh-hosts/{}", api::enc(&name)),
            &json!({}),
        )
        .await
        {
            Ok(_) => toast(format!("已移除 {name}")),
            Err(e) => toast(format!("移除失败：{e}")),
        }
    });
}

#[component]
fn SshPicker(#[prop(into)] on_close: Callback<()>) -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0u32);
    let hosts = LocalResource::new(move || {
        rev.track();
        api::get::<Vec<Value>>("/api/ssh-hosts")
    });
    let q = RwSignal::new(String::new());
    let picked = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let busy = RwSignal::new(false);
    let add = move |_| {
        let names: Vec<String> = picked.get_untracked().into_iter().collect();
        if names.is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>("POST", "/api/ssh-hosts", &json!({ "names": names })).await {
                Ok(r) => {
                    let n = r["added"].as_array().map_or(0, Vec::len);
                    toast(format!("已添加 {n} 台机器，正在检查能不能连上"));
                    app.load_state();
                    on_close.run(());
                }
                Err(e) => {
                    let _ = busy.try_set(false);
                    toast(format!("添加失败：{e}"));
                }
            }
        });
    };
    view! {
        <Modal label="添加 SSH 机器" class="dlg ssh-pick nodes-dialog" on_close=Callback::new(move |()| { if !busy.get_untracked() { on_close.run(()); } })>
                <h3>"添加 SSH 机器"</h3>
                <p class="muted small">"选择要使用的机器，即可创建工作区。"</p><details class="nodes-details"><summary>"连接方式"</summary><p class="muted small">"读取本机 ~/.ssh/config，沿用 SSH 密钥和 ProxyJump 设置。添加后无需加入组网，也能创建工作区、同步 Claude Code / Codex 版本。"</p></details>
                <input class="page-filter" aria-label="筛选 SSH 机器" placeholder="搜索名称或地址" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
                <div class="ssh-list">
                    {move || match hosts.get() {
                        None => view! { <LoadingState text="正在读取 SSH 机器…"/> }.into_any(),
                        Some(Err(e)) => view! { <InlineError message=format!("无法读取 SSH 配置：{e}") retry=Callback::new(move |()| rev.update(|n| *n += 1))/> }.into_any(),
                        Some(Ok(list)) => {
                            let k = q.get().to_lowercase();
                            let rows: Vec<Value> = list.into_iter().filter(|h| k.is_empty() || s(h, "alias").to_lowercase().contains(&k) || s(h, "hostname").to_lowercase().contains(&k)).collect();
                            if rows.is_empty() {
                                return view! { <EmptyState title=if k.is_empty() { "没有可添加的机器" } else { "没有匹配的机器" } detail=if k.is_empty() { "先在 ~/.ssh/config 中添加 SSH Host，再重试。" } else { "换个名称或地址搜索。" }/> }.into_any();
                            }
                            rows.into_iter().map(|h| {
                                let alias = s(&h, "alias");
                                let added = h["added"].as_bool() == Some(true);
                                let mesh = h["in_mesh"].as_bool() == Some(true);
                                let user = s(&h, "user");
                                let port = s(&h, "port");
                                let target = format!("{}{}{}",
                                    if user.is_empty() { String::new() } else { format!("{user}@") },
                                    Some(s(&h, "hostname")).filter(|x| !x.is_empty()).unwrap_or_else(|| alias.clone()),
                                    if port.is_empty() { String::new() } else { format!(":{port}") });
                                let file = s(&h, "file");
                                let file = file.rsplit('/').next().unwrap_or_default().to_owned();
                                let a1 = alias.clone();
                                let a2 = alias.clone();
                                view! {
                                    <label class="ssh-row" data-off=(added || mesh).to_string()>
                                        <input type="checkbox" disabled=move || busy.get() || added || mesh
                                            prop:checked=move || added || picked.with(|p| p.contains(&a1))
                                            on:change=move |e| { let on = event_target_checked(&e); let a = a2.clone(); picked.update(|p| { if on { p.insert(a); } else { p.remove(&a); } }); }/>
                                        <b class="mono">{alias.clone()}</b>
                                        <span class="mono muted">{target}</span>
                                        <span class="grow"></span>
                                        {if mesh { Some(view! { <span class="gchip">"在组网里"</span> }.into_any()) } else if added { Some(view! { <span class="gchip">"已添加"</span> }.into_any()) } else { (file != "config").then(|| view! { <span class="muted small">{file}</span> }.into_any()) }}
                                    </label>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                </div>
                <div class="dlg-foot">
                    <button class="btn" disabled=move || busy.get() on:click=move |_| on_close.run(())>"取消"</button>
                    <button class="btn primary" disabled=move || busy.get() || picked.with(|p| p.is_empty()) on:click=add>
                        {move || { let n = picked.with(|p| p.len()); if busy.get() { "添加中…".to_owned() } else if n == 0 { "添加".to_owned() } else { format!("添加 {n} 台") } }}
                    </button>
                </div>
        </Modal>
    }
}

#[component]
fn TopoFix(m: Mesh) -> impl IntoView {
    let busy = RwSignal::new(false);
    let via = RwSignal::new(String::new());
    let ct = RwSignal::new(String::new());
    view! {
        <div class="notice-box warn">
            "暂时无法发现组网机器。填写运行 EasyTier 的机器，通过 SSH 读取。"
            <div class="row-actions">
                <input class="mono small-in" aria-label="发现来源的 SSH Host" placeholder="SSH Host，如 hub-host" disabled=move || busy.get() prop:value=move || via.get() on:input=move |e| via.set(event_target_value(&e))/>
                <input class="mono small-in" aria-label="EasyTier 容器名（可选）" placeholder="容器名（可选）" disabled=move || busy.get() prop:value=move || ct.get() on:input=move |e| ct.set(event_target_value(&e))/>
                <button class="btn small primary" disabled=move || busy.get() on:click=move |_| {
                    if busy.get_untracked() { return; }
                    let v = via.get_untracked().trim().to_owned();
                    if v.is_empty() { toast("填一个 SSH Host"); return; }
                    let c = Some(ct.get_untracked().trim().to_owned()).filter(|x| !x.is_empty());
                    busy.set(true);
                    spawn_local(async move {
                        match api::send::<Value>("PUT", "/api/mesh/issuer", &json!({ "via": v, "container": c })).await {
                            Ok(_) => { toast("已设置，正在发现节点…"); gloo_timers::future::TimeoutFuture::new(4000).await; m.local.update(|n| *n += 1); m.issuer.update(|n| *n += 1); m.app.load_state(); }
                            Err(e) => toast(format!("设置失败：{e}")),
                        }
                        busy.try_set(false);
                    });
                }>{move || if busy.get() { "读取中…" } else { "读取机器" }}</button>
            </div>
        </div>
    }
}

#[component]
fn MeshLocal(
    m: Mesh,
    invite_in: NodeRef<leptos::html::Input>,
    toml_in: NodeRef<leptos::html::Input>,
) -> impl IntoView {
    let local = LocalResource::new(move || {
        m.local.track();
        api::get::<Value>("/api/mesh/local")
    });
    let ext_rpc = RwSignal::new(String::new());
    let rpc_busy = RwSignal::new(false);
    let kv = |k: &'static str, v: String, mono: bool| view! { <div class="kv"><span class="k">{k}</span><span class="v" class:mono=mono>{v}</span></div> };
    view! {
        <section class="card pad">
            {move || match local.get() {
                None => view! { <LoadingState text="正在读取组网状态…" class="muted small"/> }.into_any(),
                Some(Err(e)) => view! { <h3>"本机组网"</h3><InlineError message=format!("无法读取组网状态：{e}") retry=Callback::new(move |()|m.local.update(|n|*n+=1))/> }.into_any(),
                Some(Ok(v)) => {
                    let st = v["status"].clone();
                    let n = st["node"].clone();
                    let other = (st["joined"].as_bool() == Some(true)).then(|| st["other_engine"].as_str().map(|o| view! {
                        <div class="notice-box warn">{format!("本机还在运行另一个 EasyTier（{o}）。两个实例连同一个网络会互相抢路由，建议退出它。")}</div>
                    })).flatten();
                    if st["joined"].as_bool() != Some(true) && st["external"].is_object() {
                        let x = st["external"].clone();
                        let xn = x["node"].clone();
                        let label = s(&x, "label");
                        return view! {
                            <div class="card-title"><h3>"本机组网"</h3><span class="state-pill" data-act="completed">"已连接"</span></div>
                            <p class="muted small">"已使用现有组网连接，无需重新加入。"</p><div class="nodes-details"><p class="muted small">{format!("连接由 {label} 提供。Blazar 复用这个连接，避免两个 EasyTier 实例争用同一网段的路由。")}</p>
                            {(!s(&xn, "network_name").is_empty()).then(|| kv("网络", s(&xn, "network_name"), true))}
                            {kv("虚拟地址", Some(s(&x, "virtual_ipv4")).filter(|a| !a.is_empty()).unwrap_or_else(|| "未能识别".into()), true)}
                            {(!s(&xn, "hostname").is_empty()).then(|| kv("主机名", s(&xn, "hostname"), true))}
                            {xn.is_object().then(|| kv("可见节点", x["peer_count"].as_u64().unwrap_or(0).to_string(), false))}
                            {xn.is_object().then(|| kv("NAT", Some(s(&xn, "nat_type")).filter(|a| !a.is_empty()).unwrap_or_else(|| "—".into()), false))}
                            {(!s(&x, "rpc").is_empty()).then(|| kv("RPC", s(&x, "rpc"), true))}
                            {s(&x, "rpc").is_empty().then(|| view! {
                                <div class="notice-box">{format!("在 {label} 设置里把「RPC 门户」设为 127.0.0.1:15888，可看到本机 NAT 与可见节点数。")}
                                    <div class="row-actions">
                                        <input class="mono small-in" aria-label="EasyTier RPC 地址" placeholder="127.0.0.1:15888" disabled=move || rpc_busy.get() prop:value=move || ext_rpc.get() on:input=move |e| ext_rpc.set(event_target_value(&e))/>
                                        <button class="btn small" disabled=move || rpc_busy.get() on:click=move |_| { if rpc_busy.get_untracked() {return;} rpc_busy.set(true); let r = ext_rpc.get_untracked(); spawn_local(async move {
                                            match api::send::<Value>("PUT", "/api/mesh/external-rpc", &json!({ "rpc": r.trim() })).await {
                                                Ok(v) => { toast(if v["status"]["external"]["rpc"].is_string() { "已连上" } else { "还连不上 —— 确认 GUI 里设置的地址一致" }); m.local.update(|n| *n += 1); }
                                                Err(e) => toast(e.to_string()),
                                            }
                                            rpc_busy.try_set(false);
                                        }); }>"连接"</button>
                                    </div>
                                </div>
                            })}
                            </div><div class="row-actions"><button class="btn small" disabled=move || m.refreshing.get() on:click=move |_| spawn_local(async move { refresh_mesh(m).await; })>"刷新机器"</button>
                                <button class="btn small" disabled=move || m.operation.get() on:click=move |_| if let Some(i) = invite_in.get_untracked() { i.click() }>"使用邀请文件"</button>
                                <button class="btn small" disabled=move || m.operation.get() on:click=move |_| if let Some(i) = toml_in.get_untracked() { i.click() }>"导入 EasyTier 配置"</button>
                            </div>
                        }.into_any();
                    }
                    if st["joined"].as_bool() != Some(true) {
                        return view! {
                            <div class="card-title"><h3>"本机组网"</h3><span class="state-pill" data-act="idle">"未加入"</span></div>
                            {(st["engine_bundled"].as_bool() != Some(true)).then(|| view! { <div class="notice-box bad">"这个安装包没有带组网引擎，请安装完整版桌面端。"</div> })}
                            {other}
                            <div class="joinways">
                                <button class="joinway" on:click=move |_| if let Some(i) = invite_in.get_untracked() { i.click() }>
                                    <b>"用邀请文件加入"</b><span>"选择管理员发来的 .blazar 文件。"</span>
                                </button>
                                <button class="joinway" on:click=move |_| if let Some(i) = toml_in.get_untracked() { i.click() }>
                                    <b>"导入 EasyTier 配置"</b><span>"沿用 config.toml 中的密钥、代理、端口转发和 ACL。"</span>
                                </button>
                            </div>
                            <div class="row-actions"><button class="linkbtn" on:click=move |_| m.config.set(Some((BLANK_TOML.to_owned(), None)))>"新建 EasyTier 配置"</button>
                                <span class="muted small">"也可将 .blazar 或 .toml 文件拖到此处。"</span></div>
                        }.into_any();
                    }
                    let running = st["running"].as_bool() == Some(true);
                    view! {
                        <div class="card-title"><h3>"本机组网"</h3>
                            {if running { view! { <span class="state-pill" data-act="completed">"已连接"</span> }.into_any() } else { view! { <span class="state-pill bad" data-act="errored">"未连接"</span> }.into_any() }}
                        </div>
                        <div class="nodes-details">
                        {kv("可见机器", st["peer_count"].as_u64().unwrap_or(0).to_string(), false)}
                        {kv("网络", Some(s(&n, "network_name")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into()), true)}
                        {kv("主机名", Some(s(&n, "hostname")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into()), true)}
                        {kv("虚拟地址", Some(s(&n, "virtual_ipv4")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into()), true)}
                        {kv("NAT", Some(s(&n, "nat_type")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into()), false)}
                        {kv("引擎", format!("EasyTier {}", Some(s(&n, "version")).filter(|x| !x.is_empty()).unwrap_or_else(|| s(&st, "engine_version"))), true)}
                        </div>
                        {other}
                        {(!running).then(|| view! { <div class="notice-box warn">"组网服务已停止。请重新打开邀请文件加入；排查日志位于系统组网目录的 logs/。"</div> })}
                        <div class="row-actions">
                            <button class="btn small" disabled=move || m.refreshing.get() on:click=move |_| spawn_local(refresh_mesh(m))>"刷新机器"</button>
                            <button class="btn small" disabled=move || m.operation.get() on:click=move |_| spawn_local(async move {
                                match api::get::<Value>("/api/mesh/config").await { Ok(c) => m.config.set(Some((s(&c, "toml"), None))), Err(e) => toast(format!("读取配置失败：{e}")) }
                            })>"编辑 EasyTier 配置"</button>
                            <button class="btn small ghost" disabled=move || m.operation.get() on:click=move |_| if let Some(i) = toml_in.get_untracked() { i.click() }>"导入其他配置"</button>
                            <button class="btn small ghost" disabled=move || m.operation.get() on:click=move |_| if let Some(i) = invite_in.get_untracked() { i.click() }>"使用邀请文件"</button>
                            <span class="grow"></span>
                            <button class="btn small danger" disabled=move || m.operation.get() on:click=move |_| spawn_local(async move {
                                if m.operation.get_untracked() {return;}
                                if dialog::ask("退出组网", "退出组网后，本机上的组网凭据会被删除，其它机器将无法再连到本机。确定？", vec![Choice::plain("取消"), Choice::danger("退出")]).await != Some(1) { return; }
                                if m.operation.get_untracked() {return;}
                                m.operation.set(true);
                                toast("等待系统授权…");
                                match api::send::<Value>("POST", "/api/mesh/leave", &json!({})).await {
                                    Ok(_) => { toast("已退出组网"); m.local.update(|n| *n += 1); m.invites.update(|n| *n += 1); m.app.load_state(); }
                                    Err(e) => toast(format!("退出失败：{e}")),
                                }
                                m.operation.set(false);
                            })>"退出组网"</button>
                        </div>
                    }.into_any()
                }
            }}
            <Show when=move || !matches!(local.get(), Some(Ok(_)))>
                <div class="row-actions">
                    <button class="btn small" disabled=move || m.operation.get() on:click=move |_| if let Some(i) = invite_in.get_untracked() { i.click() }>"用邀请文件加入"</button>
                    <button class="btn small" disabled=move || m.operation.get() on:click=move |_| if let Some(i) = toml_in.get_untracked() { i.click() }>"导入 EasyTier 配置"</button>
                </div>
            </Show>
        </section>
    }
}

#[component]
fn ConfigEditor(m: Mesh, text: String, err: Option<String>) -> impl IntoView {
    let t = RwSignal::new(text);
    let err = RwSignal::new(err);
    let busy = RwSignal::new(false);
    let initial_values = StoredValue::new(t.get_untracked());
    let dirty = RwSignal::new(false);
    let confirming_close = RwSignal::new(false);
    Effect::new(move |_| dirty.set(t.get() != initial_values.get_value()));
    super::agents::guard_unsaved(dirty);
    let close = Callback::new(move |()| {
        if busy.get_untracked() || confirming_close.get_untracked() {
            return;
        }
        if t.get_untracked() == initial_values.get_value() {
            m.config.try_set(None);
            return;
        }
        confirming_close.set(true);
        spawn_local(async move {
            if dialog::ask(
                "放弃修改",
                "EasyTier 配置尚未应用。关闭后，本次修改将丢失。",
                vec![Choice::plain("继续编辑"), Choice::danger("放弃修改")],
            )
            .await
                == Some(1)
                && confirming_close.try_get_untracked().is_some()
            {
                m.config.try_set(None);
            }
            confirming_close.try_set(false);
        });
    });
    view! {
        <Modal label="EasyTier 配置" class="dlg wide nodes-dialog" on_close=close>
                <h3>"EasyTier 配置"</h3>
                <div class="muted small">"应用后将替换本机组网配置并重启服务，需要系统授权。配置含组网密钥，将保存在本机私密位置。"</div>
                <textarea class="mono cfgedit" aria-label="EasyTier TOML 配置" disabled=move || busy.get() spellcheck="false" prop:value=move || t.get() on:input=move |e| t.set(event_target_value(&e))></textarea>
                {move || err.get().map(|e| view! { <InlineError message=e/> })}
                <div class="dlg-foot">
                    <button class="btn" disabled=move || busy.get() on:click=move |_| close.run(())>"取消"</button>
                    <button class="btn primary" disabled=move || busy.get() on:click=move |_| { if busy.get_untracked(){return;}busy.set(true);let text = t.get_untracked(); spawn_local(async move {
                        match api::send::<Value>("POST", "/api/mesh/config/preview", &json!({ "toml": text })).await {
                            Ok(p) => { m.config.set(None); m.preview.set(Some((p, text))); }
                            Err(e) => { err.try_set(Some(e.to_string())); },
                        }
                        busy.try_set(false);
                    }); }>{move || if busy.get() { "校验中…" } else { "预览配置" }}</button>
                </div>
        </Modal>
    }
}

#[component]
fn ConfigPreview(m: Mesh, p: Value, text: String) -> impl IntoView {
    let sm = p["summary"].clone();
    let list = |k: &str| {
        sm[k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let peers = list("peers");
    let listeners = list("listeners");
    let features = list("features");
    let warnings = list("warnings");
    let busy = m.operation;
    let can = p["can_join"].as_bool() == Some(true);
    let net = s(&sm, "network_name");
    let t2 = text.clone();
    view! {
        <Modal label="应用 EasyTier 配置" class="dlg nodes-dialog" on_close=Callback::new(move |()| { if !busy.get_untracked() { m.preview.set(None); } })>
                <h3>"应用 EasyTier 配置"</h3>
                <div class="kv"><span class="k">"网络"</span><span class="v mono">{net.clone()}</span></div>
                <div class="kv"><span class="k">"身份"</span><span class="v">{if s(&sm, "auth") == "network_secret" { "网络密钥（管理员 / 枢纽）" } else { "个人凭据" }}</span></div>
                <details class="nodes-details"><summary>"配置详情"</summary><div class="kv"><span class="k">"主机名"</span><span class="v mono">{Some(s(&sm, "hostname")).filter(|x| !x.is_empty()).unwrap_or_else(|| "（系统主机名）".into())}</span></div>
                <div class="kv"><span class="k">"地址"</span><span class="v mono">{Some(s(&sm, "ipv4")).filter(|x| !x.is_empty()).unwrap_or_else(|| if sm["dhcp"].as_bool() == Some(true) { "自动分配".into() } else { "无".into() })}</span></div>
                <div class="kv"><span class="k">"对端"</span><span class="v mono">{peers.join("\n")}</span></div>
                {(!listeners.is_empty()).then(|| view! { <div class="kv"><span class="k">"监听"</span><span class="v mono">{listeners.join("\n")}</span></div> })}
                {(!features.is_empty()).then(|| view! { <div class="kv"><span class="k">"功能"</span><span class="v">{features.into_iter().map(|f| view! { <span class="gchip info">{f}</span>" " }).collect_view()}</span></div> })}
                </details>
                {warnings.into_iter().map(|w| view! { <div class="notice-box warn">{w}</div> }).collect_view()}
                <p class="muted small">"应用后将替换本机组网配置并重启服务，需要系统授权。"</p><div class="dlg-foot">
                    <button class="btn ghost" disabled=move || busy.get() on:click=move |_| { m.preview.set(None); m.config.set(Some((t2.clone(), None))); }>"返回修改"</button>
                    <span class="grow"></span>
                    <button class="btn" disabled=move || busy.get() on:click=move |_| m.preview.set(None)>"取消"</button>
                    <button class="btn primary" disabled=move || busy.get() || !can title=if can { "" } else { "安装包没有带组网引擎" } on:click=move |_| {
                        if busy.get_untracked(){return;}
                        busy.set(true);
                        let (text, net) = (text.clone(), net.clone());
                        spawn_local(async move {
                            match api::send::<Value>("PUT", "/api/mesh/config", &json!({ "toml": text })).await {
                                Ok(st) => {
                                    busy.set(false);
                                    m.preview.set(None);
                                    toast(format!("已应用，{net} · 本机地址 {}", Some(s(&st["node"], "virtual_ipv4")).filter(|x| !x.is_empty()).unwrap_or_else(|| "分配中".into())));
                                    m.local.update(|n| *n += 1);
                                    m.app.load_state();
                                }
                                Err(e) => { let _ = busy.try_set(false); toast(format!("应用失败：{e}")); }
                            }
                        });
                    }>{move || if busy.get() { "等待系统授权…" } else { "应用" }}</button>
                </div>
        </Modal>
    }
}

#[component]
fn JoinDialog(m: Mesh, p: Value, text: Option<String>) -> impl IntoView {
    let sm = p["summary"].clone();
    let busy = m.operation;
    let can = p["can_join"].as_bool() == Some(true);
    let net = s(&sm, "network_name");
    let exp = sm["expires_at"].as_i64().unwrap_or(0);
    let peers: Vec<String> = sm["peers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let warns: Vec<String> = p["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let pending = text.is_none();
    let close = Callback::new(move |()| {
        if busy.get_untracked() {
            return;
        }
        m.join.set(None);
        if pending {
            spawn_local(async {
                if let Err(e) = api::send::<Value>("DELETE", "/api/mesh/join", &json!({})).await {
                    toast(format!("未能清除待用邀请：{e}"));
                }
            });
        }
    });
    view! {
        <Modal label="加入团队组网" class="dlg nodes-dialog" close_on_backdrop=false on_close=close>
                <h3>"加入团队组网"</h3>
                <div class="muted small">{format!("{} 邀请这台电脑加入组网。加入后，团队里的机器可以通过虚拟地址访问它，它也能访问团队的机器。", Some(s(&sm, "issued_by")).filter(|x| !x.is_empty()).unwrap_or_else(|| "管理员".into()))}</div>
                <div class="kv"><span class="k">"网络"</span><span class="v mono">{net.clone()}</span></div>
                <div class="nodes-details"><div class="kv"><span class="k">"本机主机名"</span><span class="v mono">{s(&sm, "hostname")}</span></div>
                <div class="kv"><span class="k">"本机地址"</span><span class="v mono">{Some(s(&sm, "ipv4")).filter(|x| !x.is_empty()).unwrap_or_else(|| "自动分配".into())}</span></div>
                <div class="kv"><span class="k">"入网地址"</span><span class="v mono">{peers.join("\n")}</span></div>
                </div><div class="kv"><span class="k">"有效期至"</span><span class="v">{format!("{}（{} 天）", date_of(exp), days_left(exp))}</span></div>
                {(!s(&sm, "note").is_empty()).then(|| view! { <div class="kv"><span class="k">"备注"</span><span class="v">{s(&sm, "note")}</span></div> })}
                {warns.into_iter().map(|w| view! { <div class="notice-box warn">{w}</div> }).collect_view()}
                <div class="muted small">"需要输入这台电脑的登录密码：组网要创建虚拟网卡，并把引擎装成开机自启的系统服务（关掉 Blazar 也保持在线）。"</div>
                <div class="dlg-foot">
                    <button class="btn" disabled=move || busy.get() on:click=move |_| close.run(())>"暂不加入"</button>
                    <button class="btn primary" disabled=move || busy.get() || !can on:click=move |_| {
                        if busy.get_untracked(){return;}
                        busy.set(true);
                        let (text, net) = (text.clone(), net.clone());
                        let ip = s(&sm, "ipv4");
                        spawn_local(async move {
                            let body = match &text { Some(t) => json!({ "invite": t }), None => json!({}) };
                            match api::send::<Value>("POST", "/api/mesh/join", &body).await {
                                Ok(st) => {
                                    busy.set(false);
                                    m.join.set(None);
                                    toast(format!("已加入 {net}，本机地址 {}", Some(s(&st["node"], "virtual_ipv4")).filter(|x| !x.is_empty()).unwrap_or(ip)));
                                    m.local.update(|n| *n += 1);
                                    m.app.load_state();
                                }
                                Err(e) => { let _ = busy.try_set(false); toast(format!("加入失败：{e}")); }
                            }
                        });
                    }>{move || if busy.get() { "等待系统授权…" } else { "加入" }}</button>
                </div>
        </Modal>
    }
}

#[component]
fn Issuer(m: Mesh) -> impl IntoView {
    let v = LocalResource::new(move || {
        m.issuer.track();
        api::get::<Value>("/api/mesh/issuer")
    });
    let settings = RwSignal::new(false);
    let member = RwSignal::new(String::new());
    let host = RwSignal::new(String::new());
    let days = RwSignal::new("180".to_owned());
    let ip = RwSignal::new("auto".to_owned());
    let note = RwSignal::new(String::new());
    let msg = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let generate = move |_| {
        if busy.get_untracked() {
            return;
        }
        let who = member.get_untracked().trim().to_owned();
        if who.is_empty() {
            toast("请填写同事称呼");
            return;
        }
        busy.set(true);
        let body = json!({ "member": who, "hostname": Some(host.get_untracked().trim().to_owned()).filter(|x| !x.is_empty()),
            "days": days.get_untracked().parse::<i64>().unwrap_or(180), "ipv4": ip.get_untracked(), "note": note.get_untracked().trim() });
        spawn_local(async move {
            match api::send::<Value>("POST", "/api/mesh/invites", &body).await {
                Ok(r) => {
                    files_js::download_text(
                        &s(&r, "file_name"),
                        &s(&r, "content"),
                        "application/x-blazar-invite",
                    );
                    msg.try_set(format!("已生成 {}（{}）—— 发给 {who}，TA 用 Blazar 打开即可加入。文件即凭据，请私下发送。", s(&r, "file_name"), Some(s(&r["summary"], "ipv4")).filter(|x| !x.is_empty()).unwrap_or_else(|| "动态地址".into())));
                    member.try_set(String::new());
                    host.try_set(String::new());
                    note.try_set(String::new());
                    m.invites.update(|n| *n += 1);
                }
                Err(e) => toast(format!("生成失败：{e}")),
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <section class="card pad">
            {move || match v.get() {
                None => view! { <LoadingState text="正在读取邀请设置…" class="muted small"/> }.into_any(),
                Some(Err(e)) => view! { <h3>"邀请同事"</h3><InlineError message=format!("无法读取邀请设置：{e}") retry=Callback::new(move |()| m.issuer.update(|n| *n += 1))/> }.into_any(),
                Some(Ok(v)) if v["configured"].as_bool() != Some(true) => view! {
                    <div class="card-title"><h3>"邀请同事"</h3><span class="state-pill" data-act="idle">"待设置"</span><span class="grow"></span>
                        <button class="btn small primary" on:click=move |_| settings.set(true)>"签发设置"</button></div>
                    <p class="muted small">"设置签发机器后，即可生成邀请文件。加入他人的组网无需此设置。"</p><details class="nodes-details"><summary>"如何设置"</summary><p class="muted small">"选择持有网络密钥 network_secret 的机器（通常是枢纽），填写 SSH Host 或 local。Blazar 通过 SSH 调用 easytier-cli credential 签发和吊销邀请。加入他人的组网可使用邀请文件或 EasyTier 配置。"</p></details>
                }.into_any(),
                Some(Ok(v)) => {
                    let cfg = v["config"].clone();
                    let reach = v["reachable"].as_bool() == Some(true);
                    let where_ = format!("{}{}", s(&cfg, "via"), Some(s(&cfg, "container")).filter(|x| !x.is_empty()).map(|c| format!(" · 容器 {c}")).unwrap_or_default());
                    let eps: Vec<String> = v["entry_points"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
                    let head = view! {
                        <div class="card-title"><h3>"邀请同事"</h3>
                            <span class="state-pill" data-act=if reach { "completed" } else { "errored" }>{if reach { "可邀请" } else { "未连接" }}</span>
                            <span class="grow"></span><button class="btn small ghost" on:click=move |_| settings.set(true)>"签发设置"</button></div>
                    };
                    if !reach {
                        return view! { {head}<InlineError message=format!("无法连接签发机器，请检查连接或修改签发设置：{}", s(&v, "error")) retry=Callback::new(move |()|m.issuer.update(|n| *n += 1))/><div class="nodes-details"><div class="mono small">{where_}</div></div> }.into_any();
                    }
                    let no_entry = eps.is_empty();
                    view! {
                        {head}
                        <div class="muted small">"邀请文件就是个人入网凭据，请私下发送。可单独吊销，到期自动失效，不含整个网络的密钥。"</div>
                        <details class="nodes-details"><summary>"签发详情"</summary><div class="kv"><span class="k">"机器"</span><span class="v mono">{where_}</span></div><div class="kv"><span class="k">"网络"</span><span class="v mono">{Some(s(&v, "network_name")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into())}</span></div>
                        <div class="kv"><span class="k">"网段"</span><span class="v mono">{Some(s(&v, "subnet")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into())}</span></div>
                        <div class="kv"><span class="k">"入网地址"</span><span class="v mono">{if no_entry { "未设置 —— 点「签发设置」填写".to_owned() } else { eps.join("\n") }}</span></div>
                        </details>
                        {no_entry.then(|| view! { <div class="notice-box warn">"请先在签发设置中填写入网地址。"</div> })}
                        <fieldset class="nodes-form" disabled=move || busy.get()>
                        <div class="grid3">
                            <label class="field">"同事称呼"<input placeholder="张三" prop:value=move || member.get() on:input=move |e| member.set(event_target_value(&e))/></label>
                            <label class="field">"主机名（英文称呼可留空）"<input class="mono" placeholder="组网内唯一，如 zhangsan-mbp" prop:value=move || host.get() on:input=move |e| host.set(event_target_value(&e))/></label>
                            <label class="field">"有效期"<select prop:value=move ||days.get() on:change=move |e| days.set(event_target_value(&e))>
                                <option value="30" selected=move ||days.get()=="30">"30 天"</option><option value="90" selected=move ||days.get()=="90">"90 天"</option><option value="180" selected=move ||days.get()=="180">"180 天"</option><option value="365" selected=move ||days.get()=="365">"1 年"</option>
                            </select></label>
                        </div>
                        <div class="grid3">
                            <label class="field">"地址"<select prop:value=move ||ip.get() on:change=move |e| ip.set(event_target_value(&e))><option value="auto" selected=move ||ip.get()=="auto">"自动分配固定地址"</option><option value="dhcp" selected=move ||ip.get()=="dhcp">"由网络动态分配"</option></select></label>
                            <label class="field span2">"备注"<input placeholder="可选，如：算法组 · 实习" prop:value=move || note.get() on:input=move |e| note.set(event_target_value(&e))/></label>
                        </div>
                        </fieldset>
                        <div class="row-actions">
                            <button class="btn primary" disabled=move || busy.get() || no_entry on:click=generate>{move || if busy.get() { "生成中…" } else { "生成邀请文件" }}</button>
                            <span class="muted small" role="status" aria-live="polite">{move || msg.get()}</span>
                        </div>
                    }.into_any()
                }
            }}
            {move || (settings.get()).then(|| view! { <IssuerSettings v=v.get().and_then(Result::ok).unwrap_or(Value::Null) on_close=move |saved: bool| { settings.try_set(false); if saved { m.issuer.update(|n| *n += 1); m.invites.update(|n| *n += 1); m.local.update(|n| *n += 1); m.app.load_state(); } }/> })}
        </section>
    }
}

#[component]
fn IssuerSettings(
    v: Value,
    on_close: impl Fn(bool) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let c = v["config"].clone();
    let via = RwSignal::new(s(&c, "via"));
    let ct = RwSignal::new(s(&c, "container"));
    let eps = RwSignal::new(
        c["entry_points"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
    );
    let derived = v["entry_points"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("，")
        })
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| "无".into());
    let configured = v["configured"].as_bool() == Some(true);
    let initial_values =
        StoredValue::new((via.get_untracked(), ct.get_untracked(), eps.get_untracked()));
    let dirty = RwSignal::new(false);
    let confirming_close = RwSignal::new(false);
    Effect::new(move |_| dirty.set((via.get(), ct.get(), eps.get()) != initial_values.get_value()));
    super::agents::guard_unsaved(dirty);
    let close = Callback::new(move |()| {
        if busy.get_untracked() || confirming_close.get_untracked() {
            return;
        }
        if (via.get_untracked(), ct.get_untracked(), eps.get_untracked())
            == initial_values.get_value()
        {
            on_close(false);
            return;
        }
        confirming_close.set(true);
        spawn_local(async move {
            if dialog::ask(
                "放弃修改",
                "签发设置尚未保存。关闭后，本次修改将丢失。",
                vec![Choice::plain("继续编辑"), Choice::danger("放弃修改")],
            )
            .await
                == Some(1)
                && confirming_close.try_get_untracked().is_some()
            {
                on_close(false);
            }
            confirming_close.try_set(false);
        });
    });
    view! {
        <Modal label="签发设置" class="dlg wide nodes-dialog" on_close=close>
                <h3>"签发设置"</h3>
                <div class="notice-box warn">"签发机器必须配置 credential_file，否则重启后已签发的凭据会全部失效。"</div><details class="nodes-details"><summary>"签发原理"</summary><p class="muted small">"选择持有 network_secret 的机器，通常是枢纽。Blazar 通过 SSH 调用 easytier-cli credential。"</p></details>
                <fieldset class="nodes-form" disabled=move || busy.get()><div class="grid2">
                    <label class="field">"签发节点（SSH Host 或 local）"<input class="mono" placeholder="hub-host" prop:value=move || via.get() on:input=move |e| via.set(event_target_value(&e))/></label>
                    <label class="field">"Docker 容器名（可选）"<input class="mono" placeholder="easytier" prop:value=move || ct.get() on:input=move |e| ct.set(event_target_value(&e))/></label>
                </div>
                <label class="field">"入网地址"
                    <textarea class="mono" rows="3" placeholder="tcp://203.0.113.10:11010" prop:value=move || eps.get() on:input=move |e| eps.set(event_target_value(&e))></textarea><span class="muted small">"每行一个，必须公网可达。留空则从签发机器配置推导。"</span></label><details class="nodes-details"><summary>"当前推导地址"</summary><div class="mono small">{derived}</div></details></fieldset>
                <div class="dlg-foot">
                    {configured.then(|| view! { <button class="btn danger" disabled=move || busy.get() on:click=move |_| spawn_local(async move {
                        if dialog::ask("清除签发节点设置", "清除签发节点设置？回到「未配置」，已签发的邀请记录不动。", vec![Choice::plain("取消"), Choice::danger("清除")]).await != Some(1) { return; }
                        if busy.try_get_untracked().unwrap_or(true){return;}
                        busy.set(true);
                        match api::send::<Value>("DELETE", "/api/mesh/issuer", &json!({})).await { Ok(_) => { toast("已清除"); on_close(true); } Err(e) => toast(format!("清除失败：{e}")) }
                        busy.try_set(false);
                    })>"清除设置"</button> })}
                    <span class="grow"></span>
                    <button class="btn" disabled=move || busy.get() on:click=move |_| close.run(())>"取消"</button>
                    <button class="btn primary" disabled=move || busy.get() on:click=move |_| {
                        if busy.get_untracked(){return;}
                        if via.get_untracked().trim().is_empty(){toast("请填写签发节点");return;}
                        busy.set(true);
                        let body = json!({ "via": via.get_untracked().trim(), "container": Some(ct.get_untracked().trim().to_owned()).filter(|x| !x.is_empty()),
                            "entry_points": eps.get_untracked().lines().map(str::trim).filter(|x| !x.is_empty()).collect::<Vec<_>>() });
                        spawn_local(async move {
                            match api::send::<Value>("PUT", "/api/mesh/issuer", &body).await { Ok(_) => { toast("已保存"); on_close(true); } Err(e) => toast(format!("保存失败：{e}")) }
                            busy.try_set(false);
                        });
                    }>{move || if busy.get() { "保存中…" } else { "保存" }}</button>
                </div>
        </Modal>
    }
}

#[component]
fn Invites(m: Mesh) -> impl IntoView {
    let revoking = RwSignal::new(false);
    let v = LocalResource::new(move || {
        m.invites.track();
        api::get::<Value>("/api/mesh/invites")
    });
    view! {
        <section class="card pad">
            {move || match v.get() {
                None => view! { <LoadingState text="正在读取邀请…" class="muted small"/> }.into_any(),
                Some(Err(e)) => view! { <h3>"邀请记录"</h3><InlineError message=format!("无法读取邀请记录：{e}") retry=Callback::new(move |()| m.invites.update(|n|*n+=1))/> }.into_any(),
                Some(Ok(v)) => {
                    let list: Vec<Value> = v["invites"].as_array().cloned().unwrap_or_default();
                    let lost = list.iter().filter(|i| i["state"] == "lost").count();
                    view! {
                        <div class="card-title"><h3>"邀请记录 "<span class="gchip">{list.len()}</span></h3></div>
                        {(lost > 0).then(|| view! { <div class="notice-box bad">{format!("{lost} 份邀请的凭据已丢失，需要重新签发（通常是签发节点重启、且没有配置 credential_file）。")}</div> })}
                        {(v["issuer_checked"].as_bool() != Some(true)).then(|| view! { <div class="muted small">"签发节点连不上，状态未核对"</div> })}
                        {if list.is_empty() {
                            view! { <EmptyState title="还没有邀请" detail="生成邀请文件后，可在这里查看状态或吊销。" class="muted small"/> }.into_any()
                        } else {
                            view! {
                                <div class="nodes-table"><table class="tb">
                                    <thead><tr><th>"同事"</th><th>"连接详情"</th><th>"状态"</th><th>"到期"</th><th></th></tr></thead>
                                    <tbody>
                                        {list.into_iter().map(|i| {
                                            let st = s(&i, "state");
                                            let (cls, text) = match st.as_str() {
                                                "online" => ("ok", "已入网"), "waiting" => ("", "待使用"), "offline" => ("", "离线"),
                                                "lost" => ("bad", "凭据丢失"), "expired" => ("", "已过期"), "revoked" => ("", "已吊销"), _ => ("", "待核对"),
                                            };
                                            let live = matches!(st.as_str(), "online" | "waiting" | "offline" | "lost");
                                            let exp = i["expires_at"].as_i64().unwrap_or(0);
                                            let (id, who) = (s(&i, "id"), s(&i, "member"));
                                            view! {
                                                <tr>
                                                    <td title=s(&i, "note")>{s(&i, "member")}</td>
                                                    <td><details class="nodes-details"><summary>"查看"</summary><div class="mono small">{s(&i, "hostname")}</div><div class="mono small">{Some(s(&i, "ipv4")).filter(|x| !x.is_empty()).unwrap_or_else(|| "动态地址".into())}</div><div class="muted small">{s(&i, "note")}</div><div class="muted small">{format!("原始状态：{st}")}</div></details></td>
                                                    <td><span class=format!("state-pill {cls}") data-act=if st == "online" { "completed" } else if st == "lost" { "errored" } else { "idle" }>{text.to_owned()}</span></td>
                                                    <td class="muted small" title=format!("签发于 {}", date_of(i["issued_at"].as_i64().unwrap_or(0)))>
                                                        {if st == "revoked" { "—".to_owned() } else { format!("{}{}", date_of(exp), if live { format!(" · {} 天", days_left(exp)) } else { String::new() }) }}
                                                    </td>
                                                    <td style="text-align:right">{live.then(|| view! {
                                                        <button class="btn small danger" disabled=move || revoking.get() on:click=move |_| { let (id, who) = (id.clone(), who.clone()); spawn_local(async move {
                                                            if dialog::ask("吊销邀请", &format!("吊销 {who} 的邀请？TA 的电脑会立即断开组网。"), vec![Choice::plain("取消"), Choice::danger("吊销")]).await != Some(1) { return; }
                                                            if revoking.try_get_untracked().unwrap_or(true) { return; }
                                                            revoking.set(true);
                                                            match api::send::<Value>("DELETE", &format!("/api/mesh/invites/{}", api::enc(&id)), &json!({})).await {
                                                                Ok(_) => { toast("已吊销"); m.invites.update(|n| *n += 1); }
                                                                Err(e) => toast(format!("吊销失败：{e}")),
                                                            }
                                                            revoking.try_set(false);
                                                        }); }>"吊销"</button>
                                                    })}</td>
                                                </tr>
                                            }
                                        }).collect_view()}
                                    </tbody>
                                </table></div>
                            }.into_any()
                        }}
                    }.into_any()
                }
            }}
        </section>
    }
}

#[component]
pub fn NodeDetailPage() -> impl IntoView {
    let params = use_params_map();
    let name = move || {
        params
            .read()
            .get("name")
            .map(|n| {
                js_sys::decode_uri_component(&n)
                    .map(String::from)
                    .unwrap_or(n)
            })
            .unwrap_or_default()
    };
    move || {
        let name = name();
        view! { <NodeDetail name/> }
    }
}

#[component]
fn NodeDetail(name: String) -> impl IntoView {
    let app = use_app();
    let n2 = name.clone();
    let node = Memo::new(move |_| {
        app.state.with(|s| {
            s.as_ref()
                .and_then(|s| s.nodes.iter().find(|n| n.name == n2).cloned())
        })
    });
    let hw = RwSignal::new(None::<Result<Value, String>>);
    let agents = RwSignal::new(None::<Result<(Vec<Value>, Value), String>>);
    let left = RwSignal::new(None::<Result<Value, String>>);
    let checked = RwSignal::new(std::collections::HashSet::<String>::new());
    let force = RwSignal::new(false);
    let nm = StoredValue::new(name.clone());
    let probing = RwSignal::new(false);
    let scanning = RwSignal::new(false);
    let listing_left = RwSignal::new(false);
    let sweeping = RwSignal::new(false);
    let probe = move || {
        if probing.get_untracked() {
            return;
        }
        probing.set(true);
        hw.set(None);
        let n = nm.get_value();
        spawn_local(async move {
            let r = api::send::<Value>(
                "POST",
                &format!("/api/nodes/{}/probe", api::enc(&n)),
                &json!({}),
            )
            .await
            .map_err(|e| e.to_string());
            let _ = hw.try_set(Some(r));
            probing.try_set(false);
            app.load_state();
        });
    };
    let scan = move || {
        let Some(n) = nm.try_get_value() else {
            return;
        };
        if scanning.try_get_untracked().unwrap_or(true) {
            return;
        }
        scanning.set(true);
        agents.set(None);
        spawn_local(async move {
            let found = api::get::<Vec<Value>>(&format!("/api/nodes/{}/agents", api::enc(&n)))
                .await
                .map_err(|e| e.to_string());
            let vers = if n == "local" {
                Value::Null
            } else {
                api::get::<Value>(&format!("/api/nodes/{}/cli-versions", api::enc(&n)))
                    .await
                    .unwrap_or(Value::Null)
            };
            let _ = agents.try_set(Some(found.map(|f| (f, vers))));
            scanning.try_set(false);
        });
    };
    let leftovers = move || {
        let Some(n) = nm.try_get_value() else {
            return;
        };
        if listing_left.try_get_untracked().unwrap_or(true) {
            return;
        }
        listing_left.set(true);
        force.set(false);
        left.set(None);
        spawn_local(async move {
            let r = api::get::<Value>(&format!("/api/nodes/{}/leftovers", api::enc(&n)))
                .await
                .map_err(|e| e.to_string());
            if let Ok(v) = &r {
                checked.try_set(
                    v["orphans"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter(|l| l["dirty"].as_u64().unwrap_or(0) == 0)
                                .map(|l| s(l, "leaf"))
                                .collect()
                        })
                        .unwrap_or_default(),
                );
            }
            let _ = left.try_set(Some(r));
            listing_left.try_set(false);
        });
    };
    let kb = |n: f64| {
        if n > 1_048_576.0 {
            format!("{:.1} GB", n / 1_048_576.0)
        } else if n > 1024.0 {
            format!("{:.0} MB", n / 1024.0)
        } else {
            format!("{n} KB")
        }
    };
    let ws_here = move || {
        app.workspaces()
            .into_iter()
            .filter(|w| w.node == nm.get_value())
            .collect::<Vec<_>>()
    };

    view! {
        <div class="page nodes-page">
            <div class="page-head">
                <a href="/nodes" class="crumb">"机器与组网"</a><span class="sep">"/"</span>
                <h1>{if name == "local" { "本机".to_owned() } else { name.clone() }}</h1>
                {move || { let on = node.get().is_some_and(|n| n.status == "online"); view! { <span class="state-pill" data-act=if on { "completed" } else { "idle" }>{if on { "在线" } else { "离线" }}</span> } }}
                <span class="grow"></span>
                <button class="btn primary" on:click=move |_|{app.new_ws_node.set(Some(nm.get_value()));app.new_ws.set(true);}>"新建工作区"</button>
            </div>
            <section class="card pad">
                <h3>"工作区"</h3>
                {move || {
                    let list = ws_here();
                    if list.is_empty() { return view! { <EmptyState title="还没有工作区" detail="在这台机器上创建第一个工作区。"/> }.into_any(); }
                    view! {
                        <div class="nodes-table"><table class="tb"><tbody>
                            {list.into_iter().map(|w| view! {
                                <tr>
                                    <td><span class="dot" data-act=w.activity.clone()></span>" "<a href=format!("/w/{}", w.id)>{w.name.clone()}</a></td>
                                    <td><details class="nodes-details"><summary>"目录"</summary><div class="mono small">{w.path.clone()}</div></details></td>
                                    <td class="muted" style="text-align:right">{activity_label(&w.activity).to_owned()}</td>
                                </tr>
                            }).collect_view()}
                        </tbody></table></div>
                    }.into_any()
                }}
            </section>
            <section class="card pad">
                <div class="card-title"><h3>"机器状态"</h3><span class="grow"></span><button class="btn small" disabled=move || probing.get() on:click=move |_| probe()>{move || if probing.get() { "检查中…" } else { "检查机器" }}</button></div>
                {move || match hw.get() {
                    None if probing.get() => view! { <LoadingState text="正在检查机器…" class="muted small"/> }.into_any(),
                    None => view! { <EmptyState title="尚未检查" detail="检查硬件资源和 AI 服务连接。" class="muted small"/> }.into_any(),
                    Some(Err(e)) => view! { <InlineError message=format!("检查失败：{e}") retry=Callback::new(move |()| probe())/> }.into_any(),
                    Some(Ok(c)) => {
                        let gpus: Vec<String> = c["gpus"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
                        let mut uniq = gpus.clone(); uniq.sort(); uniq.dedup();
                        let eg: Vec<(String, i64)> = c["egress"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0))).collect()).unwrap_or_default();
                        view! {
                            <div class="kpis">
                                <div><span>"CPU"</span><b>{c["cpus"].to_string()}</b></div>
                                <div><span>"内存"</span><b>{format!("{}G", c["mem_gb"])}</b></div>
                                <div><span>"可用磁盘"</span><b>{format!("{}G", c["disk_free_gb"])}</b></div>
                                <div><span>"GPU"</span><b>{gpus.len()}</b></div>
                            </div>
                            <details class="nodes-details"><summary>"硬件与连接详情"</summary><div class="kv"><span class="k">"系统"</span><span class="v mono">{format!("{} {}", s(&c, "os"), s(&c, "arch"))}</span></div><div class="kv"><span class="k">"负载"</span><span class="v mono">{c["load1"].to_string()}</span></div>
                            {(!uniq.is_empty()).then(|| view! { <div class="muted small">{uniq.join(" / ")}</div> })}
                            <div class="row-actions">{eg.into_iter().map(|(k, v)| { let ok = (200..300).contains(&v) || v == 401; view! { <span class=if ok { "gchip ok" } else { "gchip bad" }>{format!("{k} {v}")}</span> } }).collect_view()}</div>
                            </details>
                            {(c["has_ai_egress"].as_bool() != Some(true)).then(|| view! { <div class="warn-tx small">"无法连接 AI 服务，请在「运行时 → 环境变量」中配置代理。"</div> })}
                        }.into_any()
                    }
                }}
            </section>
            <section class="card pad">
                <div class="card-title"><h3>"运行时"</h3><span class="grow"></span><button class="btn small" disabled=move || scanning.get() on:click=move |_| scan()>{move || if scanning.get() { "读取中…" } else { "检查运行时" }}</button></div>
                {move || match agents.get() {
                    None if scanning.get() => view! { <LoadingState text="正在读取运行时…" class="muted small"/> }.into_any(),
                    None => view! { <EmptyState title="尚未检查" detail="读取已安装的运行时及登录状态。" class="muted small"/> }.into_any(),
                    Some(Err(e)) => view! { <InlineError message=format!("无法读取运行时：{e}") retry=Callback::new(move |()| scan())/> }.into_any(),
                    Some(Ok((found, vers))) => {
                        let inst: Vec<Value> = found.into_iter().filter(|a| a["path"].is_string()).collect();
                        if inst.is_empty() {
                            return view! { <EmptyState title="未发现运行时" detail="在这台机器安装 Claude Code 或 Codex 后，重新检查。" class="muted small"/> }.into_any();
                        }
                        let n = nm.get_value();
                        view! {
                            <div class="nodes-table"><table class="tb">
                                <thead><tr><th>"运行时"</th><th>"版本与路径"</th><th>"登录状态"</th><th>"操作"</th></tr></thead>
                                <tbody>
                                    {inst.into_iter().map(|a| {
                                        let id = s(&a, "id");
                                        let v = vers[&id].clone();
                                        let behind = v["behind"].as_bool() == Some(true);
                                        let local_v = s(&v, "local");
                                        let busy = RwSignal::new(false);
                                        let (id2, n2, n3) = (id.clone(), n.clone(), n.clone());
                                        let authed = a["authed"].as_bool();
                                        view! {
                                            <tr>
                                                <td>{s(&a, "label")}</td>
                                                <td><details class="nodes-details"><summary>"查看详情"</summary><div class="kv"><span class="k">"版本"</span><span class="v mono">{Some(s(&a, "version")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into())}</span></div><div class="kv"><span class="k">"路径"</span><span class="v mono">{s(&a, "path")}</span></div><div class="kv"><span class="k">"本机版本"</span><span class="v mono">{if local_v.is_empty() { "—".to_owned() } else { local_v.clone() }}</span></div><div class="muted small">{s(&a, "auth_hint")}</div></details>
                                                    {behind.then(|| view! {
                                                        " "<span class="gchip warn" title=format!("本机是 {local_v}")>"可更新"</span>" "
                                                        <button class="btn small" disabled=move || busy.get() on:click=move |_| {
                                                            if busy.get_untracked() { return; }
                                                            busy.set(true);
                                                            let (id, n) = (id2.clone(), n2.clone());
                                                            spawn_local(async move {
                                                                match api::send::<Value>("POST", &format!("/api/nodes/{}/update/{id}", api::enc(&n)), &json!({})).await {
                                                                    Ok(r) => toast(s(&r, "message")),
                                                                    Err(e) => toast(format!("更新失败：{e}")),
                                                                }
                                                                busy.try_set(false);
                                                                scan();
                                                            });
                                                        }>{move || if busy.get() { "更新中…".to_owned() } else { "更新".to_owned() }}</button>
                                                    })}
                                                </td>
                                                <td>{match authed { Some(true) => view! { <span class="state-pill" data-act="completed">"已登录"</span> }.into_any(), Some(false) => view! { <span class="state-pill" data-act="idle">"未登录"</span> }.into_any(), None => view! { <span class="state-pill" data-act="awaiting_approval">"待核对"</span> }.into_any() }}</td>
                                                <td style="text-align:right">{(id == "codex" && n3 != "local").then(|| view! {
                                                    <button class="btn small ghost" on:click=move |_| crate::pages::runtimes::node_login(n3.clone(), Callback::new(move |()| scan()))>{if authed == Some(true) { "重新登录" } else { "登录" }}</button>
                                                })}</td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table></div>
                        }.into_any()
                    }
                }}
            </section>
            <details class="card pad nodes-maintenance"><summary>"清理残留目录"</summary><p class="muted small">"查找已不属于工作区的目录，确认后删除。"</p>
                <div class="card-title"><h3>"残留目录"</h3><span class="grow"></span>
                    <button class="btn small" disabled=move || listing_left.get() || sweeping.get() on:click=move |_| leftovers()>{move || if listing_left.get() { "查找中…" } else { "查找残留" }}</button></div>
                {move || match left.get() {
                    None if listing_left.get() => view! { <LoadingState text="正在查找残留目录…" class="muted small"/> }.into_any(),
                    None => view! { <EmptyState title="尚未检查" detail="查找磁盘上未被 Blazar 记录的 worktree 和私有目录。" class="muted small"/> }.into_any(),
                    Some(Err(e)) => view! { <InlineError message=format!("查找失败：{e}") retry=Callback::new(move |()| leftovers())/> }.into_any(),
                    Some(Ok(r)) => {
                        let orphans: Vec<Value> = r["orphans"].as_array().cloned().unwrap_or_default();
                        if orphans.is_empty() {
                            return view! { <div class="muted small">{format!("没有残留，已核对 {} 个目录。", r["owned_count"].as_u64().unwrap_or(0))}</div> }.into_any();
                        }
                        view! {
                            <div class="muted small">{format!("{} 个目录，共 {}。默认保留未提交的改动。", orphans.len(), kb(r["orphan_kb"].as_f64().unwrap_or(0.0)))}</div>
                            <div class="nodes-table"><table class="tb">
                                <thead><tr><th></th><th>"名字"</th><th>"分支"</th><th>"大小"</th><th>"闲置"</th><th>"状态"</th></tr></thead>
                                <tbody>
                                    {orphans.into_iter().map(|l| {
                                        let leaf = s(&l, "leaf");
                                        let l2 = leaf.clone();
                                        let dirty = l["dirty"].as_u64().unwrap_or(0);
                                        view! {
                                            <tr>
                                                <td><input type="checkbox" aria-label=format!("选择目录 {leaf}") disabled=move || sweeping.get() prop:checked=move || checked.with(|c| c.contains(&l2)) on:change={ let leaf = leaf.clone(); move |_| checked.update(|c| if !c.remove(&leaf) { c.insert(leaf.clone()); }) }/></td>
                                                <td class="mono" title=s(&l, "worktree")>{leaf.clone()}</td>
                                                <td class="mono">{Some(s(&l, "branch")).filter(|x| !x.is_empty()).unwrap_or_else(|| "—".into())}</td>
                                                <td class="mono">{kb(l["size_kb"].as_f64().unwrap_or(0.0))}</td>
                                                <td class="mono">{format!("{} 天", l["idle_days"].as_i64().unwrap_or(0))}</td>
                                                <td>{if dirty > 0 { view! { <span class="gchip warn">{format!("{dirty} 处未提交")}</span> }.into_any() }
                                                    else if l["broken"].as_bool() == Some(true) { view! { <span class="gchip">"目录已损坏"</span> }.into_any() }
                                                    else if !l["worktree"].is_string() { view! { <span class="gchip">"只剩私有目录"</span> }.into_any() }
                                                    else { view! { <span class="gchip">"干净"</span> }.into_any() }}</td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table></div>
                            <div class="row-actions">
                                <button class="btn small danger" disabled=move || sweeping.get() on:click=move |_| {
                                    if sweeping.get_untracked() { return; }
                                    let leaves: Vec<String> = checked.get_untracked().into_iter().collect();
                                    if leaves.is_empty() { toast("请先选择目录"); return; }
                                    let f = force.get_untracked();
                                    let n = nm.get_value();
                                    spawn_local(async move {
                                        let warning = if f { "所选目录及其中未提交的改动会永久删除，无法撤销。" } else { "所选残留目录会永久删除；有未提交改动的目录将被跳过。" };
                                        if dialog::ask("删除残留目录", warning, vec![Choice::plain("取消"), Choice::danger("删除目录")]).await != Some(1) { return; }
                                        if sweeping.try_get_untracked().unwrap_or(true) { return; }
                                        sweeping.set(true);
                                        match api::send::<Value>("POST", &format!("/api/nodes/{}/sweep", api::enc(&n)), &json!({ "leaves": leaves, "force": f })).await {
                                            Ok(s2) => {
                                                let removed = s2["removed"].as_array().map_or(0, Vec::len);
                                                let skipped: Vec<String> = s2["skipped"].as_array().map(|a| a.iter().map(|x| format!("{}（{}）", x[0].as_str().unwrap_or(""), x[1].as_str().unwrap_or(""))).collect()).unwrap_or_default();
                                                toast(if skipped.is_empty() { format!("清掉 {removed} 份") } else { format!("清掉 {removed} 份；跳过 {} 份：{}", skipped.len(), skipped.join("；")) });
                                                leftovers();
                                            }
                                            Err(e) => toast(format!("清扫失败：{e}")),
                                        }
                                        sweeping.try_set(false);
                                    });
                                }>{move || if sweeping.get() { "删除中…" } else { "删除所选目录" }}</button>
                                <label class="chk"><input type="checkbox" disabled=move || sweeping.get() prop:checked=move || force.get() on:change=move |_| force.update(|f| *f = !*f)/>"同时删除未提交的改动"</label>
                            </div>
                        }.into_any()
                    }
                }}
            </details>
        </div>
    }
}
