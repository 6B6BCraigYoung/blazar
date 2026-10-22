use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use wasm_bindgen::JsCast;

use super::icon;
use super::toast::toast;
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
    let keys = RwSignal::new(0u32);
    let watch_keys = crate::shortcuts::on_change(move || keys.update(|n| *n += 1));
    on_cleanup(move || watch_keys.remove());
    let palette_key = move || {
        keys.track();
        crate::shortcuts::label(&crate::shortcuts::current("palette"))
    };
    let top = move || app.top_workspaces();
    let none = Signal::derive(String::new);
    let projects = move || {
        let mut m: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for w in top() {
            if let Some(p) = w.project.clone().filter(|p| !p.trim().is_empty()) {
                let e = m.entry(p).or_default();
                if app.attention(&w) {
                    *e += 1;
                }
            }
        }
        m.into_iter().collect::<Vec<_>>()
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
            <div class="grp">
                {nav(
                    "/inbox",
                    icon::INBOX,
                    "收件箱",
                    Signal::derive(move || n(usize::try_from(app.inbox_unread.get()).unwrap_or(0))),
                    false,
                )}
                {section(
                    "/",
                    icon::FOLDER,
                    "工作区",
                    Signal::derive(move || n(app.attention_count())),
                    &["/", "/running", "/waiting", "/workspaces"],
                )}
                {section("/bots", icon::BOT, "bot", none, &["/bots"])}
                {section(
                    "/tasks",
                    icon::TASKS,
                    "任务",
                    Signal::derive(move || n(app.tasks_review.get())),
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
                                                <span class="n">{n(c)}</span>
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
                {section("/nodes", icon::NODES, "机器与组网", none, &["/nodes", "/mesh"])}
            </div>
            <div class="foot">
                {nav("/settings", icon::GEAR, "设置", none, false)}
                <StatusDot />
            </div>
        </nav>
    }
}

#[component]
fn StatusDot() -> impl IntoView {
    let app = use_app();
    let bus = use_bus();
    let open = RwSignal::new(false);
    let root = NodeRef::<html::Div>::new();
    let busy = RwSignal::new(false);
    let machines = move || {
        let nodes = app.nodes();
        let usable = nodes.iter().filter(|n| n.is_usable());
        let on = usable.clone().filter(|n| n.status == "online").count();
        let total = usable.count();
        (on, total - on)
    };
    let notify = move || {
        app.alerts_rev.track();
        if !alerts::notify_supported() {
            "不支持系统通知"
        } else if alerts::notify_on() && alerts::notify_granted() {
            "系统通知已开"
        } else {
            "系统通知未开"
        }
    };
    let tone = move || {
        if !bus.connected.get() {
            "failed"
        } else if machines().1 > 0 {
            "attention"
        } else {
            "ok"
        }
    };
    let summary = move || {
        let (on, off) = machines();
        let link = if bus.connected.get() {
            "已连接"
        } else {
            "正在重新连接…"
        };
        let machines = if off > 0 {
            format!("{on} 台机器在线，{off} 台离线")
        } else {
            format!("{on} 台机器在线")
        };
        format!("{link} · {machines} · {}", notify())
    };
    let close_down = window_event_listener(ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
            .zip(root.get_untracked())
            .is_some_and(|(t, r)| r.contains(Some(&t)));
        if !inside {
            open.set(false);
        }
    });
    on_cleanup(move || close_down.remove());
    let toggle_notify = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        leptos::task::spawn_local(async move {
            if alerts::notify_on() {
                alerts::set_notify(false);
            } else {
                let on =
                    alerts::notify_granted() || alerts::request_permission().await == "granted";
                alerts::set_notify(on);
                if !on {
                    toast("系统没有给通知权限，可以在系统设置里打开");
                }
            }
            let _ = app.alerts_rev.try_update(|n| *n += 1);
            let _ = busy.try_set(false);
        });
    };
    view! {
        <div
            class="side-status"
            node_ref=root
            on:keydown=move |e: ev::KeyboardEvent| {
                if e.key() == "Escape" && open.get_untracked() {
                    e.prevent_default();
                    e.stop_propagation();
                    open.set(false);
                }
            }
        >
            <button
                type="button"
                class="side-dot"
                aria-label="状态"
                aria-haspopup="dialog"
                aria-expanded=move || open.get().to_string()
                data-tone=tone
                title=summary
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <i aria-hidden="true"></i>
            </button>
            <Show when=move || open.get()>
                <div class="side-pop" role="dialog" aria-label="状态">
                    <div class="row">
                        <span class="k">"连接"</span>
                        <span class="v" data-up=move || bus.connected.get().to_string()>
                            {move || if bus.connected.get() { "已连接" } else { "正在重新连接…" }}
                        </span>
                    </div>
                    <div class="row">
                        <span class="k">"机器"</span>
                        <a class="v" href="/nodes" on:click=move |_| open.set(false)>
                            {move || {
                                let (on, off) = machines();
                                if off > 0 {
                                    format!("{on} 台在线，{off} 台离线")
                                } else {
                                    format!("{on} 台在线")
                                }
                            }}
                        </a>
                    </div>
                    <div class="row">
                        <span class="k">"提醒"</span>
                        <span class="v">{notify}</span>
                        <Show when=|| alerts::notify_supported()>
                            <button
                                type="button"
                                class="linkbtn"
                                disabled=move || busy.get()
                                on:click=toggle_notify
                            >
                                {move || {
                                    app.alerts_rev.track();
                                    if alerts::notify_on() { "关闭" } else { "开启" }
                                }}
                            </button>
                        </Show>
                    </div>
                    <button
                        type="button"
                        class="linkbtn side-pop-more"
                        on:click=move |_| {
                            open.set(false);
                            app.alerts_open.set(true);
                        }
                    >
                        "提醒方式…"
                    </button>
                </div>
            </Show>
        </div>
    }
}
