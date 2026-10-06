use std::collections::HashSet;
use std::rc::Rc;

use blazar_core_types::api::ServerEvent;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api;
use crate::chat_model::{self, Row};
use crate::components::toast::toast;
use crate::realtime::Bus;
use crate::storage;

use super::approval_delivery::ApprovalDeliveries;
use super::history::{RowKey, merge_history};
use super::send_context::clear_view_pending;
use super::{Chat, Queued, Thread};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SavedTabs {
    tabs: Vec<Option<String>>,
    active: Option<String>,
}

fn tabs_key(ws: &str) -> String {
    format!("blazar.chats.{ws}")
}

impl Chat {
    pub async fn load_threads(self) {
        let ws = self.ws_id();
        if let Ok(t) = api::get::<Vec<Thread>>(&format!("/api/workspaces/{ws}/sessions")).await {
            let _ = self.threads.try_set(t);
        }
    }

    pub fn thread_title(self, id: Option<&str>) -> String {
        match id {
            None => "Untitled".to_owned(),
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

    pub(super) fn save_tabs(self) {
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
        clear_view_pending(&mut self.pending.write(), &mut self.orphans.write_value());
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
        self.flush_rows();
        let (changed, request) = self.history_requests.write_value().begin(view.clone());
        if changed {
            clear_view_pending(&mut self.pending.write(), &mut self.orphans.write_value());
            self.sessions.set_value(view.iter().cloned().collect());
            self.seen.set_value(HashSet::new());
            self.rows.set(Vec::new());
        }
        let baseline: HashSet<RowKey> = self.rows.with_untracked(|rows| {
            rows.iter()
                .map(|row| (row.session_id.clone(), row.seq))
                .collect()
        });
        self.decided.update(ApprovalDeliveries::clear);
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

    pub(super) fn refresh_threads_soon(self) {
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
