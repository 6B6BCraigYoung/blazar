use std::collections::HashMap;

mod notification_policy;

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
    pub state_error: RwSignal<Option<String>>,
    pub inbox_unread: RwSignal<u64>,
    pub tasks_open: RwSignal<usize>,
    pub autopilots_active: RwSignal<usize>,
    pub agents_n: RwSignal<usize>,
    pub skills_n: RwSignal<usize>,
    pub runtimes_n: RwSignal<usize>,
    pub seen: RwSignal<HashMap<String, String>>,
    pub current_ws: RwSignal<Option<String>>,
    pub new_ws: RwSignal<bool>,
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
        let _ = self.state_error.try_set(None);
        spawn_local(async move {
            match api::get::<StateSnapshot>("/api/state").await {
                Ok(s) => {
                    let _ = self.state.try_set(Some(s));
                    let _ = self.state_error.try_set(None);
                }
                Err(error) => {
                    let _ = self.state_error.try_set(Some(error.to_string()));
                }
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

async fn notify_changes(app: AppData, fresh: Vec<WorkspaceView>) {
    let Ok(prefs) = api::get::<Value>("/api/settings").await else {
        return;
    };
    let muted: Vec<String> = prefs["inbox"]["muted"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|kind| kind.as_str().map(str::to_owned))
        .collect();
    let approvals = if fresh.iter().any(|w| w.activity == "awaiting_approval") {
        api::get::<Vec<Value>>("/api/approvals")
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let visible = document().visibility_state() == web_sys::VisibilityState::Visible;
    let current_ws = app.current_ws.get_untracked();
    let current = app.state.get_untracked().unwrap_or_default();
    let allowed: Vec<(WorkspaceView, &'static str)> = fresh
        .into_iter()
        .filter(|w| {
            !(visible && current_ws.as_deref() == Some(w.id.as_str()))
                && current.workspaces.iter().any(|now| {
                    now.id == w.id && now.session_id == w.session_id && now.activity == w.activity
                })
        })
        .filter_map(|w| {
            let tools = approvals
                .iter()
                .filter(|a| a["workspace_id"].as_str() == Some(w.id.as_str()))
                .filter_map(|a| a["request"]["tool_name"].as_str());
            notification_policy::workspace_alert_kind(&w.activity, tools, &muted)
                .map(|kind| (w, kind))
        })
        .collect();
    if !allowed.is_empty() {
        alerts::play(
            if allowed.iter().any(|(_, kind)| *kind != "run_done") {
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
    for (w, kind) in allowed {
        let what = match kind {
            "run_done" => "跑完了",
            "run_failed" => "出错了",
            "question" => "在等你回答",
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
}

pub fn provide(bus: Bus) -> AppData {
    let app = AppData {
        state: RwSignal::new(None),
        state_error: RwSignal::new(None),
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
        let fresh: Vec<WorkspaceView> = ws
            .iter()
            .filter(|w| {
                prev.get(&w.id) != Some(&w.activity)
                    && matches!(
                        w.activity.as_str(),
                        "completed" | "errored" | "awaiting_approval"
                    )
            })
            .cloned()
            .collect();
        if !fresh.is_empty() {
            spawn_local(notify_changes(app, fresh));
        }
    });
    app
}
