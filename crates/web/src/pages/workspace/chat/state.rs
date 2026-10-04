use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use blazar_core_types::api::ServerEvent;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{self, Account, Accounts};
use crate::chat_model::{self, Row, Transcript};
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::realtime::Bus;
use crate::storage;

use super::super::diff_panel::{Comment, DiffState};
use super::super::files::Files;
use super::super::git_panel::Git;

#[path = "delivery.rs"]
mod delivery;
pub use delivery::{Deliveries, edit_request};
use delivery::{steer_request, take_composer};

#[path = "send_context.rs"]
mod send_context;
use send_context::SendContext;

#[path = "history.rs"]
mod history;
use history::{HistoryRequests, RowKey, merge_history};

pub const CONTINUE_TEXT: &str = "（已换账号接着做）请从刚才中断的地方继续，把没做完的工作完成。";
pub const ACC_RUNTIMES: [&str; 2] = ["claude", "codex"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Thread {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Queued {
    pub id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub images: usize,
    #[serde(default)]
    pub held: Option<String>,
    #[serde(default)]
    pub request: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Starter {
    pub label: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub runtime_label: String,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub starters: Vec<Starter>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub remote_hands: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct CliCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "argumentHint")]
    pub argument_hint: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Catalog {
    pub commands: Vec<CliCommand>,
    pub output_styles: Vec<String>,
    pub output_style: Option<String>,
    pub mcp_servers: Vec<blazar_core_types::McpServerStatus>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelSel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default = "yes")]
    pub thinking: bool,
    #[serde(default)]
    pub fast: bool,
    #[serde(default)]
    pub style: String,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            thinking: true,
            fast: false,
            style: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingMsg {
    pub id: u32,
    pub text: String,
    pub images: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attach {
    pub media_type: String,
    pub data: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SavedTabs {
    tabs: Vec<Option<String>>,
    active: Option<String>,
}

pub type Mode = (&'static str, &'static str, &'static str, &'static str);

pub const MODES_CLAUDE: [Mode; 5] = [
    (
        "default",
        "Manual",
        "Claude will ask for approval before making each edit",
        "hand",
    ),
    (
        "acceptEdits",
        "Edit automatically",
        "Claude will edit your selected text or the whole file",
        "code",
    ),
    (
        "plan",
        "Plan",
        "Claude will explore the code and present a plan before editing",
        "plan",
    ),
    (
        "auto",
        "Auto",
        "Claude will approve actions that pass a safety check and pause for anything risky",
        "bolt",
    ),
    (
        "bypassPermissions",
        "Bypass permissions",
        "Claude will not ask about anything. Use with care.",
        "warn",
    ),
];
pub const MODES_CODEX: [Mode; 3] = [
    (
        "read-only",
        "Read Only",
        "Codex can read files and answer questions. Codex requires approval to make edits, run commands, or access network.",
        "plan",
    ),
    (
        "workspace-write",
        "Auto",
        "Codex can read files, make edits, and run commands in the workspace. Codex requires approval to work outside the workspace or access network.",
        "bolt",
    ),
    (
        "danger-full-access",
        "Full Access",
        "Codex can read files, make edits, and run commands with network access, without approval. Exercise caution.",
        "warn",
    ),
];

pub fn default_mode(rt: &str) -> &'static str {
    match rt {
        "codex" => "workspace-write",
        _ => "auto",
    }
}

pub fn modes_for(rt: &str) -> &'static [Mode] {
    match rt {
        "claude" => &MODES_CLAUDE,
        "codex" => &MODES_CODEX,
        _ => &[],
    }
}

#[derive(Clone, Copy)]
pub struct Chat {
    pub ws: StoredValue<String>,
    pub root: StoredValue<String>,
    pub node: StoredValue<String>,
    pub files: Files,
    pub diff: DiffState,
    pub git: Git,
    pub running: Signal<bool>,
    pub show_aux: Callback<()>,
    pub show_diff: Callback<()>,

    pub tabs: RwSignal<Vec<Option<String>>>,
    pub view: RwSignal<Option<String>>,
    view_revision: StoredValue<u64>,
    history_requests: StoredValue<HistoryRequests>,
    refresh_pending: StoredValue<bool>,
    pub threads: RwSignal<Vec<Thread>>,
    pub rows: RwSignal<Vec<Row>>,
    seen: StoredValue<HashSet<String>>,
    sessions: StoredValue<HashSet<String>>,
    pub transcript: RwSignal<Rc<Transcript>, LocalStorage>,
    pub decided: RwSignal<HashSet<String>>,
    owner: StoredValue<Owner>,
    pub pending: RwSignal<Vec<PendingMsg>>,
    pub deliveries: RwSignal<Deliveries>,
    pub editing_queue: RwSignal<Option<Queued>>,
    orphans: StoredValue<HashMap<String, Vec<Row>>>,
    pub local_errors: RwSignal<Vec<String>>,
    pub loading: RwSignal<bool>,
    pub queue: RwSignal<Vec<Queued>>,
    pub checkpoints: RwSignal<HashMap<String, String>>,

    pub prompt: RwSignal<String>,
    pub attach: RwSignal<Vec<Attach>>,
    pub fresh: RwSignal<bool>,
    pub agent: RwSignal<String>,
    pub perm: RwSignal<String>,
    pub ctx_off: RwSignal<Option<String>>,
    pub prefs: RwSignal<Prefs>,
    pub model_rev: RwSignal<u32>,
    pub acc_rev: RwSignal<u32>,
    pub busy: RwSignal<u32>,

    pub agents: RwSignal<Vec<AgentInfo>>,
    pub installed: RwSignal<Option<Vec<String>>>,
    pub profiles: RwSignal<Vec<Profile>>,
    pub accounts: RwSignal<Option<Accounts>>,
    pub models: RwSignal<HashMap<String, Vec<ModelInfo>>>,
    pub catalog: RwSignal<Option<Catalog>>,
    pub snippets: RwSignal<Vec<Snippet>>,
    pub remote_codex: RwSignal<Option<bool>>,
}

fn tabs_key(ws: &str) -> String {
    format!("blazar.chats.{ws}")
}

impl Chat {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ws: &str,
        root: &str,
        node: &str,
        files: Files,
        diff: DiffState,
        git: Git,
        running: Signal<bool>,
        show_aux: Callback<()>,
        show_diff: Callback<()>,
    ) -> Self {
        Self {
            ws: StoredValue::new(ws.to_owned()),
            root: StoredValue::new(root.to_owned()),
            node: StoredValue::new(node.to_owned()),
            files,
            diff,
            git,
            running,
            show_aux,
            show_diff,
            tabs: RwSignal::new(Vec::new()),
            view: RwSignal::new(None),
            view_revision: StoredValue::new(0),
            history_requests: StoredValue::new(HistoryRequests::default()),
            refresh_pending: StoredValue::new(false),
            threads: RwSignal::new(Vec::new()),
            rows: RwSignal::new(Vec::new()),
            seen: StoredValue::new(HashSet::new()),
            sessions: StoredValue::new(HashSet::new()),
            transcript: RwSignal::new_local(Rc::new(Transcript::default())),
            decided: RwSignal::new(HashSet::new()),
            owner: StoredValue::new(Owner::current().unwrap_or_default()),
            pending: RwSignal::new(Vec::new()),
            deliveries: RwSignal::new(Deliveries::default()),
            editing_queue: RwSignal::new(None),
            orphans: StoredValue::new(HashMap::new()),
            local_errors: RwSignal::new(Vec::new()),
            loading: RwSignal::new(true),
            queue: RwSignal::new(Vec::new()),
            checkpoints: RwSignal::new(HashMap::new()),
            prompt: RwSignal::new(String::new()),
            attach: RwSignal::new(Vec::new()),
            fresh: RwSignal::new(false),
            agent: RwSignal::new(
                storage::load_raw(&format!("blazar.agent.{ws}")).unwrap_or_default(),
            ),
            perm: RwSignal::new(String::new()),
            ctx_off: RwSignal::new(None),
            prefs: RwSignal::new(storage::load("blazar.prefs.claude").unwrap_or_default()),
            model_rev: RwSignal::new(0),
            acc_rev: RwSignal::new(0),
            busy: RwSignal::new(0),
            agents: RwSignal::new(Vec::new()),
            installed: RwSignal::new(None),
            profiles: RwSignal::new(Vec::new()),
            accounts: RwSignal::new(None),
            models: RwSignal::new(HashMap::new()),
            catalog: RwSignal::new(None),
            snippets: RwSignal::new(Vec::new()),
            remote_codex: RwSignal::new(None),
        }
    }

    pub fn ws_id(self) -> String {
        self.ws.try_get_value().unwrap_or_default()
    }

    pub fn spawn(self, fut: impl std::future::Future<Output = ()> + 'static) {
        if let Some(o) = self.owner.try_get_value() {
            o.with(|| leptos::task::spawn_local_scoped_with_cancellation(fut));
        }
    }

    pub fn alive(self) -> bool {
        self.ws.try_get_value().is_some()
    }

    pub fn remote(self) -> bool {
        self.node.try_with_value(|n| n != "local").unwrap_or(false)
    }

    pub fn profile(self) -> Option<Profile> {
        let a = self.agent.get();
        let id = a.strip_prefix("p:")?;
        self.profiles
            .with(|p| p.iter().find(|x| x.id == id).cloned())
    }

    pub fn runtime(self) -> String {
        let a = self.agent.get();
        if let Some(id) = a.strip_prefix("p:") {
            return self
                .profiles
                .with(|p| p.iter().find(|x| x.id == id).map(|x| x.runtime.clone()))
                .unwrap_or_else(|| "claude".into());
        }
        a.strip_prefix("r:").unwrap_or("claude").to_owned()
    }

    pub fn agent_label(self) -> String {
        let a = self.agent.get();
        if let Some(p) = self.profile() {
            return format!("{} · {}", p.name, p.runtime_label);
        }
        let rt = a.strip_prefix("r:").unwrap_or("");
        self.agents
            .with(|l| l.iter().find(|x| x.id == rt).map(|x| x.label.clone()))
            .unwrap_or_else(|| "Select agent".into())
    }

    pub fn modes(self) -> &'static [Mode] {
        modes_for(&self.runtime())
    }

    pub fn effective_mode(self) -> Option<Mode> {
        let list = self.modes();
        let pick = |v: &str| list.iter().find(|m| m.0 == v).copied();
        let p = self.perm.get();
        pick(&p)
            .or_else(|| {
                self.profile()
                    .and_then(|x| x.permission_mode)
                    .and_then(|m| pick(&m))
            })
            .or_else(|| pick(default_mode(&self.runtime())))
            .or_else(|| list.first().copied())
    }

    pub fn model_sel(self, rt: &str) -> ModelSel {
        self.model_rev.track();
        storage::load(&format!("blazar.model.{rt}")).unwrap_or_default()
    }

    pub fn set_model_sel(self, rt: &str, v: &ModelSel) {
        let prev = self.model_sel(rt);
        storage::save(&format!("blazar.model.{rt}"), v);
        self.model_rev.update(|n| *n += 1);
        if prev.model != v.model {
            self.live(
                json!({ "model": v.model.clone().unwrap_or_default() }),
                "model",
            );
        }
    }

    pub fn set_prefs(self, f: impl FnOnce(&mut Prefs), live: Option<(Value, &'static str)>) {
        self.prefs.update(f);
        storage::save("blazar.prefs.claude", &self.prefs.get_untracked());
        if let Some((body, what)) = live {
            self.live(body, what);
        }
    }

    fn acc_key(self, thread: Option<&str>) -> String {
        format!("blazar.acc.{}.{}", self.ws_id(), thread.unwrap_or("new"))
    }

    pub fn acc_sel(self) -> String {
        self.acc_rev.track();
        let v = self.view.get();
        storage::load_raw(&self.acc_key(v.as_deref())).unwrap_or_default()
    }

    pub fn set_acc_sel(self, id: &str, thread: Option<&str>) {
        let key = self.acc_key(thread);
        if id.is_empty() {
            storage::remove(&key);
        } else {
            storage::save_raw(&key, id);
        }
        self.acc_rev.update(|n| *n += 1);
    }

    pub fn current_account(self, rt: &str) -> Option<String> {
        let sel = self.acc_sel();
        if !sel.is_empty() && sel != "auto" {
            return Some(sel);
        }
        let policy = self
            .accounts
            .with(|a| a.as_ref().and_then(|a| a.modes.get(rt).cloned()))
            .unwrap_or_default();
        match policy.as_str() {
            "" => Some(format!("{rt}-default")),
            "auto" => None,
            id => Some(id.to_owned()),
        }
    }

    pub fn account(self, id: &str) -> Option<Account> {
        self.accounts.with(|a| {
            a.as_ref()
                .and_then(|a| a.accounts.iter().find(|x| x.id == id).cloned())
        })
    }

    pub fn models_for(self, rt: &str) -> Vec<ModelInfo> {
        if let Some(m) = self.models.with(|m| m.get(rt).cloned()) {
            return m;
        }
        let rt = rt.to_owned();
        self.models.update(|m| {
            m.insert(
                rt.clone(),
                vec![ModelInfo {
                    id: String::new(),
                    label: "Default".into(),
                    desc: String::new(),
                    efforts: vec![],
                }],
            );
        });
        self.spawn(async move {
            if let Ok(v) =
                api::get::<Value>(&format!("/api/runtimes/{}/models", api::enc(&rt))).await
                && let Ok(list) = serde_json::from_value::<Vec<ModelInfo>>(v["models"].clone())
                && !list.is_empty()
            {
                let _ = self.models.try_update(|m| {
                    m.insert(rt, list);
                });
            }
        });
        Vec::new()
    }

    pub fn ctx_file(self) -> Option<String> {
        let cur = self.files.current.get()?;
        (self.ctx_off.get().as_deref() != Some(cur.as_str())).then_some(cur)
    }

    pub fn load_catalogs(self) {
        self.spawn(async move {
            if let Ok(a) = api::get::<Vec<AgentInfo>>("/api/agents").await {
                let _ = self.agents.try_set(a);
            }
            if let Ok(p) = api::get::<Vec<Profile>>("/api/agent-profiles").await {
                let _ = self.profiles.try_set(p);
            }
            if let Ok(r) = api::get::<api::Runtimes>("/api/runtimes").await {
                let ok = r
                    .runtimes
                    .iter()
                    .filter(|x| x.installed && x.authed != Some(false))
                    .map(|x| x.id.clone())
                    .collect();
                let _ = self.installed.try_set(Some(ok));
            }
            self.fix_agent();
        });
        self.load_accounts();
        if self.remote() {
            let node = self.node.get_value();
            self.spawn(async move {
                if let Ok(v) =
                    api::get::<Vec<Value>>(&format!("/api/nodes/{}/agents", api::enc(&node))).await
                {
                    let authed = v
                        .iter()
                        .find(|x| x["id"] == "codex")
                        .and_then(|x| x["authed"].as_bool());
                    let _ = self.remote_codex.try_set(authed);
                }
            });
        }
    }

    pub fn load_accounts(self) {
        self.spawn(async move {
            if let Ok(a) = api::get::<Accounts>("/api/accounts").await {
                let _ = self.accounts.try_set(Some(a));
            }
        });
    }

    pub fn load_catalog(self) {
        if self.catalog.with_untracked(Option::is_some) {
            return;
        }
        self.catalog.set(Some(Catalog::default()));
        self.spawn(async move {
            if let Ok(c) = api::get::<Catalog>("/api/runtimes/claude/catalog").await {
                let _ = self.catalog.try_set(Some(c));
            }
        });
    }

    pub fn load_snippets(self) {
        self.spawn(async move {
            if let Ok(s) = api::get::<Vec<Snippet>>("/api/snippets").await {
                let _ = self.snippets.try_set(s);
            }
        });
    }

    pub fn runtimes(self) -> Vec<AgentInfo> {
        let installed = self.installed.get();
        let remote = self.remote();
        self.agents
            .get()
            .into_iter()
            .filter(|a| {
                installed.as_ref().is_none_or(|i| i.contains(&a.id)) && (!remote || a.remote_hands)
            })
            .collect()
    }

    fn fix_agent(self) {
        if !self.alive() {
            return;
        }
        let cur = self.agent.get_untracked();
        let ok = if let Some(id) = cur.strip_prefix("p:") {
            self.profiles
                .with_untracked(|p| p.iter().any(|x| x.id == id))
        } else if let Some(id) = cur.strip_prefix("r:") {
            self.runtimes_untracked().iter().any(|x| x.id == id)
        } else {
            false
        };
        if !ok {
            let first = self
                .profiles
                .with_untracked(|p| p.first().map(|x| format!("p:{}", x.id)))
                .or_else(|| {
                    self.runtimes_untracked()
                        .first()
                        .map(|x| format!("r:{}", x.id))
                });
            if let Some(f) = first {
                self.agent.set(f);
            }
        }
    }

    fn runtimes_untracked(self) -> Vec<AgentInfo> {
        untrack(move || self.runtimes())
    }

    pub fn set_agent(self, v: String) {
        storage::save_raw(&format!("blazar.agent.{}", self.ws_id()), &v);
        self.agent.set(v);
        self.perm.set(String::new());
    }

    pub async fn load_threads(self) {
        let ws = self.ws_id();
        if let Ok(t) = api::get::<Vec<Thread>>(&format!("/api/workspaces/{ws}/sessions")).await {
            let _ = self.threads.try_set(t);
        }
    }

    pub fn thread_title(self, id: Option<&str>) -> String {
        match id {
            None => "New chat".to_owned(),
            Some(id) => self
                .threads
                .with(|t| t.iter().find(|x| x.id == id).and_then(|x| x.title.clone()))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Conversation".to_owned()),
        }
    }

    pub fn thread_running(self, id: &str) -> bool {
        self.running.get()
            && self.threads.with(|t| {
                t.iter()
                    .any(|x| x.id == id && x.status.as_deref() == Some("running"))
            })
    }

    fn save_tabs(self) {
        storage::save(
            &tabs_key(&self.ws_id()),
            &SavedTabs {
                tabs: self.tabs.get_untracked(),
                active: self.view.get_untracked(),
            },
        );
    }

    pub fn init(self, requested: Signal<Option<String>>, ready: RwSignal<bool>) {
        self.spawn(async move {
            self.load_threads().await;
            let saved: SavedTabs = storage::load(&tabs_key(&self.ws_id())).unwrap_or_default();
            let known: HashSet<String> = self
                .threads
                .with_untracked(|t| t.iter().map(|x| x.id.clone()).collect());
            let mut ids: Vec<Option<String>> = saved
                .tabs
                .into_iter()
                .filter(|t| t.as_ref().is_none_or(|id| known.contains(id)))
                .collect();
            if ids.is_empty() {
                ids.push(
                    self.threads
                        .with_untracked(|t| t.first().map(|x| x.id.clone())),
                );
            }
            let target = requested.get_untracked().filter(|id| known.contains(id));
            if let Some(id) = &target
                && !ids.contains(&Some(id.clone()))
            {
                ids.push(Some(id.clone()));
            }
            let active = if target.is_some() {
                target
            } else if ids.contains(&saved.active) {
                saved.active
            } else {
                ids[0].clone()
            };
            let _ = self.tabs.try_set(ids);
            let _ = self.view.try_set(active);
            self.load_history();
            let _ = ready.try_set(true);
        });
        self.load_queue();
    }

    pub fn activate(self, id: Option<String>) {
        self.view_revision.update_value(|r| *r = r.wrapping_add(1));
        self.tabs.update(|t| {
            if !t.contains(&id) {
                t.push(id.clone());
            }
        });
        self.fresh.set(id.is_none());
        self.view.set(id);
        self.save_tabs();
        self.load_history();
    }

    pub fn new_chat(self) {
        self.activate(None);
    }

    pub fn close_tab(self, id: Option<String>) {
        let Some(i) = self
            .tabs
            .with_untracked(|t| t.iter().position(|x| *x == id))
        else {
            return;
        };
        self.tabs.update(|t| {
            t.remove(i);
            if t.is_empty() {
                t.push(None);
            }
        });
        if self.view.get_untracked() == id {
            let next = self.tabs.with_untracked(|t| t[i.min(t.len() - 1)].clone());
            self.activate(next);
        } else {
            self.save_tabs();
        }
    }

    pub fn rename(self, id: String, title: String) {
        self.spawn(async move {
            match api::send::<Value>(
                "PUT",
                &format!("/api/sessions/{id}/title"),
                &json!({ "title": title }),
            )
            .await
            {
                Ok(r) if r["ok"].as_bool() == Some(true) => {
                    let t = r["title"].as_str().unwrap_or(&title).to_owned();
                    self.threads.update(|l| {
                        if let Some(x) = l.iter_mut().find(|x| x.id == id) {
                            x.title = Some(t);
                        }
                    });
                }
                Ok(r) => toast(r["reason"].as_str().unwrap_or("Rename failed").to_owned()),
                Err(e) => toast(format!("Rename failed: {e}")),
            }
        });
    }

    pub fn load_history(self) {
        let view = self.view.get_untracked();
        let (changed, request) = self.history_requests.write_value().begin(view.clone());
        if changed {
            self.sessions.set_value(view.iter().cloned().collect());
            self.seen.set_value(HashSet::new());
            self.rows.set(Vec::new());
        }
        let baseline: HashSet<RowKey> = self.rows.with_untracked(|rows| {
            rows.iter()
                .map(|row| (row.session_id.clone(), row.seq))
                .collect()
        });
        self.decided.set(HashSet::new());
        self.local_errors.set(Vec::new());
        let Some(thread) = view else {
            self.sessions.set_value(HashSet::new());
            self.seen.set_value(HashSet::new());
            self.rows.set(Vec::new());
            self.loading.set(false);
            return;
        };
        self.loading.set(true);
        let ws = self.ws_id();
        self.spawn(async move {
            let r = api::get::<Vec<Value>>(&format!(
                "/api/workspaces/{ws}/history?session={}",
                api::enc(&thread)
            ))
            .await;
            if self.view.try_get_untracked().flatten().as_deref() != Some(thread.as_str())
                || !self
                    .history_requests
                    .try_with_value(|requests| requests.accepts(&request))
                    .unwrap_or(false)
            {
                return;
            }
            match r {
                Ok(list) => {
                    let history: Vec<Row> =
                        list.into_iter().filter_map(Row::from_history).collect();
                    let rows = self.rows.with_untracked(|current| {
                        merge_history(history, current, &baseline, |row| {
                            (row.session_id.clone(), row.seq)
                        })
                    });
                    self.sessions.set_value(
                        rows.iter()
                            .map(|r| r.session_id.clone())
                            .chain([thread.clone()])
                            .collect(),
                    );
                    self.seen.set_value(
                        rows.iter()
                            .map(|r| format!("{}:{}", r.session_id, r.seq))
                            .collect(),
                    );
                    self.rows.set(rows);
                }
                Err(e) => self.local_errors.set(vec![e.to_string()]),
            }
            self.loading.set(false);
            self.load_checkpoints();
        });
    }

    pub fn load_checkpoints(self) {
        let ws = self.ws_id();
        self.spawn(async move {
            if let Ok(list) =
                api::get::<Vec<Value>>(&format!("/api/workspaces/{ws}/checkpoints")).await
            {
                let map = list
                    .iter()
                    .filter_map(|c| {
                        Some((
                            format!("{}:{}", c["session_id"].as_str()?, c["seq"].as_u64()?),
                            c["id"].as_str()?.to_owned(),
                        ))
                    })
                    .collect();
                let _ = self.checkpoints.try_set(map);
            }
        });
    }

    pub fn load_queue(self) {
        let ws = self.ws_id();
        self.spawn(async move {
            if let Ok(q) = api::get::<Vec<Queued>>(&format!("/api/workspaces/{ws}/queue")).await {
                let _ = self.queue.try_set(q);
            }
        });
    }

    pub fn on_event(self, ev: &ServerEvent) {
        let ws = self.ws_id();
        match ev {
            ServerEvent::Entry {
                workspace_id,
                session_id,
                entry,
            } if workspace_id.to_string() == ws => {
                let sid = session_id.to_string();
                if matches!(entry.kind, blazar_core_types::EntryKind::Finished(_)) {
                    self.git.changed.update(|n| *n = n.wrapping_add(1));
                    self.git.reload.update(|n| *n = n.wrapping_add(1));
                    self.spawn(async move { self.load_threads().await });
                }
                let row = Row::from_live(sid.clone(), (**entry).clone());
                if self.sessions.with_value(|s| s.contains(&sid)) {
                    self.push_row(row);
                } else if self.pending.with_untracked(|p| !p.is_empty()) {
                    let mine = matches!(&row.kind, blazar_core_types::EntryKind::UserMessage { text } if self.matches_pending(text));
                    self.orphans
                        .update_value(|o| o.entry(sid.clone()).or_default().push(row));
                    if mine {
                        self.adopt(&sid);
                    }
                } else {
                    self.refresh_threads_soon();
                }
            }
            ServerEvent::QueueChanged { workspace_id } if workspace_id.to_string() == ws => {
                self.load_queue()
            }
            ServerEvent::QueueSent {
                workspace_id,
                queued_thread,
                thread_id,
                session_id,
            } if workspace_id.to_string() == ws => {
                if queued_thread.as_ref().map(ToString::to_string) == self.view.get_untracked() {
                    self.note_sent(Some(session_id.to_string()), Some(thread_id.to_string()));
                } else {
                    self.refresh_threads_soon();
                }
            }
            ServerEvent::SessionTitled {
                workspace_id,
                session_id,
                title,
            } if workspace_id.to_string() == ws => {
                let id = session_id.to_string();
                let known = self
                    .threads
                    .with_untracked(|t| t.iter().any(|x| x.id == id));
                if known {
                    self.threads.update(|l| {
                        if let Some(x) = l.iter_mut().find(|x| x.id == id) {
                            x.title = Some(title.clone());
                        }
                    });
                } else {
                    self.refresh_threads_soon();
                }
            }
            ServerEvent::AccountsChanged => self.load_accounts(),
            _ => {}
        }
    }

    fn refresh_threads_soon(self) {
        if self.refresh_pending.get_value() {
            return;
        }
        self.refresh_pending.set_value(true);
        gloo_timers::callback::Timeout::new(2500, move || {
            let _ = self.refresh_pending.try_set_value(false);
            if self.alive() {
                self.spawn(async move { self.load_threads().await });
                self.load_history();
            }
        })
        .forget();
    }

    fn send_context(self, thread: Option<String>) -> SendContext {
        SendContext {
            workspace: self.ws_id(),
            thread,
            revision: self.view_revision.get_value(),
        }
    }

    fn context_is_current(self, context: &SendContext) -> bool {
        self.view.try_get_untracked().is_some_and(|thread| {
            context.matches(
                &self.ws_id(),
                thread.as_deref(),
                self.view_revision.try_get_value().unwrap_or_default(),
            )
        })
    }

    fn note_sent_in(
        self,
        context: &SendContext,
        session_id: Option<String>,
        thread_id: Option<String>,
    ) {
        if self.context_is_current(context) {
            self.note_sent(session_id, thread_id);
        } else if self.alive() {
            if let Some(thread) = thread_id {
                self.tabs.update(|tabs| {
                    if !tabs.contains(&Some(thread.clone())) {
                        tabs.push(Some(thread));
                    }
                });
                self.save_tabs();
            }
            self.refresh_threads_soon();
        }
    }

    pub fn note_sent(self, session_id: Option<String>, thread_id: Option<String>) {
        let Some(sid) = session_id else { return };
        let known = self.sessions.with_value(|s| s.contains(&sid))
            || self.orphans.with_value(|o| o.contains_key(&sid));
        self.adopt(&sid);
        let view = self.view.get_untracked();
        let changed = thread_id
            .as_ref()
            .is_some_and(|thread| Some(thread) != view.as_ref());
        if let Some(t) = thread_id.filter(|t| Some(t) != view.as_ref()) {
            self.tabs.update(|tabs| {
                if view.is_none() {
                    if let Some(x) = tabs.iter_mut().find(|x| x.is_none()) {
                        *x = Some(t.clone());
                    }
                } else if !tabs.contains(&Some(t.clone())) {
                    tabs.push(Some(t.clone()));
                }
            });
            if view.is_none() {
                let a = storage::load_raw(&self.acc_key(None)).unwrap_or_default();
                if !a.is_empty() {
                    self.set_acc_sel(&a, Some(&t));
                    self.set_acc_sel("", None);
                }
            }
            self.view.set(Some(t));
            self.fresh.set(false);
            self.save_tabs();
            self.spawn(async move { self.load_threads().await });
        }
        if !known || changed {
            self.load_history();
        }
    }

    fn push_row(self, row: Row) {
        let key = format!("{}:{}", row.session_id, row.seq);
        if self.seen.with_value(|s| s.contains(&key)) {
            return;
        }
        self.seen.update_value(|s| {
            s.insert(key);
        });
        if let blazar_core_types::EntryKind::UserMessage { text } = &row.kind {
            self.take_pending(text);
            self.load_checkpoints();
        }
        self.rows.update(|r| r.push(row));
    }

    fn adopt(self, sid: &str) {
        self.sessions.update_value(|s| {
            s.insert(sid.to_owned());
        });
        let rows = self
            .orphans
            .try_update_value(|o| o.remove(sid))
            .flatten()
            .unwrap_or_default();
        for r in rows {
            self.push_row(r);
        }
    }

    fn matches_pending(self, text: &str) -> bool {
        let t = text.trim();
        self.pending
            .with_untracked(|p| p.iter().any(|m| t.starts_with(m.text.trim())))
    }

    fn take_pending(self, text: &str) {
        let t = text.trim().to_owned();
        self.pending.update(|p| {
            if let Some(i) = p.iter().position(|m| t.starts_with(m.text.trim())) {
                p.remove(i);
            }
        });
    }

    fn drop_pending(self, id: u32) {
        let _ = self.pending.try_update(|p| p.retain(|m| m.id != id));
    }

    fn send_options(self) -> serde_json::Map<String, Value> {
        let rt = untrack(move || self.runtime());
        let agent = self.agent.get_untracked();
        let sel = {
            let rt = rt.clone();
            untrack(move || self.model_sel(&rt))
        };
        let mut o = serde_json::Map::new();
        o.insert("resume".into(), json!(!self.fresh.get_untracked()));
        o.insert("model".into(), json!(sel.model.filter(|m| !m.is_empty())));
        o.insert("effort".into(), json!(sel.effort));
        let perm = untrack(move || self.effective_mode()).map(|m| m.0);
        o.insert("permission_mode".into(), json!(perm));
        o.insert("agent".into(), json!(agent.strip_prefix("r:")));
        o.insert("profile".into(), json!(agent.strip_prefix("p:")));
        o.insert(
            "brain".into(),
            json!(if self.remote() { "node" } else { "local" }),
        );
        let acc = (agent.starts_with("r:") && ACC_RUNTIMES.contains(&rt.as_str()))
            .then(|| untrack(move || self.acc_sel()))
            .filter(|a| !a.is_empty());
        o.insert("account".into(), json!(acc));
        if rt == "claude" {
            let p = self.prefs.get_untracked();
            o.insert(
                "thinking".into(),
                if p.thinking {
                    Value::Null
                } else {
                    json!(false)
                },
            );
            o.insert(
                "fast_mode".into(),
                if p.fast { json!(true) } else { Value::Null },
            );
            o.insert(
                "output_style".into(),
                if p.style.is_empty() {
                    Value::Null
                } else {
                    json!(p.style)
                },
            );
        }
        o
    }

    fn review_block(list: &[Comment]) -> String {
        if list.is_empty() {
            return String::new();
        }
        let items: Vec<String> = list
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let code: String = c.code.trim().chars().take(200).collect();
                format!(
                    "{}. `{}` 第 {} 行{}\n   > {code}\n   {}",
                    i + 1,
                    c.file,
                    c.line,
                    if c.side == "old" {
                        "（被删掉的那一行）"
                    } else {
                        ""
                    },
                    c.text.replace('\n', "\n   ")
                )
            })
            .collect();
        format!(
            "\n\n---\n审阅意见（{} 条），请逐条处理：\n\n{}",
            list.len(),
            items.join("\n\n")
        )
    }

    pub fn clear_review(self) {
        self.diff.comments.set(Vec::new());
        storage::remove(&format!("blazar.review.{}", self.ws_id()));
    }

    pub fn send(self) {
        let reviews = self.diff.comments.get_untracked();
        if self.prompt.with_untracked(|p| p.trim().is_empty())
            && self.attach.with_untracked(Vec::is_empty)
            && reviews.is_empty()
        {
            return;
        }
        let (text, images) = take_composer(&mut self.prompt.write(), &mut self.attach.write());
        let text = if text.trim().is_empty() {
            if reviews.is_empty() {
                "看一下这张图"
            } else {
                "请逐条处理下面的审阅意见。"
            }
            .to_owned()
        } else {
            text.trim().to_owned()
        };
        let wire = format!("{text}{}", Self::review_block(&reviews));
        let mut body = self.send_options();
        body.insert("text".into(), json!(wire));
        body.insert("resume_session".into(), json!(self.view.get_untracked()));
        body.insert(
            "images".into(),
            json!(
                images
                    .iter()
                    .map(|a| json!({ "media_type": a.media_type, "data": a.data }))
                    .collect::<Vec<_>>()
            ),
        );
        body.insert(
            "context_file".into(),
            json!(untrack(move || self.ctx_file())),
        );
        body.insert("wait_secs".into(), json!(20));
        if !reviews.is_empty() {
            self.clear_review();
        }
        self.send_request(Value::Object(body));
    }

    pub fn retry_failed(self, id: u32) {
        let request = self.deliveries.write().take_failed(id);
        if let Some(request) = request {
            self.send_request(request);
        }
    }

    fn send_request(self, body: Value) {
        let pid = self.deliveries.write().begin(body.clone());
        self.pending.update(|p| {
            p.push(PendingMsg {
                id: pid,
                text: body["text"].as_str().unwrap_or_default().to_owned(),
                images: body["images"].as_array().map_or(0, Vec::len),
            })
        });
        self.orphans.set_value(HashMap::new());
        self.show_aux.run(());
        self.busy.update(|n| *n += 1);
        let context = self.send_context(body["resume_session"].as_str().map(str::to_owned));
        let ws = context.workspace.clone();
        self.spawn(async move {
            let r =
                api::send::<Value>("POST", &format!("/api/workspaces/{ws}/prompt"), &body).await;
            let _ = self.busy.try_update(|n| *n = n.saturating_sub(1));
            match r {
                Ok(r) if r["admitted"] == json!(false) => {
                    let q = api::send::<Vec<Queued>>(
                        "PUT",
                        &format!("/api/workspaces/{ws}/queue"),
                        &context.queue_request(&body),
                    )
                    .await;
                    self.drop_pending(pid);
                    self.deliveries.update(|d| d.finish(pid, q.is_ok()));
                    match q {
                        Ok(q) => {
                            self.queue.set(q);
                            toast("已排队，当前轮次结束后发送");
                        }
                        Err(e) => toast(format!("排队失败，消息已保留：{e}")),
                    }
                }
                Ok(r) => {
                    self.deliveries.update(|d| d.finish(pid, true));
                    let started = r["activity"]["started"].as_bool();
                    let sid = r["session_id"].as_str().map(str::to_owned);
                    let tid = r["thread_id"].as_str().map(str::to_owned);
                    if sid.is_some() {
                        self.note_sent_in(&context, sid, tid);
                    }
                    if self.context_is_current(&context) {
                        self.fresh.set(false);
                    }
                    if started == Some(false) {
                        self.drop_pending(pid);
                        let why = r["activity"]["reason"].as_str().unwrap_or("").to_owned();
                        toast(if why.is_empty() {
                            "The agent did not start".to_owned()
                        } else {
                            why.clone()
                        });
                        let class = r["activity"]["failure_class"]
                            .as_str()
                            .map(|c| format!(" ({c})"))
                            .unwrap_or_default();
                        if self.context_is_current(&context) {
                            self.local_errors.update(|e| {
                                e.push(format!("The agent did not start: {why}{class}"))
                            });
                        }
                    }
                }
                Err(e) => {
                    self.drop_pending(pid);
                    self.deliveries.update(|d| d.finish(pid, false));
                    toast(format!("发送失败，消息已保留：{e}"));
                }
            }
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            self.drop_pending(pid);
        });
    }

    pub fn stop(self) {
        let Some(sid) = self.transcript.with_untracked(|t| t.last_session.clone()) else {
            toast("Nothing is running in this conversation");
            return;
        };
        self.spawn(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/sessions/{sid}/interrupt"),
                &json!({}),
            )
            .await
            {
                Ok(r) if r["interrupted"].as_bool() == Some(true) => toast("Interrupted"),
                Ok(r) => toast(
                    r["reason"]
                        .as_str()
                        .unwrap_or("Couldn't interrupt")
                        .to_owned(),
                ),
                Err(e) => toast(format!("Interrupt failed: {e}")),
            }
        });
    }

    pub fn live(self, body: Value, what: &'static str) {
        if !self.running.get_untracked() {
            return;
        }
        let Some(sid) = self.transcript.with_untracked(|t| t.last_session.clone()) else {
            return;
        };
        self.spawn(async move {
            match api::send::<Value>("POST", &format!("/api/sessions/{sid}/control"), &body).await {
                Ok(r) if r["accepted"].as_bool() == Some(true) => {
                    toast(format!("Changed {what} for the current turn"))
                }
                Ok(r) => toast(format!(
                    "The {what} change applies from the next turn ({})",
                    r["reason"].as_str().unwrap_or("")
                )),
                Err(_) => toast(format!("The {what} change applies from the next turn")),
            }
        });
    }

    pub fn set_mode(self, v: &str) {
        self.perm.set(v.to_owned());
        if !v.is_empty() {
            self.live(json!({ "permission_mode": v }), "permission mode");
        }
    }

    pub fn cycle_mode(self) {
        let list = untrack(move || self.modes());
        if list.is_empty() {
            return;
        }
        let cur = untrack(move || self.effective_mode()).map(|m| m.0);
        let i = list
            .iter()
            .position(|m| Some(m.0) == cur)
            .map_or(0, |i| (i + 1) % list.len());
        self.set_mode(list[i].0);
    }

    pub fn decide(self, id: String, allow: bool, message: String, answers: Option<Value>) {
        self.decided.update(|d| {
            d.insert(id.clone());
        });
        self.spawn(async move {
            let mut body = json!({ "allow": allow, "message": message });
            if let Some(a) = answers {
                body["answers"] = a;
            }
            match api::send::<Value>("POST", &format!("/api/approvals/{id}"), &body).await {
                Ok(r) if r["delivered"].as_bool() == Some(true) => {}
                Ok(r) => toast(r["reason"].as_str().unwrap_or("Not delivered").to_owned()),
                Err(e) => {
                    toast(format!("Couldn't send the decision: {e}"));
                    self.decided.update(|d| {
                        d.remove(&id);
                    });
                }
            }
        });
    }

    pub fn allow_always(self, id: String, request: Value) {
        let ws = self.ws_id();
        self.spawn(async move {
            let (tool, pattern) = chat_model::always_rule(&request);
            let what = if pattern.is_empty() {
                tool.clone()
            } else {
                format!("{tool} {pattern}…")
            };
            let ok = dialog::ask(
                "Don't ask again",
                &format!("Always allow \"{what}\" in this workspace without asking?\n\nYou can turn this off in Settings → Auto-approval."),
                vec![Choice::plain("Cancel"), Choice::plain("Always allow")],
            )
            .await;
            if ok != Some(1) {
                return;
            }
            match api::send::<Value>(
                "POST",
                "/api/approval-rules",
                &json!({ "tool": tool, "pattern": pattern, "workspace_id": ws }),
            )
            .await
            {
                Ok(_) => self.decide(id, true, String::new(), None),
                Err(e) => toast(e.to_string()),
            }
        });
    }

    pub fn queue_act(self, act: &'static str, q: Queued) {
        if act == "edit" {
            if q.request.is_some() {
                self.editing_queue.set(Some(q));
            } else {
                toast("这条消息含有不能在此编辑的设置，已保留在队列中");
            }
            return;
        }
        let context = self.send_context(q.thread_id.clone());
        let ws = context.workspace.clone();
        let steer_session = self.transcript.with_untracked(|t| t.last_session.clone());
        let base = format!("/api/workspaces/{ws}/queue/{}", q.id);
        self.spawn(async move {
            let r: Result<(), api::ApiError> = async {
                match act {
                    "drop" => {
                        api::send::<Value>("DELETE", &base, &json!({})).await?;
                        toast("Removed");
                    }
                    "send" => {
                        let r = api::send::<Value>("POST", &format!("{base}/send"), &json!({})).await?;
                        self.note_sent_in(&context, r["session_id"].as_str().map(str::to_owned), r["thread_id"].as_str().map(str::to_owned));
                    }
                    _ => {
                        let Some(sid) = steer_session else {
                            toast("No running session to steer");
                            return Ok(());
                        };
                        let Some(request) = q.request.as_ref() else {
                            toast("这条消息的设置无法用于插话，已保留在队列中");
                            return Ok(());
                        };
                        if dialog::ask("发送插话？", "附件和文件上下文会一并发送；模型、账号和权限沿用当前轮次，原排队消息的启动设置不会应用。", vec![Choice::plain("取消"), Choice::plain("发送插话")]).await != Some(1) {
                            return Ok(());
                        }
                        if !self.context_is_current(&context) {
                            toast("对话已切换，消息仍保留在原队列中");
                            return Ok(());
                        }
                        let i = api::send::<Value>("POST", &format!("/api/sessions/{sid}/input"), &steer_request(request)).await?;
                        if i["accepted"].as_bool() != Some(true) {
                            toast(i["reason"].as_str().unwrap_or("This runtime can't take messages mid-turn. It sends when the turn ends.").to_owned());
                            return Ok(());
                        }
                        api::send::<Value>("DELETE", &base, &json!({"expected": request})).await?;
                        toast("Sent into the current turn");
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = r {
                toast(e.to_string());
            }
            self.load_queue();
        });
    }

    pub fn retry(self, sid: String, text: Option<String>, later: usize) {
        if self.running.get_untracked() {
            toast("The agent is still running. Interrupt it first.");
            return;
        }
        let context = self.send_context(self.view.get_untracked());
        let ws = context.workspace.clone();
        let mut options = self.send_options();
        options.insert("wait_secs".into(), json!(20));
        self.spawn(async move {
            let head = if text.is_none() {
                "Retry this turn"
            } else {
                "Retry with the edited message"
            };
            let tail = if later > 0 {
                format!(" and the {later} turn{} after it", if later == 1 { "" } else { "s" })
            } else {
                String::new()
            };
            let body = format!(
                "· Files go back to how they were before this message (the current state is saved first)\n· This message{tail} are marked as rewound and leave the context\n· Folders outside git have no checkpoints; only the conversation restarts"
            );
            if dialog::ask(
                &format!("{head}？"),
                &body,
                vec![Choice::plain("Cancel"), Choice::danger("Retry")],
            )
            .await
                != Some(1)
            {
                return;
            }
            if !self.context_is_current(&context) {
                toast("对话已切换，未重新发送");
                return;
            }
            match api::send::<Value>(
                "POST",
                &format!("/api/workspaces/{ws}/retry"),
                &json!({ "session_id": sid, "text": text, "options": options }),
            )
            .await
            {
                Ok(r) if r["admitted"] == json!(false) => {
                    toast(r["reason"].as_str().unwrap_or("The workspace is busy").to_owned())
                }
                Ok(r) => {
                    if r["session_id"].is_string() {
                        self.note_sent_in(
                            &context,
                            r["session_id"].as_str().map(str::to_owned),
                            r["thread_id"].as_str().map(str::to_owned),
                        );
                    }
                    let restored = r["files_restored"].as_bool() == Some(true);
                    if restored {
                        self.git.changed.update(|n| *n += 1);
                    }
                    toast(if restored {
                        "Files restored. Continuing from here."
                    } else {
                        "Continuing from here (no checkpoint, files unchanged)"
                    });
                }
                Err(e) => {
                    toast(format!("Retry failed: {e}"));
                    if self.context_is_current(&context) {
                        self.load_history();
                    }
                }
            }
        });
    }

    pub fn rewind(self, cp: String, undo: bool) {
        self.spawn(async move {
            if !undo
                && dialog::ask("Rewind", "Restore files to before this message?\nThe conversation stays; the current state is saved first so you can undo.", vec![Choice::plain("Cancel"), Choice::plain("Rewind")]).await != Some(1)
            {
                return;
            }
            match api::send::<Value>(
                "POST",
                &format!("/api/checkpoints/{cp}/restore"),
                &json!({}),
            )
            .await
            {
                Ok(r) => {
                    self.git.changed.update(|n| *n += 1);
                    if undo {
                        toast("Rewind undone");
                    } else if let Some(u) = r["undo"].as_str() {
                        let u = u.to_owned();
                        if dialog::ask(
                            "Rewound",
                            "Files are back to before this message. Undo?",
                            vec![Choice::plain("Keep"), Choice::plain("Undo")],
                        )
                        .await
                            == Some(1)
                        {
                            self.rewind(u, true);
                        }
                    } else {
                        toast("Rewound");
                    }
                }
                Err(e) => toast(format!("Rewind failed: {e}")),
            }
        });
    }

    pub fn continue_on_another(self, pick: String) {
        self.set_acc_sel(&pick, self.view.get_untracked().as_deref());
        let mut body = self.send_options();
        body.insert("text".into(), json!(CONTINUE_TEXT));
        body.insert("resume".into(), json!(true));
        body.insert("resume_session".into(), json!(self.view.get_untracked()));
        body.insert("account".into(), json!(pick));
        body.insert("wait_secs".into(), json!(20));
        let context = self.send_context(self.view.get_untracked());
        let ws = context.workspace.clone();
        let label = untrack(move || self.account(&pick))
            .map(|a| a.label)
            .unwrap_or_default();
        self.spawn(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/workspaces/{ws}/prompt"),
                &Value::Object(body),
            )
            .await
            {
                Ok(r) if r["admitted"] == json!(false) => toast(
                    r["reason"]
                        .as_str()
                        .unwrap_or("The workspace is busy")
                        .to_owned(),
                ),
                Ok(r) => {
                    self.note_sent_in(
                        &context,
                        r["session_id"].as_str().map(str::to_owned),
                        r["thread_id"].as_str().map(str::to_owned),
                    );
                    toast(format!("Continuing with {label}"));
                }
                Err(e) => toast(format!("Send failed: {e}")),
            }
        });
    }

    pub fn wire(self, bus: Bus) {
        let id = bus.subscribe(move |ev| self.on_event(ev));
        on_cleanup(move || bus.unsubscribe(id));
        Effect::new(move |prev: Option<()>| {
            bus.reconnects.track();
            if prev.is_some() {
                self.load_history();
                self.load_queue();
            }
        });
        Effect::new(move |_| {
            let running = self.running.get();
            let root = self.root.get_value();
            let t = self.rows.with(|r| chat_model::build(r, &root, running));
            self.transcript.set(Rc::new(t));
        });
    }
}
