use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use super::icon;
use crate::alerts;
use crate::app_state::use_app;
use crate::realtime::use_bus;

fn rail_tip(label: &'static str) -> impl Fn() -> Option<&'static str> + Copy {
    let app = use_app();
    move || app.side_rail.get().then_some(label)
}

fn nav(
    href: &'static str,
    ic: &'static str,
    label: &'static str,
    count: Signal<String>,
    exact: bool,
) -> impl IntoView {
    view! {
        <A href=href exact=exact attr:class="nav" attr:title=rail_tip(label)>
            <span class="ic" inner_html=icon::svg(ic)></span>
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
        <a href=href class="nav" title=rail_tip(label) aria-current=move || on().then_some("page")>
            <span class="ic" inner_html=icon::svg(ic)></span>
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
        let mut parts = vec![format!("{on} 台机器在线"), format!("{r} 运行中")];
        if a > 0 {
            parts.push(format!("{a} 等你批准"));
        }
        if u > 0 {
            parts.push(format!("{u} 条新结果"));
        }
        parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                view! {
                    {(i > 0).then_some(" · ")}
                    <span class="nowrap">{p}</span>
                }
            })
            .collect_view()
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
                <span class="nm">"Blazar"</span>
                <span class="grow"></span>
                <button
                    type="button"
                    class="linkbtn side-fold"
                    aria-label="收起侧栏"
                    title="收起侧栏"
                    on:click=move |_| {
                        if app.current_ws.get_untracked().is_some() {
                            app.side_rail.set(true);
                        } else {
                            app.side_collapsed.set(true);
                        }
                    }
                >
                    "«"
                </button>
            </div>
            <button
                type="button"
                class="nav side-unfold"
                aria-label="展开侧栏"
                title="展开侧栏"
                on:click=move |_| app.side_rail.set(false)
            >
                <span class="ic">"»"</span>
            </button>
            <button
                type="button"
                class="nav search"
                aria-haspopup="dialog"
                aria-expanded=move || app.palette.get().to_string()
                title=rail_tip("搜索")
                on:click=move |_| app.palette.set(true)
            >
                <span class="ic" inner_html=icon::svg(icon::SEARCH)></span>
                <span class="nm muted">"搜索…"</span>
                <span class="n">{palette_key}</span>
            </button>
            <button
                type="button"
                class="btn new-ws"
                aria-haspopup="dialog"
                aria-expanded=move || app.new_ws.get().to_string()
                title=rail_tip("新建工作区")
                on:click=move |_| app.new_ws.set(true)
            >
                <span class="ic" inner_html=icon::svg(icon::PLUS)></span>
                <span class="nm">"新建工作区"</span>
            </button>
            <div class="grp">
                {nav(
                    "/inbox",
                    icon::INBOX,
                    "收件箱",
                    Signal::derive(move || {
                        let u = app.inbox_unread.get();
                        if u > 0 { u.to_string() } else { String::new() }
                    }),
                    false,
                )}
                {section(
                    "/",
                    icon::FOLDER,
                    "工作区",
                    all,
                    &["/", "/running", "/waiting", "/workspaces"],
                )}
                {section(
                    "/bots",
                    icon::BOT,
                    "bot",
                    Signal::derive(move || n(app.bots.with(|b| b.len()))),
                    &["/bots"],
                )}
                {section(
                    "/tasks",
                    icon::TASKS,
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
                            <div class="grp side-projects">
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
                                                <span class="ic" inner_html=icon::svg(icon::FOLDER)></span>
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
                    icon::AGENT,
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
                {nav("/apps", icon::APPS, "接入的软件", none, false)}
                {section(
                    "/nodes",
                    icon::NODES,
                    "机器与组网",
                    Signal::derive(move || {
                        let (a, b) = nodes();
                        format!("{a}/{b}")
                    }),
                    &["/nodes", "/mesh"],
                )}
            </div>
            <div class="foot">
                {nav("/settings", icon::GEAR, "设置", none, false)}
                <div class="side-status">
                    {status}
                    <button
                        type="button"
                        class="linkbtn"
                        aria-haspopup="dialog"
                        aria-expanded=move || app.alerts_open.get().to_string()
                        on:click=move |_| app.alerts_open.set(true)
                    >
                        <span class="ic-inline" inner_html=icon::svg(icon::BELL)></span>
                        <span>{move || if alerts_on() { "提醒已开" } else { "开启提醒" }}</span>
                    </button>
                </div>
                <span class="conn" role="status" data-up=move || bus.connected.get().to_string()>
                    {move || if bus.connected.get() { "已连接" } else { "正在重新连接…" }}
                </span>
            </div>
        </nav>
    }
}
