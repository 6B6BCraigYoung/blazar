//! 侧栏：页面导航、实时计数和提醒入口。

use leptos::prelude::*;
use leptos_router::components::A;

use crate::alerts;
use crate::app_state::use_app;
use crate::realtime::use_bus;

fn icon(d: &str) -> String {
    format!(
        r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{d}</svg>"#
    )
}

const I_INBOX: &str = r#"<path d="M3 13l2.5-7.5A2 2 0 0 1 7.4 4h9.2a2 2 0 0 1 1.9 1.5L21 13v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M3 13h5l1.5 2.5h5L16 13h5"/>"#;
const I_FOLDER: &str =
    r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#;
const I_CLOCK: &str = r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#;
const I_WARN: &str = r#"<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>"#;
const I_TASKS: &str = r#"<rect x="3" y="4" width="5" height="16" rx="1.5"/><rect x="9.5" y="4" width="5" height="10" rx="1.5"/><rect x="16" y="4" width="5" height="13" rx="1.5"/>"#;
const I_AUTO: &str = r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/><path d="M4.5 4.5 7 7M19.5 4.5 17 7"/>"#;
const I_RT: &str = r#"<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4M7 9l2 2-2 2M12 13h4"/>"#;
const I_AGENT: &str = r#"<rect x="4" y="7" width="16" height="12" rx="2"/><path d="M12 7V4M9 12h.01M15 12h.01M9 16h6"/>"#;
const I_SKILL: &str =
    r#"<path d="M4 5h5v15H4zM9.5 5h5v15h-5z"/><path d="M14 8.2l4.3-1.2 3 11.6-4.3 1.2z"/>"#;
const I_APPS: &str = r#"<rect x="3" y="3" width="7.5" height="7.5" rx="2"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="2"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="2"/><path d="M17.25 14v6.5M14 17.25h6.5"/>"#;
const I_NODES: &str = r#"<circle cx="12" cy="5" r="2.2"/><circle cx="5" cy="18" r="2.2"/><circle cx="19" cy="18" r="2.2"/><path d="M10.9 6.9 6.1 16M13.1 6.9l4.8 9.1M7.2 18h9.6"/>"#;
const I_USAGE: &str = r#"<path d="M3 20V10M9 20V4M15 20v-7M21 20V8"/>"#;
const I_GEAR: &str = r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"/>"#;
const I_SEARCH: &str = r#"<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>"#;
const I_BELL: &str = r#"<path d="M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>"#;

/// 新界面里的页面。
fn nav(
    href: &'static str,
    ic: &'static str,
    label: &'static str,
    count: Signal<String>,
    exact: bool,
) -> impl IntoView {
    view! {
        <A href=href exact=exact attr:class="nav">
            <span class="ic" inner_html=icon(ic)></span>
            <span class="nm">{label}</span>
            <span class="n">{move || count.get()}</span>
        </A>
    }
}

fn n(v: usize) -> String {
    if v == 0 { String::new() } else { v.to_string() }
}

#[component]
pub fn Sidebar() -> impl IntoView {
    let app = use_app();
    let bus = use_bus();
    let ws = move || app.workspaces();
    let running =
        Signal::derive(move || n(ws().iter().filter(|w| w.activity == "running").count()));
    let waiting = Signal::derive(move || {
        n(ws()
            .iter()
            .filter(|w| w.activity == "awaiting_approval")
            .count())
    });
    let all = Signal::derive(move || ws().len().to_string());
    let none = Signal::derive(String::new);
    let projects = move || {
        let mut m: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for w in ws() {
            if let Some(p) = w.project.filter(|p| !p.trim().is_empty()) {
                *m.entry(p).or_default() += 1;
            }
        }
        m.into_iter().collect::<Vec<_>>()
    };
    let nodes = move || {
        app.state
            .with(|s| {
                s.as_ref().map(|s| {
                    (
                        s.nodes.iter().filter(|n| n.status == "online").count(),
                        s.nodes.len(),
                    )
                })
            })
            .unwrap_or((0, 0))
    };
    let status = move || {
        let (on, _) = nodes();
        let w = ws();
        let r = w.iter().filter(|x| x.activity == "running").count();
        let a = w
            .iter()
            .filter(|x| x.activity == "awaiting_approval")
            .count();
        let u = w.iter().filter(|x| app.unseen(x)).count();
        let mut s = format!("{on} 节点在线 · {r} 运行中");
        if a > 0 {
            s.push_str(&format!(" · {a} 等审批"));
        }
        if u > 0 {
            s.push_str(&format!(" · {u} 条新结果"));
        }
        s
    };
    let alerts_on = move || {
        app.alerts_rev.track();
        alerts::notify_on() || alerts::sound_prefs().on
    };

    view! {
        <nav class="side">
            <div class="brand"><i></i>"Blazar"<span class="grow"></span><button class="linkbtn" aria-label="收起侧栏" title="收起侧栏" on:click=move |_| app.side_collapsed.set(true)>"«"</button></div>
            <button class="nav search" on:click=move |_| app.palette.set(true)>
                <span class="ic" inner_html=icon(I_SEARCH)></span><span class="nm muted">"搜索…"</span><span class="n">"⌘K"</span>
            </button>
            <button class="btn primary new-ws" on:click=move |_| app.new_ws.set(true)>"＋ 新建工作区"</button>
            <div class="grp">
                <span>"工作"</span>
                {nav("/inbox", I_INBOX, "收件箱", Signal::derive(move || { let u = app.inbox_unread.get(); if u > 0 { u.to_string() } else { String::new() } }), false)}
                {nav("/", I_FOLDER, "全部工作区", all, true)}
                {nav("/running", I_CLOCK, "运行中", running, true)}
                {nav("/waiting", I_WARN, "等我审批", waiting, true)}
                {nav("/tasks", I_TASKS, "任务", Signal::derive(move || n(app.tasks_open.get())), false)}
                {nav("/autopilots", I_AUTO, "自动化", Signal::derive(move || n(app.autopilots_active.get())), false)}
            </div>
            {move || {
                let p = projects();
                (!p.is_empty()).then(|| view! {
                    <div class="grp">
                        <span>"项目"</span>
                        {p.into_iter().map(|(name, c)| {
                            let href = format!("/project/{}", js_sys::encode_uri_component(&name));
                            let title = name.clone();
                            view! {
                                <A href=href attr:class="nav" attr:title=title>
                                    <span class="ic" inner_html=icon(I_FOLDER)></span><span class="nm">{name}</span><span class="n">{c}</span>
                                </A>
                            }
                        }).collect_view()}
                    </div>
                })
            }}
            <div class="grp">
                <span>"AI"</span>
                {nav("/runtimes", I_RT, "运行时", Signal::derive(move || n(app.runtimes_n.get())), false)}
                {nav("/agents", I_AGENT, "智能体", Signal::derive(move || n(app.agents_n.get())), false)}
                {nav("/skills", I_SKILL, "SKILLs", Signal::derive(move || n(app.skills_n.get())), false)}
            </div>
            <div class="grp">
                <span>"办公"</span>
                {nav("/apps", I_APPS, "接入的软件", none, false)}
            </div>
            <div class="grp">
                <span>"基础设施"</span>
                {nav("/nodes", I_NODES, "机器与组网", Signal::derive(move || { let (a, b) = nodes(); format!("{a}/{b}") }), false)}
            </div>
            <div class="foot">
                {nav("/usage", I_USAGE, "用量", none, false)}
                {nav("/settings", I_GEAR, "设置", none, false)}
                <div class="status">
                    {status}
                    <button class="linkbtn" on:click=move |_| app.alerts_open.set(true)>
                        <span class="ic-inline" inner_html=icon(I_BELL)></span>{move || if alerts_on() { " 提醒已开" } else { " 开启提醒" }}
                    </button>
                </div>
                <span class="conn" data-up=move || bus.connected.get().to_string()>
                    {move || if bus.connected.get() { "已连接" } else { "正在重连 hub…" }}
                </span>
            </div>
        </nav>
    }
}
