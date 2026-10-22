use super::*;

#[component]
pub(super) fn PendingMachines(
    nodes: Signal<Vec<blazar_core_types::api::NodeView>>,
) -> impl IntoView {
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
                        <h3>"新发现的组网机器"</h3>
                        <span class="muted small">{format!("{} 台待确认", list.len())}</span>
                        <span class="grow"></span>
                        <button
                            class="btn small"
                            on:click=move |_| set_roles(
                                all.iter().map(|n| (n.clone(), Some("personal"))).collect(),
                            )
                        >
                            "其余都是个人电脑"
                        </button>
                    </div>
                    <p class="muted small">
                        "选出可以运行任务的开发机。个人电脑只显示在线状态，不会出现在选择机器的地方。"
                    </p>
                    <div class="pending-list">
                        {list
                            .into_iter()
                            .map(|n| {
                                let (dev, personal) = (n.name.clone(), n.name.clone());
                                view! {
                                    <div class="pending-row">
                                        <StateOf state=if n.status == "online" { "online" } else { "offline" } />
                                        <span class="pending-name">{n.name.clone()}</span>
                                        <span class="muted small mono">
                                            {n.ipv4.clone().unwrap_or_default()}
                                        </span>
                                        <span class="muted small">{latency(n.latency_ms)}</span>
                                        <span class="grow"></span>
                                        <button
                                            class="btn small primary"
                                            aria-label=format!("把 {} 设为开发机", n.name)
                                            on:click=move |_| set_roles(
                                                vec![(dev.clone(), Some("dev"))],
                                            )
                                        >
                                            "开发机"
                                        </button>
                                        <button
                                            class="btn small"
                                            aria-label=format!("把 {} 设为个人电脑", n.name)
                                            on:click=move |_| set_roles(
                                                vec![(personal.clone(), Some("personal"))],
                                            )
                                        >
                                            "个人电脑"
                                        </button>
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>
                </section>
            }
        })
    }
}

#[component]
pub(super) fn PersonalMachines(
    nodes: Signal<Vec<blazar_core_types::api::NodeView>>,
) -> impl IntoView {
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
                    <summary>
                        <b>"个人电脑"</b>
                        <span class="muted small">
                            {format!("{} 台 · {} 台在线", list.len(), online)}
                        </span>
                    </summary>
                    <p class="muted small">
                        "这些电脑只显示在线状态，不会用来运行任务。"
                    </p>
                    <div class="pending-list">
                        {list
                            .into_iter()
                            .map(|n| {
                                let (dev, reset) = (n.name.clone(), n.name.clone());
                                view! {
                                    <div class="pending-row">
                                        <StateOf state=if n.status == "online" { "online" } else { "offline" } />
                                        <span class="pending-name">{n.name.clone()}</span>
                                        <span class="muted small mono">
                                            {n.ipv4.clone().unwrap_or_default()}
                                        </span>
                                        <span class="grow"></span>
                                        <button
                                            class="btn small"
                                            aria-label=format!("把 {} 改为开发机", n.name)
                                            on:click=move |_| set_roles(
                                                vec![(dev.clone(), Some("dev"))],
                                            )
                                        >
                                            "改为开发机"
                                        </button>
                                        {(n.network.as_deref() == Some("easytier"))
                                            .then(|| {
                                                view! {
                                                    <button
                                                        class="btn small ghost"
                                                        title="放回待确认"
                                                        on:click=move |_| set_roles(vec![(reset.clone(), None)])
                                                    >
                                                        "重新确认"
                                                    </button>
                                                }
                                            })}
                                    </div>
                                }
                            })
                            .collect_view()}
                    </div>
                </details>
            }
        })
    }
}
