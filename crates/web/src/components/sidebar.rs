use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

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
const I_TASKS: &str = r#"<rect x="3" y="4" width="5" height="16" rx="1.5"/><rect x="9.5" y="4" width="5" height="10" rx="1.5"/><rect x="16" y="4" width="5" height="13" rx="1.5"/>"#;
const I_AGENT: &str = r#"<rect x="4" y="7" width="16" height="12" rx="2"/><path d="M12 7V4M9 12h.01M15 12h.01M9 16h6"/>"#;
const I_APPS: &str = r#"<rect x="3" y="3" width="7.5" height="7.5" rx="2"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="2"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="2"/><path d="M17.25 14v6.5M14 17.25h6.5"/>"#;
const I_NODES: &str = r#"<circle cx="12" cy="5" r="2.2"/><circle cx="5" cy="18" r="2.2"/><circle cx="19" cy="18" r="2.2"/><path d="M10.9 6.9 6.1 16M13.1 6.9l4.8 9.1M7.2 18h9.6"/>"#;
const I_GEAR: &str = r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1"/>"#;
const I_SEARCH: &str = r#"<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>"#;
const I_BOT: &str = r#"<rect x="5" y="8" width="14" height="11" rx="3"/><path d="M12 8V5M8 5h8M9.5 13h.01M14.5 13h.01M9 16.5h6"/>"#;
const I_BELL: &str = r#"<path d="M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>"#;

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

fn section(
    href: &'static str,
    ic: &'static str,
    label: &'static str,
    count: Signal<String>,
    paths: &'static [&'static str],
) -> impl IntoView {
    let location = use_location();
    let on = move || {
        location.pathname.with(|p| {
            paths
                .iter()
                .any(|x| p == x || (*x != "/" && p.starts_with(&format!("{x}/"))))
        })
    };
    view! {
        <a href=href class="nav" aria-current=move || on().then_some("page")>
            <span class="ic" inner_html=icon(ic)></span>
            <span class="nm">{label}</span>
            <span class="n">{move || count.get()}</span>
        </a>
    }
}

fn n(v: usize) -> String {
    if v == 0 { String::new() } else { v.to_string() }
}

#[component]
pub fn Sidebar() -> impl IntoView {
    let app = use_app();
    let bus = use_bus();
    let keys = RwSignal::new(0u32);
    let watch_keys = crate::shortcuts::on_change(move || keys.update(|n| *n += 1));
    on_cleanup(move || watch_keys.remove());
    let palette_key = move || {
        keys.track();
        crate::shortcuts::label(&crate::shortcuts::current("palette"))
    };
    let ws = move || app.workspaces();
    let top = move || app.top_workspaces();
    let all = Signal::derive(move || top().len().to_string());
    let none = Signal::derive(String::new);
    let projects = move || {
        let mut m: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for w in top() {
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
                        s.nodes
                            .iter()
                            .filter(|n| n.is_usable() && n.status == "online")
                            .count(),
                        s.nodes.iter().filter(|n| n.is_usable()).count(),
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
        let u = top().iter().filter(|x| app.unseen(x)).count();
        let mut s = format!("{on} 台机器在线 · {r} 运行中");
        if a > 0 {
            s.push_str(&format!(" · {a} 等你批准"));
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

    const LOGO: &str = r##"<svg viewBox="100 100 824 824" aria-hidden="true"><rect x="100" y="100" width="824" height="824" rx="185" fill="#0b0f19"/><g fill="none" stroke-linecap="round" transform="translate(0 24)"><path d="M365 616 A208 208 0 1 1 659 616" stroke="#e5e7f0" stroke-width="46"/><path d="M471 478 L449 690 M553 478 L575 690" stroke="#5ee6f0" stroke-width="54"/></g></svg>"##;
    view! {
        <nav class="side" aria-label="主导航">
            <div class="brand">
                <span class="logo" inner_html=LOGO></span>
                "Blazar"
                <span class="grow"></span>
                <button
                    type="button"
                    class="linkbtn"
                    aria-label="收起侧栏"
                    title="收起侧栏"
                    on:click=move |_| app.side_collapsed.set(true)
                >
                    "«"
                </button>
            </div>
            <button
                type="button"
                class="nav search"
                aria-haspopup="dialog"
                aria-expanded=move || app.palette.get().to_string()
                on:click=move |_| app.palette.set(true)
            >
                <span class="ic" inner_html=icon(I_SEARCH)></span>
                <span class="nm muted">"搜索…"</span>
                <span class="n">{palette_key}</span>
            </button>
            <button
                type="button"
                class="btn new-ws"
                aria-haspopup="dialog"
                aria-expanded=move || app.new_ws.get().to_string()
                on:click=move |_| app.new_ws.set(true)
            >
                "新建工作区"
            </button>
            <div class="grp">
                {nav(
                    "/inbox",
                    I_INBOX,
                    "收件箱",
                    Signal::derive(move || {
                        let u = app.inbox_unread.get();
                        if u > 0 { u.to_string() } else { String::new() }
                    }),
                    false,
                )}
                {section(
                    "/",
                    I_FOLDER,
                    "工作区",
                    all,
                    &["/", "/running", "/waiting", "/workspaces"],
                )}
                {section(
                    "/bots",
                    I_BOT,
                    "bot",
                    Signal::derive(move || n(app.bots.with(|b| b.len()))),
                    &["/bots"],
                )}
                {section(
                    "/tasks",
                    I_TASKS,
                    "任务",
                    Signal::derive(move || n(app.tasks_open.get())),
                    &["/tasks", "/autopilots", "/jobs"],
                )}
            </div>
            {move || {
                let p = projects();
                (!p.is_empty())
                    .then(|| {
                        view! {
                            <div class="grp">
                                <span>"项目"</span>
                                {p
                                    .into_iter()
                                    .map(|(name, c)| {
                                        let href = format!(
                                            "/project/{}",
                                            js_sys::encode_uri_component(&name),
                                        );
                                        let title = name.clone();
                                        view! {
                                            <A href=href attr:class="nav" attr:title=title>
                                                <span class="ic" inner_html=icon(I_FOLDER)></span>
                                                <span class="nm">{name}</span>
                                                <span class="n">{c}</span>
                                            </A>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        }
                    })
            }}
            <div class="grp">
                <span>"管理"</span>
                {section(
                    "/agents",
                    I_AGENT,
                    "智能体",
                    none,
                    &[
                        "/agents",
                        "/agent",
                        "/skills",
                        "/library",
                        "/runtimes",
                        "/accounts",
                        "/usage",
                    ],
                )}
                {nav("/apps", I_APPS, "接入的软件", none, false)}
                {section(
                    "/nodes",
                    I_NODES,
                    "机器与组网",
                    Signal::derive(move || {
                        let (a, b) = nodes();
                        format!("{a}/{b}")
                    }),
                    &["/nodes", "/mesh"],
                )}
            </div>
            <div class="foot">
                {nav("/settings", I_GEAR, "设置", none, false)}
                <div class="side-status">
                    {status}
                    <button
                        type="button"
                        class="linkbtn"
                        aria-haspopup="dialog"
                        aria-expanded=move || app.alerts_open.get().to_string()
                        on:click=move |_| app.alerts_open.set(true)
                    >
                        <span class="ic-inline" inner_html=icon(I_BELL)></span>
                        {move || if alerts_on() { " 提醒已开" } else { " 开启提醒" }}
                    </button>
                </div>
                <span class="conn" role="status" data-up=move || bus.connected.get().to_string()>
                    {move || if bus.connected.get() { "已连接" } else { "正在重新连接…" }}
                </span>
            </div>
        </nav>
    }
}
