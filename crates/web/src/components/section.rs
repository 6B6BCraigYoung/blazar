use leptos::prelude::*;

use crate::app_state::use_app;

pub struct Tab {
    pub label: &'static str,
    pub href: &'static str,
    pub count: Option<Signal<usize>>,
}

impl Tab {
    #[must_use]
    pub const fn new(label: &'static str, href: &'static str) -> Self {
        Self {
            label,
            href,
            count: None,
        }
    }

    #[must_use]
    pub fn counted(label: &'static str, href: &'static str, count: Signal<usize>) -> Self {
        Self {
            label,
            href,
            count: Some(count),
        }
    }
}

#[component]
pub fn SectionTabs(label: &'static str, tabs: Vec<Tab>, current: &'static str) -> impl IntoView {
    view! {
        <nav class="seg section-tabs" aria-label=label>
            {tabs
                .into_iter()
                .map(|tab| {
                    view! {
                        <a href=tab.href aria-current=(tab.href == current).then_some("page")>
                            {tab.label}
                            {tab
                                .count
                                .map(|count| move || {
                                    let n = count.get();
                                    (n > 0).then(|| view! { <span class="n">{n}</span> })
                                })}
                        </a>
                    }
                })
                .collect_view()}
        </nav>
    }
}

pub fn tasks(current: &'static str) -> impl IntoView {
    let app = use_app();
    view! {
        <SectionTabs
            label="任务"
            current
            tabs=vec![
                Tab::counted("任务", "/tasks", app.tasks_open.into()),
                Tab::counted("自动化", "/autopilots", app.autopilots_active.into()),
            ]
        />
    }
}

pub fn agents(current: &'static str) -> impl IntoView {
    let app = use_app();
    view! {
        <SectionTabs
            label="智能体"
            current
            tabs=vec![
                Tab::counted("角色档案", "/agents", app.agents_n.into()),
                Tab::counted("技能", "/skills", app.skills_n.into()),
                Tab::counted("运行时", "/runtimes", app.runtimes_n.into()),
                Tab::new("用量", "/usage"),
            ]
        />
    }
}
