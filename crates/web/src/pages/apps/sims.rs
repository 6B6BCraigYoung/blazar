use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_query_map;
use serde_json::{Value, json};

use super::{AppLogo, AppStatus};
use crate::api;
use crate::app_state::use_app;
use crate::components::dialog::{Choice, ask};
use crate::components::modal::Modal;
use crate::components::status::{EmptyState, InlineError, LoadingState};
use crate::components::toast::toast;
use crate::fmt;
use crate::realtime::use_bus;

pub const SIMS: &[(&str, &str, &str)] = &[
    (
        "isaacsim",
        "Isaac Sim",
        "在工作区所在的机器上启动 Isaac Sim，在这里看高清画面。",
    ),
    (
        "mujoco",
        "MuJoCo",
        "在工作区所在的机器上打开 MuJoCo 查看器，在这里看画面。",
    ),
];

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}

fn running_of(v: &Value, id: &str) -> usize {
    v["launches"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter(|x| x["software"] == id && x["state"] == "running")
                .count()
        })
        .unwrap_or(0)
}

#[component]
pub fn SimCard(id: &'static str, name: &'static str, desc: &'static str) -> impl IntoView {
    let bus = use_bus();
    let data = LocalResource::new(move || {
        bus.nodes.track();
        async move { api::get::<Value>("/api/sims").await }
    });
    view! {
        <article class="card apps-card">
            <div class="apps-card-head">
                <AppLogo id />
                <h2>{name}</h2>
            </div>
            <p class="muted">{desc}</p>
            <div class="apps-card-status" aria-live="polite">
                {move || match data.get() {
                    None => view! { <AppStatus label="读取中…" tone="neutral" /> }.into_any(),
                    Some(Err(error)) => {
                        view! {
                            <span title=error.to_string()>
                                <AppStatus label="无法读取" tone="error" />
                            </span>
                        }
                            .into_any()
                    }
                    Some(Ok(v)) => {
                        let n = running_of(&v, id);
                        if n > 0 {
                            view! { <AppStatus label=format!("{n} 个在运行") tone="ok" /> }
                                .into_any()
                        } else {
                            view! { <AppStatus label="未启动" tone="neutral" /> }.into_any()
                        }
                    }
                }}
            </div>
            <a
                class="btn apps-card-link"
                href=format!("/apps/{id}")
                aria-label=format!("启动 {name}")
            >
                "启动"
                <span aria-hidden="true">"→"</span>
            </a>
        </article>
    }
}

fn state_label(state: &str, code: Option<i64>) -> (String, &'static str) {
    match state {
        "running" => ("运行中".into(), "ok"),
        "exited" => (
            match code {
                Some(0) => "已退出".into(),
                Some(c) => format!("退出（{c}）"),
                None => "已退出".into(),
            },
            "neutral",
        ),
        "lost" => ("进程不在了".into(), "error"),
        _ => ("连不上机器".into(), "error"),
    }
}

#[component]
pub fn SimApp(id: &'static str, name: &'static str) -> impl IntoView {
    let app = use_app();
    let bus = use_bus();
    let rev = RwSignal::new(0u32);
    let launches = LocalResource::new(move || {
        bus.nodes.track();
        rev.track();
        async move { api::get::<Value>("/api/sims").await }
    });
    let ws_sel = RwSignal::new(String::new());
    let scan = RwSignal::new(None::<Result<Value, String>>);
    let scanning = RwSignal::new(false);
    let cand = RwSignal::new(0usize);
    let display = RwSignal::new("auto".to_owned());
    let args = RwSignal::new(String::new());
    let cfg_path = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let viewing = RwSignal::new(None::<(String, String)>);
    let script = RwSignal::new(None::<String>);
    let log = RwSignal::new(None::<(String, Value)>);
    let inst_node = RwSignal::new(String::new());
    let inst_version = RwSignal::new(if id == "isaacsim" {
        "5.1.0.0".to_owned()
    } else {
        String::new()
    });
    let inst_eula = RwSignal::new(false);
    let inst = RwSignal::new(None::<Value>);
    let inst_busy = RwSignal::new(false);
    let poll_install = move || {
        let node = inst_node.get_untracked();
        if node.is_empty() {
            inst.set(None);
            return;
        }
        spawn_local(async move {
            if let Ok(v) =
                api::get::<Value>(&format!("/api/sims/{id}/install/{}", api::enc(&node))).await
                && inst_node.get_untracked() == node
            {
                inst.set(Some(v));
            }
        });
    };
    let start_install = move || {
        let node = inst_node.get_untracked();
        if node.is_empty() || inst_busy.get_untracked() {
            return;
        }
        inst_busy.set(true);
        spawn_local(async move {
            match api::send::<Value>("POST", &format!("/api/sims/{id}/install"), &json!({
                "node": node, "version": inst_version.get_untracked(), "accept_eula": inst_eula.get_untracked(),
            })).await {
                Ok(r) => toast(format!("已在 {} 上开始安装", s(&r, "node"))),
                Err(e) => toast(e.to_string()),
            }
            inst_busy.set(false);
            poll_install();
        });
    };
    let cancel_install = move || {
        let node = inst_node.get_untracked();
        spawn_local(async move {
            if ask(
                "取消安装",
                "会停掉机器上的安装进程，已下载的部分保留。",
                vec![Choice::plain("继续装"), Choice::danger("取消安装")],
            )
            .await
                != Some(1)
            {
                return;
            }
            if let Err(e) = api::send::<Value>(
                "POST",
                &format!("/api/sims/{id}/install/{}/cancel", api::enc(&node)),
                &json!({}),
            )
            .await
            {
                toast(e.to_string());
            }
            poll_install();
        });
    };
    Effect::new(move |_| {
        let running = inst.with(|v| v.as_ref().is_some_and(|v| v["state"] == "running"));
        if !running {
            return;
        }
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(4000).await;
            poll_install();
        });
    });
    let machines = move || {
        let mut list: Vec<String> = app
            .nodes()
            .into_iter()
            .filter(|n| {
                n.transport != "local"
                    && n.status == "online"
                    && n.role.as_deref() != Some("personal")
            })
            .map(|n| n.name)
            .collect();
        list.sort();
        list
    };

    let node_of = move || {
        let id = ws_sel.get();
        app.workspaces()
            .into_iter()
            .find(|w| w.id == id)
            .map(|w| w.node)
            .unwrap_or_default()
    };
    let do_scan = move || {
        let ws = ws_sel.get_untracked();
        if ws.is_empty() || scanning.get_untracked() {
            return;
        }
        scanning.set(true);
        spawn_local(async move {
            let r = api::send::<Value>(
                "POST",
                &format!("/api/sims/{id}/scan"),
                &json!({ "workspace": ws }),
            )
            .await
            .map_err(|e| e.to_string());
            if let Ok(v) = &r {
                cfg_path.set(s(v, "configured"));
                cand.set(0);
                let first_args = v["candidates"][0]["args"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                args.set(first_args);
            }
            scan.set(Some(r));
            scanning.set(false);
        });
    };
    let save_path = move || {
        let node = node_of();
        let path = cfg_path.get_untracked();
        if node.is_empty() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>(
                "PUT",
                &format!("/api/sims/{id}/path/{}", api::enc(&node)),
                &json!({ "path": path }),
            )
            .await
            {
                Ok(_) => do_scan(),
                Err(e) => toast(e.to_string()),
            }
            busy.set(false);
        });
    };
    let launch = move || {
        let ws = ws_sel.get_untracked();
        let Some(Ok(v)) = scan.get_untracked() else {
            return;
        };
        let c = &v["candidates"][cand.get_untracked()];
        let program = s(c, "program");
        if program.is_empty() {
            toast("先选一个能启动的程序");
            return;
        }
        let arg_list: Vec<String> = args
            .get_untracked()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>("POST", &format!("/api/sims/{id}/launch"), &json!({
                "workspace": ws, "program": program, "args": arg_list, "display": display.get_untracked(),
            })).await {
                Ok(r) => {
                    toast(format!("已在 {} 上启动，显示 {}", s(&r, "node"), s(&r, "display")));
                    viewing.set(Some((s(&r, "node"), s(&r, "display"))));
                    rev.update(|n| *n += 1);
                }
                Err(e) => toast(e.to_string()),
            }
            busy.set(false);
        });
    };
    let prepare_virtual = move || {
        let node = node_of();
        if node.is_empty() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/sims/display/{}/virtual", api::enc(&node)),
                &json!({}),
            )
            .await
            {
                Ok(r) => {
                    toast(if r["already"] == true {
                        format!("虚拟显示 {} 已经在跑", s(&r, "display"))
                    } else {
                        format!("虚拟显示 {} 起来了", s(&r, "display"))
                    });
                    do_scan();
                }
                Err(e) => toast(e.to_string()),
            }
            busy.set(false);
        });
    };
    let show_script = move || {
        spawn_local(async move {
            match api::get::<Value>("/api/sims/display/setup-script").await {
                Ok(v) => script.set(Some(s(&v, "script"))),
                Err(e) => toast(e.to_string()),
            }
        });
    };
    let act = move |launch_id: String, what: &'static str| {
        spawn_local(async move {
            let r = match what {
                "stop" => api::send::<Value>(
                    "POST",
                    &format!("/api/sims/launches/{launch_id}/stop"),
                    &json!({}),
                )
                .await
                .map(|_| ()),
                "remove" => {
                    if ask(
                        "移除记录",
                        "只是从列表里移除，不会动机器上的东西。",
                        vec![Choice::plain("取消"), Choice::danger("移除")],
                    )
                    .await
                        != Some(1)
                    {
                        return;
                    }
                    api::send::<Value>(
                        "DELETE",
                        &format!("/api/sims/launches/{launch_id}"),
                        &json!({}),
                    )
                    .await
                    .map(|_| ())
                }
                _ => {
                    match api::get::<Value>(&format!("/api/sims/launches/{launch_id}/log")).await {
                        Ok(v) => {
                            log.set(Some((launch_id.clone(), v)));
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            if let Err(e) = r {
                toast(e.to_string());
            }
            rev.update(|n| *n += 1);
        });
    };

    let query = use_query_map();
    Effect::new(move |_| {
        let want = query.with(|q| q.get("workspace")).unwrap_or_default();
        if want.is_empty() || !ws_sel.get_untracked().is_empty() {
            return;
        }
        if app
            .workspaces()
            .iter()
            .any(|w| w.id == want && w.node != "local")
        {
            ws_sel.set(want);
            do_scan();
        }
    });

    let remote_ws = move || {
        let mut list: Vec<_> = app
            .workspaces()
            .into_iter()
            .filter(|w| w.node != "local")
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    };

    view! {
        <div class="page-head">
            <a href="/apps">"接入的软件"</a>
            <span class="muted" aria-hidden="true">
                "/"
            </span>
            <AppLogo id />
            <h1>{name}</h1>
            <span class="grow"></span>
            <button class="btn" on:click=move |_| rev.update(|n| *n += 1)>
                "刷新"
            </button>
        </div>
        <section class="card settings-card">
            <h3>"启动"</h3>
            <p class="muted">
                {format!(
                    "选一个远端工作区，Blazar 在它所在的机器上找 {name}，启动后在下面看画面。",
                )}
            </p>
            <div class="sim-form">
                <label for="sim-ws">"工作区"</label>
                <select
                    id="sim-ws"
                    class="settings-input"
                    prop:value=move || ws_sel.get()
                    disabled=move || scanning.get() || busy.get()
                    on:change=move |e| {
                        ws_sel.set(event_target_value(&e));
                        scan.set(None);
                        do_scan();
                    }
                >
                    <option value="" selected=move || ws_sel.get().is_empty()>
                        "选择工作区…"
                    </option>
                    {move || {
                        remote_ws()
                            .into_iter()
                            .map(|w| {
                                let id = w.id.clone();
                                let on = move || ws_sel.get() == id;
                                view! {
                                    <option value=w.id.clone() selected=on>
                                        {crate::fmt::ws_label(&w.name, &w.node)}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
                {move || {
                    scan.get()
                        .map(|r| match r {
                            Err(e) => {
                                view! {
                                    <label>"程序"</label>
                                    <div class="sim-hint bad">{e}</div>
                                }
                                    .into_any()
                            }
                            Ok(v) => {
                                let cands: Vec<Value> = v["candidates"]
                                    .as_array()
                                    .cloned()
                                    .unwrap_or_default();
                                let node = s(&v, "node");
                                let standalone: Vec<String> = v["standalone"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(Value::as_str)
                                            .map(str::to_owned)
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let displays: Vec<Value> = v["displays"]["displays"]
                                    .as_array()
                                    .cloned()
                                    .unwrap_or_default();
                                let mine: Vec<Value> = displays
                                    .iter()
                                    .filter(|d| d["mine"] == true)
                                    .cloned()
                                    .collect();
                                let virtual_ready = v["displays"]["virtual_ready"] == true;
                                let missing: Vec<String> = v["displays"]["virtual_missing"]
                                    .as_array()
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(Value::as_str)
                                            .map(str::to_owned)
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let ffmpeg_hint = if v["displays"]["ffmpeg"].is_null() {
                                    "这台机器还没有 ffmpeg，第一次看画面时会自动装进 ~/.blazar。"
                                } else if v["displays"]["nvenc"] == true {
                                    ""
                                } else {
                                    "这台机器的 ffmpeg 没有 NVENC，推流会用 CPU 编码。"
                                };
                                let cands2 = cands.clone();
                                view! {
                                    <label for="sim-prog">"程序"</label>
                                    {if cands.is_empty() {
                                        view! {
                                            <div class="sim-hint bad">
                                                {format!(
                                                    "在 {node} 上没找到 {name}：工作区目录里没有装了它的 .venv，conda 环境里也没有。{}",
                                                    if standalone.is_empty() {
                                                        ""
                                                    } else {
                                                        "可以在下面填独立安装的启动脚本路径。"
                                                    },
                                                )}
                                            </div>
                                        }
                                            .into_any()
                                    } else {
                                        view! {
                                            <select
                                                id="sim-prog"
                                                class="settings-input"
                                                disabled=move || busy.get()
                                                on:change=move |e| {
                                                    let i: usize = event_target_value(&e).parse().unwrap_or(0);
                                                    cand.set(i);
                                                    let a = cands2
                                                        .get(i)
                                                        .and_then(|c| c["args"].as_array())
                                                        .map(|a| {
                                                            a.iter()
                                                                .filter_map(Value::as_str)
                                                                .collect::<Vec<_>>()
                                                                .join(" ")
                                                        })
                                                        .unwrap_or_default();
                                                    args.set(a);
                                                }
                                            >
                                                {cands
                                                    .iter()
                                                    .enumerate()
                                                    .map(|(i, c)| {
                                                        let ver = s(c, "version");
                                                        view! {
                                                            <option
                                                                value=i.to_string()
                                                                selected=move || cand.get() == i
                                                            >
                                                                {format!(
                                                                    "{}{} · {}",
                                                                    s(c, "source"),
                                                                    if ver.is_empty() {
                                                                        String::new()
                                                                    } else {
                                                                        format!(" · {ver}")
                                                                    },
                                                                    s(c, "program"),
                                                                )}
                                                            </option>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </select>
                                        }
                                            .into_any()
                                    }}
                                    {(!standalone.is_empty())
                                        .then(|| {
                                            view! {
                                                <label for="sim-path">"独立安装"</label>
                                                <div class="settings-actions">
                                                    <input
                                                        id="sim-path"
                                                        class="settings-input"
                                                        placeholder=format!(
                                                            "例如 /opt/isaac-sim/{}",
                                                            standalone[0],
                                                        )
                                                        prop:value=move || cfg_path.get()
                                                        on:input=move |e| cfg_path.set(event_target_value(&e))
                                                    />
                                                    <button
                                                        class="btn"
                                                        disabled=move || busy.get()
                                                        on:click=move |_| save_path()
                                                    >
                                                        "保存并重新查找"
                                                    </button>
                                                </div>
                                            }
                                        })}
                                    <label for="sim-args">"参数"</label>
                                    <input
                                        id="sim-args"
                                        class="settings-input"
                                        placeholder="可留空"
                                        prop:value=move || args.get()
                                        on:input=move |e| args.set(event_target_value(&e))
                                    />
                                    <label for="sim-display">"显示"</label>
                                    <select
                                        id="sim-display"
                                        class="settings-input"
                                        disabled=move || busy.get()
                                        on:change=move |e| display.set(event_target_value(&e))
                                    >
                                        <option
                                            value="auto"
                                            selected=move || display.get() == "auto"
                                        >
                                            "自动（有桌面用桌面，没有就起虚拟显示）"
                                        </option>
                                        {mine
                                            .iter()
                                            .map(|d| {
                                                let v = format!(":{}", d["number"]);
                                                let v2 = v.clone();
                                                let v3 = v.clone();
                                                view! {
                                                    <option value=v selected=move || display.get() == v2>
                                                        {format!(
                                                            "{v3}{}{}{}",
                                                            d["size"]
                                                                .as_str()
                                                                .map(|x| format!(" · {x}"))
                                                                .unwrap_or_default(),
                                                            if d["managed"] == true {
                                                                " · 虚拟"
                                                            } else {
                                                                " · 桌面"
                                                            },
                                                            if d["locked"] == true { "（已锁屏）" } else { "" },
                                                        )}
                                                    </option>
                                                }
                                            })
                                            .collect_view()}
                                        <option
                                            value="virtual"
                                            selected=move || display.get() == "virtual"
                                        >
                                            "虚拟显示"
                                        </option>
                                    </select>
                                    <span></span>
                                    <div
                                        class="sim-hint"
                                        class:bad=!virtual_ready && mine.is_empty()
                                    >
                                        {if mine
                                            .iter()
                                            .any(|d| d["locked"] == true && d["managed"] != true)
                                            && !mine.iter().any(|d| d["locked"] != true)
                                        {
                                            format!(
                                                "{node} 的桌面已锁屏，画面里只会看到锁屏：到那台机器上解锁，或者改用虚拟显示。{ffmpeg_hint}",
                                            )
                                        } else if mine.is_empty() && !virtual_ready {
                                            format!(
                                                "{node} 上没有你能用的桌面，虚拟显示也还没准备好（{}）。",
                                                missing.join("、"),
                                            )
                                        } else if mine.is_empty() {
                                            format!(
                                                "{node} 上没有桌面会话，会用虚拟显示。{ffmpeg_hint}",
                                            )
                                        } else {
                                            ffmpeg_hint.to_owned()
                                        }}
                                    </div>
                                    <span></span>
                                    <div class="settings-actions">
                                        <button
                                            class="btn primary"
                                            disabled=move || busy.get() || cands.is_empty()
                                            on:click=move |_| launch()
                                        >
                                            {move || if busy.get() { "处理中…" } else { "启动" }}
                                        </button>
                                        {(mine.is_empty() || !virtual_ready)
                                            .then(|| {
                                                view! {
                                                    {virtual_ready
                                                        .then(|| {
                                                            view! {
                                                                <button
                                                                    class="btn"
                                                                    disabled=move || busy.get()
                                                                    on:click=move |_| prepare_virtual()
                                                                >
                                                                    "先起虚拟显示"
                                                                </button>
                                                            }
                                                        })}
                                                    <button class="btn" on:click=move |_| show_script()>
                                                        "准备脚本（root）"
                                                    </button>
                                                }
                                            })}
                                    </div>
                                }
                                    .into_any()
                            }
                        })
                }}
            </div>
            {move || {
                scanning
                    .get()
                    .then(|| {
                        view! {
                            <LoadingState text="正在机器上查找…" class="muted small" />
                        }
                    })
            }}
        </section>
        <section class="card settings-card">
            <h3>"安装到机器"</h3>
            <p class="muted">
                {if id == "isaacsim" {
                    "Blazar 在那台机器上建一个自己的 Python 3.11 环境，从 NVIDIA 的软件源装 Isaac Sim（约 8 GB 下载，装完 20 GB 左右）。装好后这台机器上的所有工作区都能直接选到。"
                } else {
                    "Blazar 在那台机器上建一个自己的 Python 环境并装 MuJoCo。装好后这台机器上的所有工作区都能直接选到。"
                }}
            </p>
            <div class="sim-form">
                <label for="sim-inst-node">"机器"</label>
                <select
                    id="sim-inst-node"
                    class="settings-input"
                    prop:value=move || inst_node.get()
                    disabled=move || inst_busy.get()
                    on:change=move |e| {
                        inst_node.set(event_target_value(&e));
                        inst.set(None);
                        poll_install();
                    }
                >
                    <option value="" selected=move || inst_node.get().is_empty()>
                        "选择机器…"
                    </option>
                    {move || {
                        machines()
                            .into_iter()
                            .map(|n| {
                                let n2 = n.clone();
                                let n3 = n.clone();
                                view! {
                                    <option value=n selected=move || inst_node.get() == n2>
                                        {n3}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
                <label for="sim-inst-ver">"版本"</label>
                <input
                    id="sim-inst-ver"
                    class="settings-input"
                    placeholder=if id == "isaacsim" { "例如 5.1.0.0" } else { "留空装最新" }
                    prop:value=move || inst_version.get()
                    on:input=move |e| inst_version.set(event_target_value(&e))
                />
                {(id == "isaacsim")
                    .then(|| {
                        view! {
                            <span></span>
                            <label class="chk block">
                                <input
                                    type="checkbox"
                                    prop:checked=move || inst_eula.get()
                                    on:change=move |_| inst_eula.update(|c| *c = !*c)
                                />
                                <span>
                                    "我接受 "
                                    <a
                                        href="https://docs.isaacsim.omniverse.nvidia.com/latest/common/NVIDIA_Omniverse_License_Agreement.html"
                                        target="_blank"
                                        rel="noopener noreferrer"
                                    >
                                        "NVIDIA Omniverse 软件许可协议"
                                    </a>"（Isaac Sim 的 pip 包免费但不是开源许可）"
                                </span>
                            </label>
                        }
                    })}
                <span></span>
                <div class="settings-actions">
                    <button
                        class="btn primary"
                        disabled=move || {
                            inst_busy.get() || inst_node.get().is_empty()
                                || (id == "isaacsim" && !inst_eula.get())
                                || inst
                                    .with(|v| v.as_ref().is_some_and(|v| v["state"] == "running"))
                        }
                        on:click=move |_| start_install()
                    >
                        {move || if inst_busy.get() { "准备中…" } else { "安装" }}
                    </button>
                    {move || {
                        inst.with(|v| v.as_ref().is_some_and(|v| v["state"] == "running"))
                            .then(|| {
                                view! {
                                    <button class="btn" on:click=move |_| cancel_install()>
                                        "取消"
                                    </button>
                                }
                            })
                    }}
                </div>
            </div>
            {move || {
                inst.get()
                    .filter(|v| v["state"] != "none")
                    .map(|v| {
                        let state = s(&v, "state");
                        let (label, tone) = match state.as_str() {
                            "running" => ("安装中".to_owned(), "busy"),
                            "exited" => {
                                match v["exit_code"].as_i64() {
                                    Some(0) => ("已装好".to_owned(), "ok"),
                                    Some(c) => (format!("失败（{c}）"), "error"),
                                    None => ("已结束".to_owned(), "neutral"),
                                }
                            }
                            "lost" => ("进程不在了".to_owned(), "error"),
                            _ => ("连不上机器".to_owned(), "error"),
                        };
                        view! {
                            <div class="sim-inst">
                                <div class="apps-section-head">
                                    <AppStatus label tone />
                                    <span class="muted small">
                                        {format!(
                                            "{} · {}",
                                            s(&v, "version"),
                                            fmt::ago(&s(&v, "started_at")),
                                        )}
                                    </span>
                                </div>
                                <pre class="settings-pre sim-inst-log">
                                    <span class="sim-log-flow">
                                        {
                                            let l = s(&v, "log");
                                            if l.is_empty() {
                                                "（还没有输出）".to_owned()
                                            } else {
                                                l
                                            }
                                        }
                                    </span>
                                </pre>
                            </div>
                        }
                    })
            }}
        </section>
        <section class="card settings-card">
            <h3>"已启动"</h3>
            {move || match launches.get() {
                None => {
                    view! { <LoadingState text="读取中…" class="muted small" /> }.into_any()
                }
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            message=e.to_string()
                            retry=Callback::new(move |_| rev.update(|n| *n += 1))
                        />
                    }
                        .into_any()
                }
                Some(Ok(v)) => {
                    let mut list: Vec<Value> = v["launches"]
                        .as_array()
                        .map(|l| l.iter().filter(|x| x["software"] == id).cloned().collect())
                        .unwrap_or_default();
                    list.reverse();
                    if list.is_empty() {
                        return view! {
                            <EmptyState title="还没有启动过" class="muted small" />
                        }
                            .into_any();
                    }
                    view! {
                        <div class="sim-rows">
                            {list
                                .into_iter()
                                .map(|l| {
                                    let lid = s(&l, "id");
                                    let node = s(&l, "node");
                                    let disp = s(&l, "display");
                                    let state = s(&l, "state");
                                    let (label, tone) = state_label(
                                        &state,
                                        l["exit_code"].as_i64(),
                                    );
                                    let running = state == "running";
                                    let (n1, d1) = (node.clone(), disp.clone());
                                    let (l1, l2, l3) = (lid.clone(), lid.clone(), lid.clone());
                                    view! {
                                        <div class="sim-row">
                                            <div>
                                                <div class="nm">
                                                    {format!("{} · {node}", s(&l, "workspace_name"))}
                                                </div>
                                                <div class="meta">
                                                    {format!(
                                                        "{disp} · {} · {}",
                                                        fmt::ago(&s(&l, "started_at")),
                                                        s(&l, "program"),
                                                    )}
                                                </div>
                                            </div>
                                            <AppStatus label tone />
                                            <button
                                                class="btn small"
                                                disabled=!running
                                                on:click=move |_| {
                                                    viewing.set(Some((n1.clone(), d1.clone())))
                                                }
                                            >
                                                "画面"
                                            </button>
                                            <div class="act">
                                                <button
                                                    class="btn small ghost"
                                                    on:click=move |_| act(l1.clone(), "log")
                                                >
                                                    "日志"
                                                </button>
                                                {if running {
                                                    view! {
                                                        <button
                                                            class="btn small ghost"
                                                            on:click=move |_| act(l2.clone(), "stop")
                                                        >
                                                            "停止"
                                                        </button>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! {
                                                        <button
                                                            class="btn small ghost"
                                                            on:click=move |_| act(l3.clone(), "remove")
                                                        >
                                                            "移除"
                                                        </button>
                                                    }
                                                        .into_any()
                                                }}
                                            </div>
                                        </div>
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                        .into_any()
                }
            }}
        </section>
        {move || {
            viewing
                .get()
                .map(|(node, disp)| {
                    view! {
                        <SimViewer node disp on_close=Callback::new(move |_| viewing.set(None)) />
                    }
                })
        }}
        {move || {
            script
                .get()
                .map(|text| {
                    view! {
                        <Modal
                            label="准备虚拟显示"
                            class="dlg wide"
                            on_close=Callback::new(move |_| script.set(None))
                        >
                            <h3>"准备虚拟显示（需要 root）"</h3>
                            <p class="muted">
                                "把这个脚本放到那台机器上，用 sudo 跑一次：装 Xorg、写一份只给 Blazar 用的无头显示配置、允许普通用户起 X。不会动机器原有的 xorg.conf。"
                            </p>
                            <textarea
                                class="settings-input sim-script"
                                readonly
                                prop:value=text.clone()
                            ></textarea>
                            <div class="dlg-foot">
                                <span class="muted small grow">
                                    "跑完后回到这里点「刷新」。"
                                </span>
                                <button
                                    class="btn"
                                    on:click=move |_| crate::files_js::download_text(
                                        "blazar-headless-display.sh",
                                        &text,
                                        "text/x-shellscript",
                                    )
                                >
                                    "下载脚本"
                                </button>
                                <button class="btn primary" on:click=move |_| script.set(None)>
                                    "关闭"
                                </button>
                            </div>
                        </Modal>
                    }
                })
        }}
        {move || {
            log.get()
                .map(|(lid, v)| {
                    view! {
                        <Modal
                            label="启动日志"
                            class="dlg wide"
                            on_close=Callback::new(move |_| log.set(None))
                        >
                            <h3>"启动日志"</h3>
                            <p class="muted">{format!("记录 {lid} 的最后 200 行。")}</p>
                            <pre class="settings-pre">
                                {
                                    let out = s(&v, "stdout");
                                    let err = s(&v, "stderr");
                                    if out.is_empty() && err.is_empty() {
                                        "（还没有输出）".to_owned()
                                    } else {
                                        format!(
                                            "{out}\n{}",
                                            if err.is_empty() {
                                                String::new()
                                            } else {
                                                format!("--- stderr ---\n{err}")
                                            },
                                        )
                                    }
                                }
                            </pre>
                            <div class="dlg-foot">
                                <span class="grow"></span>
                                <button class="btn primary" on:click=move |_| log.set(None)>
                                    "关闭"
                                </button>
                            </div>
                        </Modal>
                    }
                })
        }}
    }
}

fn kbps_for(height: u32, fps: u32) -> u32 {
    let base = match height {
        0 => 20000,
        h if h >= 1080 => 12000,
        _ => 6000,
    };
    if fps >= 60 { base * 3 / 2 } else { base }
}

#[component]
fn SimViewer(node: String, disp: String, on_close: Callback<()>) -> impl IntoView {
    let canvas = NodeRef::<html::Canvas>::new();
    let live = StoredValue::new_local(None::<Rc<crate::stream::Open>>);
    let state = RwSignal::new("连接中…".to_owned());
    let bad = RwSignal::new(false);
    let stats = RwSignal::new(String::new());
    let height = RwSignal::new(0u32);
    let fps = RwSignal::new(30u32);
    let again = RwSignal::new(0u32);
    let (node_e, disp_e) = (node.clone(), disp.clone());
    Effect::new(move |_| {
        again.track();
        let (h, f) = (height.get(), fps.get());
        let Some(el) = canvas.get() else { return };
        live.set_value(None);
        state.set("连接中…".into());
        bad.set(false);
        stats.set(String::new());
        let path = format!(
            "/api/sims/stream/ws?node={}&display={}&fps={f}&kbps={}&height={h}",
            api::enc(&node_e),
            api::enc(&disp_e),
            kbps_for(h, f)
        );
        let o = crate::stream::open(&el, &path, move |m| {
            if let Some(x) = m.strip_prefix("stats:") {
                let _ = stats.try_set(x.to_owned());
                return;
            }
            let (text, is_bad) = match m.as_str() {
                "open" => ("准备推流…".to_owned(), false),
                "ready" => ("等待画面…".to_owned(), false),
                "playing" => ("播放中".to_owned(), false),
                "closed" => ("已断开".to_owned(), false),
                other => match other.split_once(':') {
                    Some(("error", e)) => (e.to_owned(), true),
                    Some(("ended", e)) => (
                        if e.is_empty() {
                            "推流结束".to_owned()
                        } else {
                            format!("推流结束：{e}")
                        },
                        true,
                    ),
                    _ => (other.to_owned(), false),
                },
            };
            let _ = state.try_set(text);
            let _ = bad.try_set(is_bad);
        });
        live.set_value(Some(Rc::new(o)));
    });
    on_cleanup(move || live.set_value(None));
    let snap = move |_| {
        live.with_value(|l| {
            if let Some(l) = l {
                l.stream.snapshot()
            }
        })
    };
    let full = move |_| {
        live.with_value(|l| {
            if let Some(l) = l {
                l.stream.fullscreen()
            }
        })
    };
    view! {
        <section class="card settings-card">
            <div class="apps-section-head">
                <h3>{format!("画面 · {node} {disp}")}</h3>
                <span class="grow"></span>
                <button class="btn small" on:click=move |_| on_close.run(())>
                    "关闭"
                </button>
            </div>
            <div class="sim-view">
                <canvas node_ref=canvas width="1920" height="1080"></canvas>
            </div>
            <div class="sim-bar">
                <select
                    class="settings-input"
                    aria-label="分辨率"
                    on:change=move |e| height.set(event_target_value(&e).parse().unwrap_or(0))
                >
                    <option value="0" selected=move || height.get() == 0>
                        "原始分辨率"
                    </option>
                    <option value="1080" selected=move || height.get() == 1080>
                        "1080p"
                    </option>
                    <option value="720" selected=move || height.get() == 720>
                        "720p"
                    </option>
                </select>
                <select
                    class="settings-input"
                    aria-label="帧率"
                    on:change=move |e| fps.set(event_target_value(&e).parse().unwrap_or(30))
                >
                    <option value="30" selected=move || fps.get() == 30>
                        "30 fps"
                    </option>
                    <option value="60" selected=move || fps.get() == 60>
                        "60 fps"
                    </option>
                </select>
                <span class="grow"></span>
                <button class="btn small" on:click=move |_| again.update(|n| *n += 1)>
                    "重连"
                </button>
                <button class="btn small" on:click=snap>
                    "截图"
                </button>
                <button class="btn small" on:click=full>
                    "全屏"
                </button>
            </div>
            <div class="sim-state" class:bad=move || bad.get()>
                {move || {
                    let st = stats.get();
                    if st.is_empty() { state.get() } else { format!("{} · {st}", state.get()) }
                }}
            </div>
        </section>
    }
}
