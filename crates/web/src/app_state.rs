//! 全局共用的数据：工作区 / 机器快照、侧栏上的各种计数、「看过了没」，以及状态变化时的提醒。

use std::collections::HashMap;

use blazar_core_types::api::{ServerEvent, StateSnapshot, WorkspaceView};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::alerts;
use crate::api;
use crate::realtime::Bus;
use crate::storage;

const SEEN_KEY: &str = "blazar.seen.v1";

#[derive(Clone, Copy)]
pub struct AppData {
    pub state: RwSignal<Option<StateSnapshot>>,
    pub inbox_unread: RwSignal<u64>,
    pub tasks_open: RwSignal<usize>,
    pub autopilots_active: RwSignal<usize>,
    pub agents_n: RwSignal<usize>,
    pub skills_n: RwSignal<usize>,
    pub runtimes_n: RwSignal<usize>,
    /// 工作区 id → 上次看它的时间（RFC3339）
    pub seen: RwSignal<HashMap<String, String>>,
    /// 正在看的工作区
    pub current_ws: RwSignal<Option<String>>,
    pub new_ws: RwSignal<bool>,
    /// 新建工作区时预先选好的机器
    pub new_ws_node: RwSignal<Option<String>>,
    pub palette: RwSignal<bool>,
    pub side_collapsed: RwSignal<bool>,
    pub alerts_open: RwSignal<bool>,
    pub alerts_rev: RwSignal<u32>,
}

pub fn use_app() -> AppData {
    expect_context::<AppData>()
}

impl AppData {
    pub fn workspaces(self) -> Vec<WorkspaceView> {
        self.state
            .with(|s| s.as_ref().map(|s| s.workspaces.clone()).unwrap_or_default())
    }

    /// 跑完 / 出错 / 等审批之后还没打开看过。
    pub fn unseen(self, w: &WorkspaceView) -> bool {
        if !matches!(
            w.activity.as_str(),
            "completed" | "errored" | "awaiting_approval"
        ) {
            return false;
        }
        if self
            .current_ws
            .with(|c| c.as_deref() == Some(w.id.as_str()))
        {
            return false;
        }
        let at = self.seen.with(|s| s.get(&w.id).cloned());
        match (at, &w.last_active_at) {
            (None, _) => true,
            (Some(at), Some(last)) => js_sys::Date::parse(last) > js_sys::Date::parse(&at),
            (Some(_), None) => false,
        }
    }

    /// 列表里显示的状态：看过的「已完成 / 出错」就当空闲。
    pub fn shown_activity(self, w: &WorkspaceView) -> String {
        if self.unseen(w) || !matches!(w.activity.as_str(), "completed" | "errored") {
            w.activity.clone()
        } else {
            "idle".into()
        }
    }

    pub fn mark_seen(self, id: &str) {
        let now = String::from(js_sys::Date::new_0().to_iso_string());
        self.seen.update(|s| {
            s.insert(id.to_owned(), now);
        });
        storage::save(SEEN_KEY, &self.seen.get_untracked());
    }

    pub fn load_state(self) {
        spawn_local(async move {
            if let Ok(s) = api::get::<StateSnapshot>("/api/state").await {
                let _ = self.state.try_set(Some(s));
            }
        });
    }

    fn load_counts(self, which: &'static str) {
        spawn_local(async move {
            match which {
                "inbox" => {
                    if let Ok(v) = api::get::<Value>("/api/inbox/count").await {
                        let _ = self.inbox_unread.try_set(v["unread"].as_u64().unwrap_or(0));
                    }
                }
                "tasks" => {
                    if let Ok(v) = api::get::<Vec<Value>>("/api/tasks").await {
                        let n = v
                            .iter()
                            .filter(|t| {
                                matches!(
                                    t["status"].as_str(),
                                    Some("todo" | "in_progress" | "in_review")
                                )
                            })
                            .count();
                        let _ = self.tasks_open.try_set(n);
                    }
                }
                "autopilots" => {
                    if let Ok(v) = api::get::<Vec<Value>>("/api/autopilots").await {
                        let _ = self
                            .autopilots_active
                            .try_set(v.iter().filter(|a| a["status"] == "active").count());
                    }
                }
                "agents" => {
                    if let Ok(v) = api::get::<Vec<Value>>("/api/agent-profiles").await {
                        let _ = self.agents_n.try_set(v.len());
                    }
                }
                "skills" => {
                    if let Ok(v) = api::get::<Vec<Value>>("/api/skills").await {
                        let _ = self.skills_n.try_set(v.len());
                    }
                }
                _ => {
                    if let Ok(r) = api::get::<api::Runtimes>("/api/runtimes").await {
                        let _ = self.runtimes_n.try_set(
                            r.runtimes
                                .iter()
                                .filter(|x| x.installed && x.authed != Some(false))
                                .count(),
                        );
                    }
                }
            }
        });
    }
}

/// 建全局数据、放进上下文、接上实时事件。只在根组件里调一次。
pub fn provide(bus: Bus) -> AppData {
    let app = AppData {
        state: RwSignal::new(None),
        inbox_unread: RwSignal::new(0),
        tasks_open: RwSignal::new(0),
        autopilots_active: RwSignal::new(0),
        agents_n: RwSignal::new(0),
        skills_n: RwSignal::new(0),
        runtimes_n: RwSignal::new(0),
        seen: RwSignal::new(storage::load(SEEN_KEY).unwrap_or_default()),
        current_ws: RwSignal::new(None),
        new_ws: RwSignal::new(false),
        new_ws_node: RwSignal::new(None),
        palette: RwSignal::new(false),
        side_collapsed: RwSignal::new(
            storage::load_raw("blazar.side.collapsed")
                .map(|s| s == "1")
                .unwrap_or_else(|| {
                    window()
                        .inner_width()
                        .ok()
                        .and_then(|v| v.as_f64())
                        .is_some_and(|w| w < 760.0)
                }),
        ),
        alerts_open: RwSignal::new(false),
        alerts_rev: RwSignal::new(0),
    };
    provide_context(app);
    Effect::new(move |_| {
        storage::save_raw(
            "blazar.side.collapsed",
            if app.side_collapsed.get() { "1" } else { "0" },
        )
    });
    Effect::new(move |_| {
        bus.workspaces.track();
        bus.nodes.track();
        app.load_state();
    });
    for k in [
        "inbox",
        "tasks",
        "autopilots",
        "agents",
        "skills",
        "runtimes",
    ] {
        app.load_counts(k);
    }
    bus.subscribe(move |ev| match ev {
        ServerEvent::InboxChanged => app.load_counts("inbox"),
        ServerEvent::TasksChanged => app.load_counts("tasks"),
        ServerEvent::AutopilotsChanged => app.load_counts("autopilots"),
        ServerEvent::AccountsChanged => app.load_counts("runtimes"),
        _ => {}
    });

    // 状态变化：标签页标题带上「新结果」的数量；跑完 / 出错 / 等审批时响一声、弹通知。
    let last: StoredValue<Option<HashMap<String, String>>> = StoredValue::new(None);
    Effect::new(move |_| {
        let ws = app.workspaces();
        let n = ws.iter().filter(|w| app.unseen(w)).count();
        document().set_title(&if n > 0 {
            format!("({n}) Blazar")
        } else {
            "Blazar".to_owned()
        });
        let now: HashMap<String, String> = ws
            .iter()
            .map(|w| (w.id.clone(), w.activity.clone()))
            .collect();
        let prev = last.get_value();
        last.set_value(Some(now));
        let Some(prev) = prev else { return };
        let fresh: Vec<&WorkspaceView> = ws
            .iter()
            .filter(|w| {
                prev.get(&w.id) != Some(&w.activity)
                    && matches!(
                        w.activity.as_str(),
                        "completed" | "errored" | "awaiting_approval"
                    )
            })
            .collect();
        let visible = document().visibility_state() == web_sys::VisibilityState::Visible;
        let cur = app.current_ws.get_untracked();
        let audible: Vec<&&WorkspaceView> = fresh
            .iter()
            .filter(|w| !(visible && cur.as_deref() == Some(w.id.as_str())))
            .collect();
        if !audible.is_empty() {
            alerts::play(
                if audible.iter().any(|w| w.activity != "completed") {
                    "attention"
                } else {
                    "done"
                },
                false,
            );
        }
        if !alerts::notify_on() || !alerts::notify_granted() {
            return;
        }
        for w in fresh {
            if visible && cur.as_deref() == Some(w.id.as_str()) {
                continue;
            }
            let what = match w.activity.as_str() {
                "completed" => "跑完了",
                "errored" => "出错了",
                _ => "在等你裁决",
            };
            let id = w.id.clone();
            alerts::notify(
                &format!("{} {what}", w.name),
                &format!("{} · {}", w.node, w.path),
                &format!("blazar-{}", w.id),
                w.activity == "awaiting_approval",
                move || {
                    let _ = window().location().set_href(&format!("/w/{id}"));
                },
            );
        }
    });
    app
}
