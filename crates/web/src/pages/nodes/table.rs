use super::*;
use crate::components::menu::menu_keydown;
use crate::components::rows::open_row;
use crate::fmt;
use blazar_core_types::api::NodeView;
use leptos::ev;
use leptos::html;
use wasm_bindgen::JsCast;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct MenuAt {
    x: i32,
    y: i32,
    index: usize,
}

pub(super) fn listed(n: &NodeView) -> bool {
    !n.is_self_mesh()
}

fn rank(n: &NodeView) -> u8 {
    if n.name == "local" {
        0
    } else if n.is_usable() {
        1
    } else if n.is_pending() {
        2
    } else {
        3
    }
}

pub(super) fn sorted(mut list: Vec<NodeView>) -> Vec<NodeView> {
    list.sort_by_key(rank);
    list
}

fn connection(n: &NodeView) -> &'static str {
    if n.name == "local" {
        "本机"
    } else if n.network.as_deref() == Some("ssh") {
        "SSH"
    } else {
        match n.cost.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("p2p") => "直连",
            Some(c) if c.starts_with("relay") => "中继",
            _ => "组网",
        }
    }
}

fn last_seen(n: &NodeView) -> String {
    n.last_seen_at
        .as_deref()
        .map(fmt::ago)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "—".into())
}

#[component]
pub(super) fn MachineTable(nodes: Signal<Vec<NodeView>>) -> impl IntoView {
    let menu = RwSignal::new(None::<MenuAt>);
    view! {
        <div class="machines">
            <table>
                <thead>
                    <tr>
                        <th aria-label="状态"></th>
                        <th>"名称"</th>
                        <th>"地址"</th>
                        <th>"连接"</th>
                        <th class="num">"延迟"</th>
                        <th class="num">"工作区"</th>
                        <th>"最后在线"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        nodes
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, n)| row_view(i, n, menu))
                            .collect_view()
                    }}
                </tbody>
            </table>
        </div>
        {move || {
            menu.get()
                .and_then(|at| {
                    nodes
                        .with(|list| list.get(at.index).cloned())
                        .map(|n| view! { <RowMenu n at menu /> })
                })
        }}
    }
}

fn row_view(i: usize, n: NodeView, menu: RwSignal<Option<MenuAt>>) -> impl IntoView {
    let online = n.status == "online";
    let href = format!("/nodes/{}", api::enc(&n.name));
    let addr = n
        .ipv4
        .clone()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "—".into());
    let latency = (online && n.network.as_deref() == Some("easytier"))
        .then_some(n.latency_ms)
        .flatten()
        .map(|ms| format!("{ms:.0} ms"))
        .unwrap_or_default();
    let seen = if online {
        "在线".to_owned()
    } else {
        last_seen(&n)
    };
    let badge = if n.is_pending() {
        Some(
            view! {
                <span class="status node-badge" data-tone="attention">
                    "待确认"
                </span>
            }
            .into_any(),
        )
    } else if n.is_personal() {
        Some(view! { <span class="gchip">"个人电脑"</span> }.into_any())
    } else {
        None
    };
    view! {
        <tr
            class="node-row"
            data-index=i.to_string()
            data-off=(!online).to_string()
            data-menu=move || menu.with(|m| m.is_some_and(|m| m.index == i)).to_string()
            on:click=move |e: ev::MouseEvent| open_row(&e, "a.node-name")
            on:contextmenu=move |e: ev::MouseEvent| {
                e.prevent_default();
                menu.set(
                    Some(MenuAt {
                        x: e.client_x(),
                        y: e.client_y(),
                        index: i,
                    }),
                );
            }
        >
            <td>
                <span
                    class="dot"
                    role="img"
                    aria-label=if online { "在线" } else { "离线" }
                ></span>
            </td>
            <td>
                <a class="node-name" href=href>
                    {fmt::machine(&n.name)}
                </a>
                {badge}
            </td>
            <td class="node-addr" title=addr.clone()>
                {addr.clone()}
            </td>
            <td class="node-link">{connection(&n)}</td>
            <td class="node-lat num">{latency}</td>
            <td class="node-ws num">{n.workspace_count}</td>
            <td class="node-seen">{seen}</td>
        </tr>
    }
}

#[component]
fn RowMenu(n: NodeView, at: MenuAt, menu: RwSignal<Option<MenuAt>>) -> impl IntoView {
    let app = use_app();
    let root = NodeRef::<html::Div>::new();
    let first = NodeRef::<html::A>::new();
    let close = move || menu.set(None);
    let close_down = window_event_listener(ev::mousedown, move |e| {
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|el| el.closest(".ctx-menu").ok().flatten())
            .is_some();
        if !inside {
            close();
        }
    });
    let close_blur = window_event_listener(ev::blur, move |_| close());
    on_cleanup(move || {
        close_down.remove();
        close_blur.remove();
    });
    request_animation_frame(move || {
        if let Some(a) = first.try_get_untracked().flatten() {
            let _ = a.focus();
        }
    });
    let w = window()
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let h = window()
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i32;
    let (x, y) = (at.x.min(w - 200).max(0), at.y.min(h - 220).max(0));
    let online = n.status == "online";
    let mesh = n.network.as_deref() == Some("easytier");
    let removable = n.network.as_deref() == Some("ssh") && n.workspace_count == 0;
    let name = StoredValue::new(n.name.clone());
    let role_item = move |label: &'static str, role: Option<&'static str>| {
        view! {
            <button
                type="button"
                role="menuitem"
                class="ctx-item"
                on:click=move |_| {
                    close();
                    set_roles(app, vec![(name.get_value(), role)]);
                }
            >
                {label}
            </button>
        }
    };
    let roles = if !mesh {
        ().into_any()
    } else if n.is_pending() {
        view! {
            {role_item("设为开发机", Some("dev"))}
            {role_item("设为个人电脑", Some("personal"))}
        }
        .into_any()
    } else if n.is_personal() {
        view! {
            {role_item("改为开发机", Some("dev"))}
            {role_item("重新确认", None)}
        }
        .into_any()
    } else {
        role_item("改为个人电脑", Some("personal")).into_any()
    };
    let usable = n.is_usable();
    let note = (!online).then(|| format!("最后在线 {}", last_seen(&n)));
    view! {
        <div
            class="ctx-menu machines-menu"
            role="menu"
            aria-label=format!("{} 的操作", fmt::machine(&n.name))
            node_ref=root
            style=format!("left:{x}px;top:{y}px")
            on:keydown=move |e| {
                if let Some(r) = root.get_untracked() {
                    menu_keydown(&e, r.unchecked_ref(), Callback::new(move |_| close()));
                }
            }
        >
            <a
                role="menuitem"
                class="ctx-item"
                href=format!("/nodes/{}", api::enc(&n.name))
                node_ref=first
                on:click=move |_| close()
            >
                "打开"
            </a>
            <button
                type="button"
                role="menuitem"
                class="ctx-item"
                disabled=!online || !usable
                on:click=move |_| {
                    close();
                    app.new_ws_node.set(Some(name.get_value()));
                    app.new_ws.set(true);
                }
            >
                "新建工作区"
            </button>
            {roles}
            {removable
                .then(|| {
                    view! {
                        <div class="menu-sep"></div>
                        <button
                            type="button"
                            role="menuitem"
                            class="ctx-item danger"
                            title="从 Blazar 移除，不修改 SSH 配置"
                            on:click=move |_| {
                                close();
                                remove_ssh(name.get_value());
                            }
                        >
                            "移除"
                        </button>
                    }
                })}
            {note.map(|t| view! { <div class="menu-note">{t}</div> })}
        </div>
    }
}
