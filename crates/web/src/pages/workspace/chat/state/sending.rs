use std::collections::HashMap;

use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::chat_model::Row;
use crate::components::toast::toast;
use crate::storage;

use super::delivery::{steer_request, take_composer};
use super::send_context::SendContext;
use super::{ACC_RUNTIMES, CONTINUE_TEXT, Chat, Comment, PendingMsg, Queued, ShellRun};

const PROMPT_HISTORY_MAX: usize = 200;

impl Chat {
    pub(super) fn send_context(self, thread: Option<String>) -> SendContext {
        SendContext {
            workspace: self.ws_id(),
            thread,
            revision: self.view_revision.get_value(),
        }
    }

    pub(super) fn context_is_current(self, context: &SendContext) -> bool {
        self.view.try_get_untracked().is_some_and(|thread| {
            context.matches(
                &self.ws_id(),
                thread.as_deref(),
                self.view_revision.try_get_value().unwrap_or_default(),
            )
        })
    }

    pub(super) fn note_sent_in(
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
        if let Some(t) = thread_id.as_ref().or(view.as_ref()) {
            self.set_thread_status(t, "running");
        }
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

    pub(super) fn push_row(self, row: Row) {
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
        self.row_buffer.update_value(|b| b.push(row));
        if self.flush_scheduled.get_value() {
            return;
        }
        self.flush_scheduled.set_value(true);
        request_animation_frame(move || {
            let _ = self.flush_scheduled.try_set_value(false);
            self.flush_rows();
        });
    }

    pub(super) fn flush_rows(self) {
        let Some(buffered) = self.row_buffer.try_update_value(std::mem::take) else {
            return;
        };
        if buffered.is_empty() {
            return;
        }
        let _ = self.rows.try_update(|r| r.extend(buffered));
    }

    pub(super) fn adopt(self, sid: &str) {
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

    pub(super) fn matches_pending(self, text: &str) -> bool {
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

    pub(super) fn send_options(self) -> serde_json::Map<String, Value> {
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
            .filter(|a| !a.is_empty())
            .filter(|a| {
                a == "auto" || untrack(|| self.account(a)).is_none_or(|x| x.provider == rt)
            });
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
                    "{}. `{}` line {}{}\n   > {code}\n   {}",
                    i + 1,
                    c.file,
                    c.line,
                    if c.side == "old" {
                        " (deleted line)"
                    } else {
                        ""
                    },
                    c.text.replace('\n', "\n   ")
                )
            })
            .collect();
        format!(
            "\n\n---\nReview comments ({}). Please address each one:\n\n{}",
            list.len(),
            items.join("\n\n")
        )
    }

    pub fn clear_review(self) {
        self.diff.clear_comments();
    }

    pub fn prompt_history(self) -> Vec<String> {
        storage::load(&format!("blazar.history.{}", self.ws_id())).unwrap_or_default()
    }

    fn remember_prompt(self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let mut history = self.prompt_history();
        history.retain(|h| h != text);
        history.push(text.to_owned());
        let excess = history.len().saturating_sub(PROMPT_HISTORY_MAX);
        history.drain(..excess);
        storage::save(&format!("blazar.history.{}", self.ws_id()), &history);
    }

    pub fn run_shell(self, command: String) {
        if self.shell_busy.get_untracked() {
            return;
        }
        let Some(ws) = self.ws.try_get_value() else {
            return;
        };
        self.remember_prompt(&format!("!{command}"));
        self.prompt.set(String::new());
        self.shell_busy.set(true);
        self.spawn(async move {
            match api::send::<ShellRun>(
                "POST",
                &format!("/api/workspaces/{ws}/shell"),
                &json!({ "command": command }),
            )
            .await
            {
                Ok(run) => {
                    let _ = self.shells.try_update(|s| s.push(run));
                }
                Err(e) => toast(format!("Command failed: {e}")),
            }
            let _ = self.shell_busy.try_set(false);
        });
    }

    pub fn send(self) {
        if self.attach.with_untracked(Vec::is_empty)
            && let Some(command) = self
                .prompt
                .with_untracked(|p| p.trim().strip_prefix('!').map(|c| c.trim().to_owned()))
                .filter(|c| !c.is_empty())
        {
            self.run_shell(command);
            return;
        }
        let reviews = self.diff.comments.get_untracked();
        if self.prompt.with_untracked(|p| p.trim().is_empty())
            && self.attach.with_untracked(Vec::is_empty)
            && reviews.is_empty()
        {
            return;
        }
        let (text, images) = take_composer(&mut self.prompt.write(), &mut self.attach.write());
        self.remember_prompt(&text);
        let text = if text.trim().is_empty() {
            if reviews.is_empty() {
                "Take a look at this image"
            } else {
                "Please address the review comments below."
            }
            .to_owned()
        } else {
            text.trim().to_owned()
        };
        let shells = self.shells.get_untracked();
        let shell_block: String = shells
            .iter()
            .map(|r| format!("{}\n\n", r.context()))
            .collect();
        if !shells.is_empty() {
            self.shells.set(Vec::new());
        }
        let wire = format!("{shell_block}{text}{}", Self::review_block(&reviews));
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

    pub fn send_text(self, text: String) {
        let mut body = self.send_options();
        body.insert("text".into(), json!(text));
        body.insert("resume_session".into(), json!(self.view.get_untracked()));
        body.insert("images".into(), json!([]));
        body.insert("wait_secs".into(), json!(20));
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
                    let running = r["running_session_id"].as_str().map(str::to_owned);
                    let same_turn = r["running_interactive"] == json!(true)
                        && body["resume"] != json!(false)
                        && running.is_some()
                        && self.transcript.with_untracked(|t| t.last_session.clone()) == running
                        && self.context_is_current(&context);
                    if let (true, Some(sid)) = (same_turn, running) {
                        let i = api::send::<Value>(
                            "POST",
                            &format!("/api/sessions/{sid}/input"),
                            &steer_request(&body),
                        )
                        .await;
                        if let Ok(i) = &i
                            && i["accepted"] == json!(true)
                        {
                            self.drop_pending(pid);
                            self.deliveries.update(|d| d.finish(pid, true));
                            return;
                        }
                    }
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
                            toast("Queued. Sends after this turn.");
                        }
                        Err(e) => toast(format!("Couldn't queue, message kept: {e}")),
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
                            "Agent failed to start".to_owned()
                        } else {
                            why.clone()
                        });
                        let logged = r["activity"]["failed"] == json!(true);
                        if !logged && self.context_is_current(&context) {
                            self.local_errors
                                .update(|e| e.push(format!("Agent failed to start: {why}")));
                        }
                    }
                }
                Err(e) => {
                    self.drop_pending(pid);
                    self.deliveries.update(|d| d.finish(pid, false));
                    toast(format!("Send failed, message kept: {e}"));
                }
            }
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            self.drop_pending(pid);
        });
    }

    pub fn continue_thread(self) {
        let mut body = self.send_options();
        body.insert("text".into(), json!(crate::failure::CONTINUE));
        body.insert("resume".into(), json!(true));
        body.insert("resume_session".into(), json!(self.view.get_untracked()));
        body.insert("wait_secs".into(), json!(20));
        let context = self.send_context(self.view.get_untracked());
        let ws = context.workspace.clone();
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
                        .unwrap_or("The workspace is busy with another operation")
                        .to_owned(),
                ),
                Ok(r) => self.note_sent_in(
                    &context,
                    r["session_id"].as_str().map(str::to_owned),
                    r["thread_id"].as_str().map(str::to_owned),
                ),
                Err(e) => toast(format!("Couldn't continue: {e}")),
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
                        .unwrap_or("The workspace is busy with another operation")
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
}
