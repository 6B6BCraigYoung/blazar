use super::*;
use crate::components::prefs::{PrefRow, Sheet};
use blazar_core_types::api::NodeView;
use std::collections::HashSet;

#[component]
pub(super) fn ConfirmSheet(pending: Vec<NodeView>, on_close: Callback<()>) -> impl IntoView {
    let app = use_app();
    let names = StoredValue::new(pending.iter().map(|n| n.name.clone()).collect::<Vec<_>>());
    let picked = RwSignal::new(names.get_value().into_iter().collect::<HashSet<String>>());
    let role = RwSignal::new("dev");
    let count = move || picked.with(HashSet::len);
    let confirm = move || {
        let chosen: Vec<_> = names
            .get_value()
            .into_iter()
            .filter(|n| picked.with_untracked(|p| p.contains(n)))
            .map(|n| (n, Some(role.get_untracked())))
            .collect();
        if chosen.is_empty() {
            return;
        }
        set_roles(app, chosen);
        on_close.run(());
    };
    let choice = move |label: &'static str, value: &'static str| {
        view! {
            <button
                type="button"
                role="radio"
                aria-checked=move || (role.get() == value).to_string()
                data-on=move || (role.get() == value).to_string()
                on:click=move |_| role.set(value)
            >
                {label}
            </button>
        }
    };
    view! {
        <Sheet
            title="确认新机器"
            on_close
            foot=move || {
                view! {
                    <button type="button" class="btn" on:click=move |_| on_close.run(())>
                        "取消"
                    </button>
                    <button
                        type="button"
                        class="btn primary"
                        data-modal-initial-focus=""
                        disabled=move || count() == 0
                        on:click=move |_| confirm()
                    >
                        {move || format!("确认 {} 台", count())}
                    </button>
                }
            }
        >
            <p>
                "组网里发现了这些机器。开发机可以运行任务；个人电脑只显示在线状态，不会出现在选择机器的地方。"
            </p>
            <div class="confirm-list" role="group" aria-label="待确认的机器">
                {pending
                    .into_iter()
                    .map(|n| {
                        let online = n.status == "online";
                        let (id, name) = (n.name.clone(), n.name.clone());
                        view! {
                            <label class="confirm-row">
                                <input
                                    type="checkbox"
                                    prop:checked=move || picked.with(|p| p.contains(&id))
                                    on:change=move |e| {
                                        let on = event_target_checked(&e);
                                        picked
                                            .update(|p| {
                                                if on {
                                                    p.insert(name.clone());
                                                } else {
                                                    p.remove(&name);
                                                }
                                            });
                                    }
                                />
                                <span
                                    class="dot"
                                    data-off=(!online).to_string()
                                    aria-hidden="true"
                                ></span>
                                <span class="confirm-name">{n.name.clone()}</span>
                                <span class="confirm-addr">
                                    {n.ipv4.clone().unwrap_or_default()}
                                </span>
                            </label>
                        }
                    })
                    .collect_view()}
            </div>
            <PrefRow label="设为">
                <div class="seg" role="radiogroup" aria-label="机器类型">
                    {choice("开发机", "dev")}
                    {choice("个人电脑", "personal")}
                </div>
            </PrefRow>
        </Sheet>
    }
}
