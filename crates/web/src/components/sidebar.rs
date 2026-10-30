use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_location, use_navigate, use_query_map};
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

/// Bumped whenever a shortcut changes, so tooltips pick up the new combo.
#[derive(Clone, Copy)]
struct KeysRev(RwSignal<u32>);

fn shortcut_of(href: &str) -> Option<&'static str> {
    match href {
        "/" => Some("console"),
        "/workspaces" => Some("workspaces"),
        "/tasks" => Some("tasks"),
        "/settings" => Some("settings"),
        _ => None,
    }
}

fn with_key(label: &'static str, key: Option<&'static str>, rev: Option<KeysRev>) -> String {
    if let Some(KeysRev(rev)) = rev {
        rev.track();
    }
    key.map_or_else(|| label.to_owned(), |k| crate::shortcuts::titled(label, k))
}

/// The tooltip of a sidebar item: the custom one in rail mode (the name plus its shortcut),
/// the native `title` otherwise, and only when the item has a shortcut to teach.
fn tips(
    label: &'static str,
    key: Option<&'static str>,
) -> (
    impl Fn() -> Option<String> + Copy,
    impl Fn() -> Option<String> + Copy,
) {
    let app = use_app();
    let rev = use_context::<KeysRev>();
    let rail = move || app.side_rail.get().then(|| with_key(label, key, rev));
    let title = move || {
        let text = with_key(label, key, rev);
        (!app.side_rail.get() && text != label).then_some(text)
    };
    (rail, title)
}

fn badge(count: Signal<String>, urgent: Signal<bool>) -> impl IntoView {
    view! {
        <span
            class="rail-dot"
            aria-hidden="true"
            data-on=move || count.with(|c| !c.is_empty()).to_string()
            data-urgent=move || urgent.get().to_string()
        ></span>
    }
}

fn nav(
    href: &'static str,
    ic: &'static str,
    label: &'static str,
    count: Signal<String>,
    urgent: Signal<bool>,
    exact: bool,
) -> impl IntoView {
    let (tip, title) = tips(label, shortcut_of(href));
    view! {
        <A
            href=href
            exact=exact
            attr:class="nav"
            attr:data-tip=tip
            attr:title=title
            attr:aria-label=rail_tip(label)
        >
            <span class="ic" inner_html=icon::svg(ic)></span>
            <span class="nm">{label}</span>
            <span class="n">{move || count.get()}</span>
            {badge(count, urgent)}
        </A>
    }
}

fn section(
    href: &'static str,
    ic: &'static str,
    label: &'static str,
    count: Signal<String>,
    urgent: Signal<bool>,
    paths: &'static [&'static str],
) -> impl IntoView {
    section_with_pill(href, ic, label, count, urgent, paths, false)
}

fn section_with_pill(
    href: &'static str,
    ic: &'static str,
    label: &'static str,
    count: Signal<String>,
    urgent: Signal<bool>,
    paths: &'static [&'static str],
    pill: bool,
) -> impl IntoView {
    let location = use_location();
    let on = move || {
        location.pathname.with(|p| {
            paths
                .iter()
                .any(|x| p == x || (*x != "/" && p.starts_with(&format!("{x}/"))))
        })
    };
    let (tip, title) = tips(label, shortcut_of(href));
    view! {
        <a
            href=href
            class="nav"
            data-tip=tip
            title=title
            aria-label=rail_tip(label)
            aria-current=move || on().then_some("page")
        >
            <span class="ic" inner_html=icon::svg(ic)></span>
            <span class="nm">{label}</span>
            <span
                class="n"
                data-pill=move || {
                    (pill && urgent.get() && count.with(|c| !c.is_empty())).to_string()
                }
            >
                {move || count.get()}
            </span>
            {badge(count, urgent)}
        </a>
    }
}

fn n(v: usize) -> String {
    if v == 0 { String::new() } else { v.to_string() }
}

#[component]
pub fn Sidebar() -> impl IntoView {
    let app = use_app();
    let location = use_location();
    let keys = RwSignal::new(0u32);
    let watch_keys = crate::shortcuts::on_change(move || keys.update(|n| *n += 1));
    on_cleanup(move || watch_keys.remove());
    provide_context(KeysRev(keys));
    let in_settings = Memo::new(move |_| location.pathname.with(|p| p == "/settings"));
    Effect::new(move |_| {
        let path = location.pathname.get();
        let query = location.query.get();
        if path != "/settings" {
            app.settings_from
                .set(format!("{path}{}", query.to_query_string()));
        }
    });

    let tip = RwSignal::new(None::<(String, f64, f64)>);
    let timer = StoredValue::new_local(None::<TimeoutHandle>);
    let hide = move || {
        if let Some(h) = timer.get_value() {
            h.clear();
        }
        timer.set_value(None);
        tip.set(None);
    };
    let show = move |e: web_sys::Event| {
        let Some(el) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("[data-tip]").ok().flatten())
        else {
            hide();
            return;
        };
        let text = el.get_attribute("data-tip").unwrap_or_default();
        if tip.with_untracked(|t| t.as_ref().is_some_and(|t| t.0 == text)) {
            return;
        }
        hide();
        let r = el.get_bounding_client_rect();
        let at = (text, r.right() + 8.0, r.top() + r.height() / 2.0);
        timer.set_value(
            set_timeout_with_handle(
                move || tip.set(Some(at.clone())),
                std::time::Duration::from_millis(300),
            )
            .ok(),
        );
    };
    view! {
        <nav
            class="side"
            aria-label="主导航"
            data-mode=move || if in_settings.get() { "settings" } else { "main" }
            on:mouseover=move |e| show(e.into())
            on:mouseleave=move |_| hide()
            on:click=move |_| hide()
        >
            {move || {
                tip.get()
                    .map(|(text, x, y)| {
                        view! {
                            <div
                                class="side-tip"
                                role="tooltip"
                                style=format!("left: {x}px; top: {y}px")
                            >
                                {text}
                            </div>
                        }
                    })
            }}
            <div class="brand">
                <span class="logo" inner_html=super::brand::tile()></span>
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
            {move || {
                if in_settings.get() {
                    view! { <SettingsNav /> }.into_any()
                } else {
                    view! { <MainNav keys /> }.into_any()
                }
            }}
        </nav>
    }
}

#[component]
fn SettingsNav() -> impl IntoView {
    let app = use_app();
    let query = use_query_map();
    let navigate = use_navigate();
    let section = move || query.with(|q| q.get("s").unwrap_or_else(|| "appearance".into()));
    view! {
        <button
            type="button"
            class="side-back"
            title="返回之前的页面（Esc）"
            on:click=move |_| navigate(&app.settings_from.get_untracked(), Default::default())
        >
            <span class="ic" aria-hidden="true">
                "‹"
            </span>
            <span class="nm">"返回"</span>
        </button>
        <div class="grp" aria-label="设置分类">
            {crate::pages::SECTIONS
                .iter()
                .map(|&(id, label, ic)| {
                    view! {
                        <a
                            class="nav"
                            href=format!("/settings?s={id}")
                            data-tip=rail_tip(label)
                            aria-label=rail_tip(label)
                            aria-current=move || (section() == id).then_some("page")
                        >
                            <span class="ic" inner_html=icon::svg(ic)></span>
                            <span class="nm">{label}</span>
                        </a>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
fn MainNav(keys: RwSignal<u32>) -> impl IntoView {
    let app = use_app();
    let palette_key = move || {
        keys.track();
        crate::shortcuts::label(&crate::shortcuts::current("palette"))
    };
    let top = move || app.top_workspaces();
    let none = Signal::derive(String::new);
    let calm = Signal::derive(|| false);
    let waiting = Signal::derive(move || app.attention_count() > 0);
    let needs_you = Signal::derive(move || app.pending_count() > 0);
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
    view! {
        <button
            type="button"
            class="nav search"
            aria-haspopup="dialog"
            aria-expanded=move || app.palette.get().to_string()
            data-tip=tips("搜索", Some("palette")).0
            aria-label=rail_tip("搜索")
            on:click=move |_| app.palette.set(true)
        >
            <span class="ic" inner_html=icon::svg(icon::SEARCH)></span>
            <span class="nm muted">"搜索…"</span>
            <span class="n">{palette_key}</span>
        </button>
        <div class="grp">
            {section_with_pill(
                "/",
                icon::CONSOLE,
                "中控台",
                Signal::derive(move || n(app.pending_count())),
                needs_you,
                &["/", "/chats", "/inbox"],
                true,
            )}
            {section(
                "/workspaces",
                icon::FOLDER,
                "工作区",
                Signal::derive(move || n(app.attention_count())),
                waiting,
                &["/running", "/waiting", "/workspaces", "/w", "/project"],
            )} {section("/bots", icon::BOT, "bot", none, calm, &["/bots"])}
            {section(
                "/tasks",
                icon::TASKS,
                "任务",
                Signal::derive(move || n(app.tasks_review.get())),
                calm,
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
                icon::SPARKLES,
                "智能体",
                none,
                calm,
                &["/agents", "/agent", "/skills", "/library", "/runtimes", "/accounts", "/usage"],
            )}
            {nav("/apps", icon::APPS, "接入的软件", none, calm, false)}
            {section("/nodes", icon::NODES, "机器", none, calm, &["/nodes", "/mesh"])}
        </div>
        <div class="foot">
            {nav("/settings", icon::GEAR, "设置", none, calm, false)} <StatusDot />
        </div>
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
    let notify_ready = move || {
        app.alerts_rev.track();
        !alerts::notify_supported() || (alerts::notify_on() && alerts::notify_granted())
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
        } else if !notify_ready() {
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
            .zip(root.try_get_untracked().flatten())
            .is_some_and(|(t, r)| r.contains(Some(&t)));
        if !inside {
            let _ = open.try_set(false);
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
                        <span class="v">
                            <span
                                class="status"
                                data-tone=move || {
                                    if bus.connected.get() { "other" } else { "failed" }
                                }
                            >
                                {move || {
                                    if bus.connected.get() {
                                        "已连接"
                                    } else {
                                        "正在重新连接…"
                                    }
                                }}
                            </span>
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
