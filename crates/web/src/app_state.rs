use std::collections::HashMap;

mod notification_policy;

use blazar_core_types::api::{ServerEvent, StateSnapshot, WorkspaceView};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::alerts;
use crate::api;
use crate::fmt;
use crate::realtime::Bus;
use crate::storage;

const SEEN_KEY: &str = "blazar.seen.v1";

#[derive(Clone, Copy)]
pub struct AppData {
    pub state: RwSignal<Option<StateSnapshot>>,
    pub state_error: RwSignal<Option<String>>,
    pub inbox_unread: RwSignal<u64>,
    pub tasks_review: RwSignal<usize>,
    pub autopilots_active: RwSignal<usize>,
    pub jobs_running: RwSignal<usize>,
    pub agents_n: RwSignal<usize>,
    pub skills_n: RwSignal<usize>,
    pub runtimes_n: RwSignal<usize>,
    pub bots: RwSignal<Vec<Value>>,
    pub chats: RwSignal<api::Assistant>,
    chats_refresh: StoredValue<bool>,
    pub seen: RwSignal<HashMap<String, String>>,
    pub current_ws: RwSignal<Option<String>>,
    pub selected_ws: RwSignal<Option<String>>,
    pub inspect_ws: RwSignal<Option<String>>,
    pub new_ws: RwSignal<bool>,
    pub new_ws_node: RwSignal<Option<String>>,
    pub new_ws_parent: RwSignal<Option<(String, String)>>,
    pub palette: RwSignal<bool>,
    pub side_collapsed: RwSignal<bool>,
    pub side_rail: RwSignal<bool>,
    pub settings_from: RwSignal<String>,
    pub alerts_open: RwSignal<bool>,
    pub alerts_rev: RwSignal<u32>,
}

pub fn is_top(w: &WorkspaceView, all: &[WorkspaceView]) -> bool {
    let Some(first) = w.parent.as_deref() else {
        return true;
    };
    let mut seen = vec![w.id.as_str()];
    let mut parent = Some(first);
    while let Some(p) = parent {
        match all.iter().find(|x| x.id == p) {
            None => return p == first,
            Some(x) if seen.contains(&x.id.as_str()) => return true,
            Some(x) => {
                seen.push(&x.id);
                parent = x.parent.as_deref();
            }
        }
    }
    false
}

pub fn tree_order(all: &[WorkspaceView]) -> Vec<(usize, WorkspaceView)> {
    let mut out: Vec<(usize, WorkspaceView)> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut stack: Vec<(usize, &WorkspaceView)> = all
        .iter()
        .filter(|w| is_top(w, all))
        .rev()
        .map(|w| (0, w))
        .collect();
    while let Some((depth, w)) = stack.pop() {
        if !seen.insert(w.id.as_str()) {
            continue;
        }
        out.push((depth, w.clone()));
        stack.extend(
            all.iter()
                .filter(|c| c.parent.as_deref() == Some(w.id.as_str()))
                .rev()
                .map(|c| (depth + 1, c)),
        );
    }
    for w in all {
        if !seen.contains(w.id.as_str()) {
            out.push((0, w.clone()));
        }
    }
    out
}

pub fn indented_label(depth: usize, w: &WorkspaceView) -> String {
    let label = crate::fmt::ws_label(&w.name, &w.node);
    if depth == 0 {
        label
    } else {
        format!("{}↳ {label}", "\u{3000}".repeat(depth - 1))
    }
}

pub fn use_app() -> AppData {
    expect_context::<AppData>()
}

impl AppData {
    pub fn workspaces(self) -> Vec<WorkspaceView> {
        self.state
            .with(|s| s.as_ref().map(|s| s.workspaces.clone()).unwrap_or_default())
    }

    pub fn top_workspaces(self) -> Vec<WorkspaceView> {
        self.state.with(|s| {
            s.as_ref()
                .map(|s| {
                    s.workspaces
                        .iter()
                        .filter(|w| is_top(w, &s.workspaces))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    pub fn in_tree_order(self) -> Vec<(usize, WorkspaceView)> {
        self.state.with(|s| {
            s.as_ref()
                .map(|s| tree_order(&s.workspaces))
                .unwrap_or_default()
        })
    }

    pub fn needs_you(self, w: &WorkspaceView) -> bool {
        w.activity == "awaiting_approval"
            || self
                .children_of(&w.id)
                .iter()
                .any(|c| c.activity == "awaiting_approval")
    }

    pub fn attention(self, w: &WorkspaceView) -> bool {
        self.needs_you(w) || self.shown_activity(w) == "errored"
    }

    pub fn attention_count(self) -> usize {
        let top = self.top_workspaces();
        top.iter().filter(|w| self.attention(w)).count()
    }

    pub fn children_of(self, id: &str) -> Vec<WorkspaceView> {
        self.state.with(|s| {
            s.as_ref()
                .map(|s| {
                    s.workspaces
                        .iter()
                        .filter(|w| w.parent.as_deref() == Some(id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    pub fn workspace(self, id: &str) -> Option<WorkspaceView> {
        self.state.with(|s| {
            s.as_ref().and_then(|s| {
                s.workspaces
                    .iter()
                    .chain(s.assistant.as_ref())
                    .find(|w| w.id == id)
                    .cloned()
            })
        })
    }

    pub fn assistant(self) -> Option<WorkspaceView> {
        self.state
            .with(|s| s.as_ref().and_then(|s| s.assistant.clone()))
    }

    pub fn is_assistant(self, id: &str) -> bool {
        self.state.with(|s| {
            s.as_ref()
                .and_then(|s| s.assistant.as_ref())
                .is_some_and(|w| w.id == id)
        })
    }

    pub fn chats_waiting(self) -> usize {
        self.chats.with(|a| {
            a.threads
                .iter()
                .filter(|t| t.state() == "awaiting_approval")
                .count()
        })
    }

    pub fn load_chats(self) {
        spawn_local(async move {
            if let Ok(a) = api::get::<api::Assistant>("/api/assistant").await {
                let _ = self.chats.try_set(a);
            }
        });
    }

    fn load_chats_soon(self) {
        if self.chats_refresh.get_value() {
            return;
        }
        self.chats_refresh.set_value(true);
        gloo_timers::callback::Timeout::new(400, move || {
            let _ = self.chats_refresh.try_set_value(false);
            self.load_chats();
        })
        .forget();
    }

    pub fn node_unreachable(self, node: &str) -> bool {
        node != "local"
            && self.state.with(|s| {
                s.as_ref()
                    .and_then(|s| s.nodes.iter().find(|n| n.name == node))
                    .is_some_and(|n| n.status != "online")
            })
    }

    pub fn nodes(self) -> Vec<blazar_core_types::api::NodeView> {
        self.state
            .with(|s| s.as_ref().map(|s| s.nodes.clone()).unwrap_or_default())
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
        if w.activity == "completed" && !self.unseen(w) {
            "idle".into()
        } else {
            w.activity.clone()
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

    pub fn load_bots(self) {
        spawn_local(async move {
            if let Ok(v) = api::get::<Value>("/api/bots").await {
                let list = v["bots"].as_array().cloned().unwrap_or_default();
                let _ = self.bots.try_set(list);
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
                                t["parent_id"].as_str().is_none_or(str::is_empty)
                                    && t["status"].as_str() == Some("in_review")
                            })
                            .count();
                        let _ = self.tasks_review.try_set(n);
                    }
                }
                "autopilots" => {
                    if let Ok(v) = api::get::<Vec<Value>>("/api/autopilots").await {
                        let _ = self
                            .autopilots_active
                            .try_set(v.iter().filter(|a| a["status"] == "active").count());
                    }
                }
                "jobs" => {
                    if let Ok(v) = api::get::<Value>("/api/jobs?status=running").await {
                        let _ = self
                            .jobs_running
                            .try_set(v["jobs"].as_array().map_or(0, Vec::len));
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
    if !allowed.is_empty() && alerts::notify_undecided() && alerts::notify_supported() {
        let granted = alerts::request_permission().await == "granted";
        alerts::set_notify(granted);
        let _ = app.alerts_rev.try_update(|n| *n += 1);
    }
    if !alerts::notify_on() || !alerts::notify_granted() {
        return;
    }
    for (w, kind) in allowed {
        let what = match kind {
            "run_done" => "跑完了",
            "run_failed" => "出错了",
            "question" => "在等你回答",
            _ => "在等你批准",
        };
        let href = if app.is_assistant(&w.id) {
            "/chats".to_owned()
        } else {
            format!("/w/{}", w.id)
        };
        alerts::notify(
            &format!("{} {what}", fmt::ws_label(&w.name, &w.node)),
            &w.path,
            &format!("blazar-{}", w.id),
            w.activity == "awaiting_approval",
            move || {
                let _ = window().location().set_href(&href);
            },
        );
    }
}

pub fn provide(bus: Bus) -> AppData {
    let app = AppData {
        state: RwSignal::new(None),
        state_error: RwSignal::new(None),
        inbox_unread: RwSignal::new(0),
        tasks_review: RwSignal::new(0),
        autopilots_active: RwSignal::new(0),
        jobs_running: RwSignal::new(0),
        agents_n: RwSignal::new(0),
        skills_n: RwSignal::new(0),
        runtimes_n: RwSignal::new(0),
        bots: RwSignal::new(Vec::new()),
        chats: RwSignal::new(api::Assistant::default()),
        chats_refresh: StoredValue::new(false),
        seen: RwSignal::new(storage::load(SEEN_KEY).unwrap_or_default()),
        current_ws: RwSignal::new(None),
        selected_ws: RwSignal::new(None),
        inspect_ws: RwSignal::new(None),
        new_ws: RwSignal::new(false),
        new_ws_node: RwSignal::new(None),
        new_ws_parent: RwSignal::new(None),
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
        side_rail: RwSignal::new(false),
        settings_from: RwSignal::new("/".into()),
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
    Effect::new(move |_| {
        bus.workspaces.track();
        bus.resyncs.track();
        app.load_chats();
    });
    Effect::new(move |_| {
        bus.resyncs.track();
        for k in [
            "inbox",
            "tasks",
            "autopilots",
            "jobs",
            "agents",
            "skills",
            "runtimes",
        ] {
            app.load_counts(k);
        }
        app.load_bots();
    });
    bus.subscribe(move |ev| match ev {
        ServerEvent::BotsChanged => app.load_bots(),
        ServerEvent::SessionTitled { workspace_id, .. }
            if app.is_assistant(&workspace_id.to_string()) =>
        {
            app.load_chats()
        }
        ServerEvent::Entry {
            workspace_id,
            entry,
            ..
        } if app.is_assistant(&workspace_id.to_string())
            && matches!(
                entry.kind,
                blazar_core_types::EntryKind::SessionStarted { .. }
                    | blazar_core_types::EntryKind::Approval { .. }
                    | blazar_core_types::EntryKind::ApprovalResolved { .. }
                    | blazar_core_types::EntryKind::Finished(_)
            ) =>
        {
            app.load_chats_soon()
        }
        ServerEvent::InboxChanged => app.load_counts("inbox"),
        ServerEvent::TasksChanged => app.load_counts("tasks"),
        ServerEvent::AutopilotsChanged => app.load_counts("autopilots"),
        ServerEvent::JobsChanged => app.load_counts("jobs"),
        ServerEvent::AccountsChanged => app.load_counts("runtimes"),
        _ => {}
    });

    let last: StoredValue<Option<HashMap<String, String>>> = StoredValue::new(None);
    Effect::new(move |_| {
        let ws = app.workspaces();
        let n = ws
            .iter()
            .filter(|w| app.unseen(w) && (is_top(w, &ws) || w.activity == "awaiting_approval"))
            .count();
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
                    && match w.activity.as_str() {
                        "awaiting_approval" => true,
                        "completed" | "errored" => is_top(w, &ws),
                        _ => false,
                    }
            })
            .cloned()
            .collect();
        if !fresh.is_empty() {
            spawn_local(notify_changes(app, fresh));
        }
    });
    app
}
