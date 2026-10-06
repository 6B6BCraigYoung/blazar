use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use leptos::prelude::*;

use crate::api::Accounts;
use crate::chat_model::{Row, Transcript};
use crate::storage;

use super::super::diff_panel::{Comment, DiffState};
use super::super::files::Files;
use super::super::git_panel::Git;

#[path = "delivery.rs"]
mod delivery;
pub use delivery::{Deliveries, edit_request};

#[path = "send_context.rs"]
mod send_context;

#[path = "history.rs"]
mod history;
use history::HistoryRequests;

#[path = "approval_delivery.rs"]
mod approval_delivery;
use approval_delivery::ApprovalDeliveries;

mod actions;
mod catalog;
mod conversations;
mod sending;
mod types;

pub use types::*;

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
    row_buffer: StoredValue<Vec<Row>>,
    flush_scheduled: StoredValue<bool>,
    fs_refresh_pending: StoredValue<bool>,
    seen: StoredValue<HashSet<String>>,
    sessions: StoredValue<HashSet<String>>,
    pub transcript: RwSignal<Rc<Transcript>, LocalStorage>,
    pub decided: RwSignal<ApprovalDeliveries>,
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
            row_buffer: StoredValue::new(Vec::new()),
            flush_scheduled: StoredValue::new(false),
            fs_refresh_pending: StoredValue::new(false),
            seen: StoredValue::new(HashSet::new()),
            sessions: StoredValue::new(HashSet::new()),
            transcript: RwSignal::new_local(Rc::new(Transcript::default())),
            decided: RwSignal::new(ApprovalDeliveries::default()),
            owner: StoredValue::new(Owner::current().unwrap_or_default()),
            pending: RwSignal::new(Vec::new()),
            deliveries: RwSignal::new(Deliveries::default()),
            editing_queue: RwSignal::new(None),
            orphans: StoredValue::new(HashMap::new()),
            local_errors: RwSignal::new(Vec::new()),
            loading: RwSignal::new(true),
            queue: RwSignal::new(Vec::new()),
            checkpoints: RwSignal::new(HashMap::new()),
            prompt: RwSignal::new(
                storage::load_raw(&format!("blazar.draft.{ws}")).unwrap_or_default(),
            ),
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
}
