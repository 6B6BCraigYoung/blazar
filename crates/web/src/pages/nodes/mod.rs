use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;
use serde_json::{Value, json};

use crate::api;
use crate::app_state::{AppData, use_app};
use crate::components::dialog::{self, Choice};
use crate::components::page_head::PageHead;
use crate::components::prefs::PrefRow;
use crate::components::toast::toast;
use crate::components::{
    icon,
    modal::Modal,
    status::{Checking, EmptyState, InlineError, LoadingState, StateOf, Status, Tone},
};
use crate::files_js;
use wasm_bindgen::closure::Closure;

mod advanced;
mod detail;
mod invites;
mod mesh;
mod roles;
mod ssh;
mod table;

use advanced::AdvancedSheet;
pub use detail::NodeDetailPage;
use invites::{Invites, Issuer};
use mesh::{ConfigEditor, ConfigPreview, JoinDialog, MeshLocal};
use roles::ConfirmSheet;
use ssh::{SshPicker, remove_ssh};
use table::{MachineTable, listed, sorted};

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn dash(x: String) -> String {
    if x.is_empty() { "—".into() } else { x }
}

fn text_row(label: &'static str, value: String, mono: bool) -> impl IntoView {
    view! {
        <PrefRow label=label>
            <span class="pref-text" class:mono=mono>
                {value}
            </span>
        </PrefRow>
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
    view! { {move || m.join.get().map(|(p, text)| view! { <JoinDialog m p text /> })} }
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
    let confirm = RwSignal::new(false);
    let advanced = RwSignal::new(false);
    let nodes = move || {
        app.state
            .with(|s| s.as_ref().map(|s| s.nodes.clone()).unwrap_or_default())
    };
    let shown = Signal::derive(move || {
        let k = q.get().to_lowercase();
        sorted(
            nodes()
                .into_iter()
                .filter(listed)
                .filter(|n| {
                    k.is_empty()
                        || n.name.to_lowercase().contains(&k)
                        || (n.name == "local" && "本机".contains(&k))
                })
                .collect(),
        )
    });
    let pending_count = Memo::new(move |_| nodes().iter().filter(|n| n.is_pending()).count());
    let loading = Memo::new(move |_| app.state.with(|s| s.is_none()));
    let empty = Memo::new(move |_| shown.with(Vec::is_empty));
    let bus = crate::realtime::use_bus();
    let topo = LocalResource::new(move || {
        m.local.track();
        bus.nodes.track();
        api::get::<Value>("/api/mesh/topology")
    });
    let source = Signal::derive(move || {
        topo.get()
            .and_then(Result::ok)
            .map(|t| s(&t, "source"))
            .filter(|x| !x.is_empty())
    });
    let on_file = move |e: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&e);
        if let Some(f) = input.files().and_then(|l| l.get(0)) {
            spawn_local(open_mesh_file(m, f));
        }
        input.set_value("");
    };
    view! {
        <div
            class="page nodes-page"
            on:dragover=|e: leptos::ev::DragEvent| e.prevent_default()
            on:drop=move |e: leptos::ev::DragEvent| {
                let files = e.data_transfer().and_then(|d| d.files());
                let f = files
                    .and_then(|l| {
                        (0..l.length())
                            .filter_map(|i| l.get(i))
                            .find(|f| {
                                let n = f.name().to_lowercase();
                                n.ends_with(".blazar") || n.ends_with(".toml")
                            })
                    });
                if let Some(f) = f {
                    e.prevent_default();
                    e.stop_propagation();
                    spawn_local(open_mesh_file(m, f));
                }
            }
        >
            <PageHead
                title="机器与组网"
                lead="选择一台机器，创建或打开工作区。"
                status=move || {
                    let n = nodes()
                        .iter()
                        .filter(|x| x.is_usable() && x.status == "online")
                        .count();
                    view! { <Status tone=Tone::Other label=format!("{n} 台开发机在线") /> }
                }
            >
                {move || {
                    let n = pending_count.get();
                    (n > 0)
                        .then(|| {
                            view! {
                                <button
                                    type="button"
                                    class="btn"
                                    aria-haspopup="dialog"
                                    on:click=move |_| confirm.set(true)
                                >
                                    {format!("确认 {n} 台…")}
                                </button>
                            }
                        })
                }}
                <button class="btn primary" on:click=move |_| ssh_pick.set(true)>
                    "添加机器"
                </button>
            </PageHead>
            <input type="file" node_ref=invite_in accept=".blazar" hidden on:change=on_file />
            <input
                type="file"
                node_ref=toml_in
                accept=".toml,text/plain"
                hidden
                on:change=on_file
            />
            {move || {
                (nodes().len() >= 8)
                    .then(|| {
                        view! {
                            <div class="machines-toolbar">
                                <input
                                    class="page-filter"
                                    aria-label="筛选机器"
                                    placeholder="搜索机器"
                                    prop:value=move || q.get()
                                    on:input=move |e| q.set(event_target_value(&e))
                                />
                            </div>
                        }
                    })
            }}
            {move || {
                topo.get()
                    .and_then(Result::err)
                    .map(|e| {
                        view! {
                            <InlineError
                                message=format!("无法读取组网机器：{e}")
                                retry=Callback::new(move |()| m.local.update(|n| *n += 1))
                            />
                        }
                    })
            }}
            {move || {
                app.state_error
                    .get()
                    .map(|message| {
                        view! {
                            <InlineError
                                message
                                retry=Callback::new(move |_| app.load_state())
                            />
                        }
                    })
            }}
            {move || {
                if loading.get() {
                    return if app.state_error.with(Option::is_some) {
                        ().into_any()
                    } else {
                        view! { <LoadingState text="正在读取机器…" /> }.into_any()
                    };
                }
                if empty.get() {
                    let filtered = !q.with(String::is_empty);
                    return view! {
                        <EmptyState
                            title=if filtered { "没有匹配的机器" } else { "还没有可用机器" }
                            detail=if filtered {
                                "换个名称搜索，或清空筛选。"
                            } else {
                                "添加 SSH 机器，或发现已加入组网的机器。"
                            }
                        />
                    }
                        .into_any();
                }
                view! { <MachineTable nodes=shown /> }.into_any()
            }}
            {move || {
                topo.get()
                    .and_then(Result::ok)
                    .map(|_| {
                        let src = source.get();
                        view! {
                            <p class="machines-foot muted small">
                                <span>
                                    {src
                                        .clone()
                                        .map_or_else(
                                            || "暂时无法发现组网机器。".to_owned(),
                                            |x| format!("组网机器由{x}发现。"),
                                        )}
                                </span>
                                <button
                                    type="button"
                                    class="linkbtn"
                                    aria-haspopup="dialog"
                                    on:click=move |_| advanced.set(true)
                                >
                                    {if src.is_some() { "高级…" } else { "设置…" }}
                                </button>
                            </p>
                        }
                    })
            }}
            {move || {
                confirm
                    .get()
                    .then(|| {
                        let pending = app
                            .state
                            .with_untracked(|s| {
                                s.as_ref()
                                    .map(|s| {
                                        s.nodes.iter().filter(|n| n.is_pending()).cloned().collect()
                                    })
                                    .unwrap_or_default()
                            });
                        view! {
                            <ConfirmSheet
                                pending
                                on_close=Callback::new(move |()| confirm.set(false))
                            />
                        }
                    })
            }}
            {move || {
                advanced
                    .get()
                    .then(|| {
                        view! {
                            <AdvancedSheet
                                m
                                source
                                on_close=Callback::new(move |()| advanced.set(false))
                            />
                        }
                    })
            }}
            <div class="nodes-section-heading">
                <h2>"团队组网"</h2>
                <span class="muted small">"由 EasyTier 提供"</span>
            </div>
            <MeshLocal m invite_in toml_in />
            <Issuer m />
            <Invites m />
            {move || {
                ssh_pick.get().then(|| view! { <SshPicker on_close=move || ssh_pick.set(false) /> })
            }}
            {move || m.config.get().map(|(text, err)| view! { <ConfigEditor m text err /> })}
            {move || m.preview.get().map(|(p, text)| view! { <ConfigPreview m p text /> })}
        </div>
    }
}

fn set_roles(app: AppData, roles: Vec<(String, Option<&'static str>)>) {
    let roles: serde_json::Map<String, Value> = roles
        .into_iter()
        .map(|(name, role)| (name, json!(role)))
        .collect();
    spawn_local(async move {
        match api::send::<Value>("PUT", "/api/nodes/roles", &json!({ "roles": roles })).await {
            Ok(_) => app.load_state(),
            Err(e) => toast(format!("设置机器类型失败：{e}")),
        }
    });
}
