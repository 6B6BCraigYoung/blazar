use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::chat_model;
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;

use super::delivery::steer_request;
use super::{Chat, Queued};

impl Chat {
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
                Ok(r) if r["interrupted"].as_bool() == Some(true) => toast("Stopped"),
                Ok(r) => toast(r["reason"].as_str().unwrap_or("Couldn't stop").to_owned()),
                Err(e) => toast(format!("Stop failed: {e}")),
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
                    toast(format!("{what} applied to the current turn"))
                }
                Ok(r) => toast(format!(
                    "{what} takes effect next turn ({})",
                    r["reason"].as_str().unwrap_or("")
                )),
                Err(_) => toast(format!("{what} takes effect next turn")),
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
        if !self.decided.write().begin(&id) {
            return;
        }
        self.spawn(async move {
            let mut body = json!({ "allow": allow, "message": message });
            if let Some(a) = answers {
                body["answers"] = a;
            }
            let result = api::send::<Value>("POST", &format!("/api/approvals/{id}"), &body).await;
            let delivered = result
                .as_ref()
                .is_ok_and(|result| result["delivered"].as_bool() == Some(true));
            let _ = self
                .decided
                .try_update(|decisions| decisions.finish(&id, delivered));
            match result {
                Ok(_) if delivered => {}
                Ok(result) => toast(
                    result["reason"]
                        .as_str()
                        .unwrap_or("The decision was not delivered, please retry")
                        .to_owned(),
                ),
                Err(error) => toast(format!("Send failed, input kept: {error}")),
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
                &format!(
                    "Always allow \"{what}\" in this workspace?\n\nYou can turn this off in Settings."
                ),
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
                toast("This message has settings that can't be edited here; it stays queued");
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
                            toast("No running session to add to");
                            return Ok(());
                        };
                        let Some(request) = q.request.as_ref() else {
                            toast("This message's settings can't be sent mid-turn; it stays queued");
                            return Ok(());
                        };
                        if dialog::ask("Send now?", "Attachments and file context are sent too. Model, account and permissions follow the current turn.", vec![Choice::plain("Cancel"), Choice::plain("Send now")]).await != Some(1) {
                            return Ok(());
                        }
                        if !self.context_is_current(&context) {
                            toast("Conversation changed; the message stays queued");
                            return Ok(());
                        }
                        let i = api::send::<Value>("POST", &format!("/api/sessions/{sid}/input"), &steer_request(request)).await?;
                        if i["accepted"].as_bool() != Some(true) {
                            toast(i["reason"].as_str().unwrap_or("This runtime can't take messages mid-turn; it will send after this turn.").to_owned());
                            return Ok(());
                        }
                        api::send::<Value>("DELETE", &base, &json!({"expected": request})).await?;
                        toast("Added to the current turn");
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
            toast("Stop the current turn before retrying.");
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
                "Edit and retry"
            };
            let tail = if later > 0 {
                format!(" and the {later} turns after it")
            } else {
                String::new()
            };
            let body = format!(
                "· Files are restored to before this message (current state is backed up)\n· This message{tail} leave the context\n· Non-Git folders have no checkpoints; only the conversation restarts"
            );
            if dialog::ask(
                &format!("{head}?"),
                &body,
                vec![Choice::plain("Cancel"), Choice::danger("Retry")],
            )
            .await
                != Some(1)
            {
                return;
            }
            if !self.context_is_current(&context) {
                toast("Conversation changed; nothing was resent");
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
                    toast(r["reason"].as_str().unwrap_or("The workspace is busy with another operation").to_owned())
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
                        "Continuing from here; no checkpoint, files unchanged."
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
                && dialog::ask(
                    "Rewind code",
                    "Restore files to before this message?\nThe conversation is kept and the current state is backed up, so you can undo.",
                    vec![Choice::plain("Cancel"), Choice::plain("Rewind code")],
                )
                .await
                    != Some(1)
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
                            "Files are back to before this message. Undo the rewind?",
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
}
