use leptos::prelude::*;

use crate::app_state::use_app;

pub struct Tab {
    pub label: &'static str,
    pub href: &'static str,
    pub count: Option<Signal<usize>>,
    pub counts: &'static str,
}

impl Tab {
    #[must_use]
    pub const fn new(label: &'static str, href: &'static str) -> Self {
        Self {
            label,
            href,
            count: None,
            counts: "",
        }
    }

    #[must_use]
    pub fn counted(label: &'static str, href: &'static str, count: Signal<usize>) -> Self {
        Self {
            label,
            href,
            count: Some(count),
            counts: "",
        }
    }

    #[must_use]
    pub const fn counting(mut self, what: &'static str) -> Self {
        self.counts = what;
        self
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
                        <a
                            href=tab.href
                            aria-current=(tab.href == current).then_some("page")
                            aria-label=move || match tab.count.map(|c| c.get()) {
                                Some(n) if n > 0 && !tab.counts.is_empty() => {
                                    format!("{}，{n} 个{}", tab.label, tab.counts)
                                }
                                Some(n) if n > 0 => format!("{} {n}", tab.label),
                                _ => tab.label.to_owned(),
                            }
                            title=move || match tab.count.map(|c| c.get()) {
                                Some(n) if n > 0 && !tab.counts.is_empty() => {
                                    Some(format!("{n} 个{}", tab.counts))
                                }
                                _ => None,
                            }
                        >
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
                Tab::counted("全部", "/tasks", app.tasks_open.into()).counting("未完成的任务"),
                Tab::counted("自动化", "/autopilots", app.autopilots_active.into())
                    .counting("启用中的自动化"),
                Tab::counted("后台任务", "/jobs", app.jobs_running.into())
                    .counting("运行中的后台任务"),
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
