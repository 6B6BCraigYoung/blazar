use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api::{self, Account, Accounts};
use crate::storage;

use super::{
    AgentInfo, Catalog, Chat, Mode, ModelInfo, ModelSel, Prefs, Profile, Snippet, default_mode,
    modes_for,
};

impl Chat {
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
            .unwrap_or_else(|| "Choose agent".into())
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

    pub(super) fn acc_key(self, thread: Option<&str>) -> String {
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
}
