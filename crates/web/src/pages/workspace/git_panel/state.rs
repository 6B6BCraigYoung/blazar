use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{self, GitOpResult, GitStatus};
use crate::components::toast::toast;
use crate::realtime::use_bus;

use super::Git;

impl Git {
    pub fn new(ws: &str) -> Self {
        Self {
            status: RwSignal::new(None),
            load_error: RwSignal::new(None),
            busy: RwSignal::new(None),
            out: RwSignal::new(None),
            pr: RwSignal::new(None),
            reload: RwSignal::new(0),
            changed: RwSignal::new(0),
            ws: StoredValue::new(ws.to_owned()),
        }
    }

    pub fn keep_loaded(self) {
        let bus = use_bus();
        Effect::new(move |_| {
            bus.workspaces.track();
            self.reload.track();
            let ws = self.ws.get_value();
            spawn_local(async move {
                let s = match api::get::<GitStatus>(&format!("/api/workspaces/{ws}/git")).await {
                    Ok(status) => {
                        let _ = self.load_error.try_set(None);
                        status
                    }
                    Err(error) => {
                        let _ = self.load_error.try_set(Some(error.to_string()));
                        GitStatus {
                            reason: error.to_string(),
                            ..GitStatus::default()
                        }
                    }
                };
                let _ = self.status.try_set(Some(s));
            });
        });
    }

    pub(super) async fn op(self, op: &'static str, body: serde_json::Value) -> Option<GitOpResult> {
        if self.busy.get_untracked().is_some() {
            return None;
        }
        self.busy.set(Some(op));
        let ws = self.ws.get_value();
        let r = api::send::<GitOpResult>("POST", &format!("/api/workspaces/{ws}/git/{op}"), &body)
            .await;
        let _ = self.busy.try_set(None);
        if matches!(op, "rebase" | "continue" | "abort" | "commit" | "merge") {
            let _ = self.changed.try_update(|n| *n = n.wrapping_add(1));
        }
        match r {
            Ok(r) => {
                if let Some(s) = r.status.clone() {
                    let _ = self.status.try_set(Some(s));
                }
                let _ = self
                    .out
                    .try_set(Some((op.to_owned(), r.ok, r.output.clone())));
                Some(r)
            }
            Err(e) => {
                let _ = self
                    .out
                    .try_set(Some((op.to_owned(), false, e.to_string())));
                toast(e.to_string());
                None
            }
        }
    }
}
