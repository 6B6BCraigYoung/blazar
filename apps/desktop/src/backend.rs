use std::sync::Arc;
use std::time::Duration;

use blazar_hub::accounts::{self, MenuChoice, MenuProvider};
use blazar_hub::state::{AppState, EventFrame, ServerEvent};
use reqwest::Method;
use serde_json::{Value, json};
use tokio::sync::{Notify, broadcast};

use crate::remote::RemoteHub;

pub const REMOTE_INVITE: &str = "远程模式下，邀请要在 hub 所在的机器上导入";

pub enum Hub {
    Embedded { state: Arc<AppState>, url: String },
    Remote(Arc<RemoteHub>),
}

pub struct Watch {
    bus: Option<broadcast::Receiver<EventFrame>>,
    wanted: fn(&ServerEvent) -> bool,
    kick: Option<Arc<Notify>>,
    every: Option<Duration>,
}

impl Watch {
    pub async fn next(&mut self) -> bool {
        let every = self.every;
        let tick = async move {
            match every {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        };
        let kick = self.kick.clone();
        let kicked = async move {
            match kick {
                Some(k) => k.notified().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(tick, kicked);
        let Some(bus) = self.bus.as_mut() else {
            tokio::select! {
                () = &mut tick => {}
                () = &mut kicked => {}
            }
            return true;
        };
        loop {
            tokio::select! {
                () = &mut tick => return true,
                () = &mut kicked => return true,
                event = bus.recv() => match event {
                    Ok(e) if (self.wanted)(&e.event) => return true,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return true,
                    Err(broadcast::error::RecvError::Closed) => return false,
                },
            }
        }
    }
}

impl Hub {
    pub fn remote(&self) -> Option<&Arc<RemoteHub>> {
        match self {
            Self::Embedded { .. } => None,
            Self::Remote(r) => Some(r),
        }
    }

    pub fn url(&self) -> &str {
        match self {
            Self::Embedded { url, .. } => url,
            Self::Remote(r) => &r.base,
        }
    }

    pub fn watch_accounts(&self) -> Watch {
        match self {
            Self::Embedded { state, .. } => Watch {
                bus: Some(state.bus.subscribe()),
                wanted: |e| matches!(e, ServerEvent::AccountsChanged),
                kick: None,
                every: None,
            },
            Self::Remote(r) => Watch {
                bus: None,
                wanted: |_| false,
                kick: Some(r.accounts_kick.clone()),
                every: Some(Duration::from_secs(60)),
            },
        }
    }

    pub fn watch_inbox(&self) -> Watch {
        match self {
            Self::Embedded { state, .. } => Watch {
                bus: Some(state.bus.subscribe()),
                wanted: |e| matches!(e, ServerEvent::InboxChanged),
                kick: None,
                every: Some(Duration::from_secs(60)),
            },
            Self::Remote(r) => Watch {
                bus: None,
                wanted: |_| false,
                kick: Some(r.inbox_kick.clone()),
                every: Some(Duration::from_secs(20)),
            },
        }
    }

    pub fn inbox_maybe_changed(&self) {
        if let Self::Remote(r) = self {
            r.inbox_kick.notify_one();
        }
    }

    pub fn accounts_changed(&self) {
        match self {
            Self::Embedded { state, .. } => state.emit(ServerEvent::AccountsChanged),
            Self::Remote(r) => r.accounts_kick.notify_one(),
        }
    }

    pub async fn unread(&self) -> Result<i64, String> {
        match self {
            Self::Embedded { state, .. } => Ok(blazar_hub::inbox::unread(state).await),
            Self::Remote(r) => {
                let v = r.request(Method::GET, "/api/inbox/count", None).await?;
                v["unread"]
                    .as_i64()
                    .ok_or_else(|| "hub 回复里没有未读数".to_owned())
            }
        }
    }

    pub async fn account_menu(&self) -> Result<(bool, Vec<MenuProvider>), String> {
        match self {
            Self::Embedded { state, .. } => Ok(accounts::menu_state(state).await),
            Self::Remote(r) => {
                let list = r.request(Method::GET, "/api/accounts", None).await?;
                let global = r.request(Method::GET, "/api/accounts/global", None).await?;
                Ok((
                    global["enabled"].as_bool() == Some(true),
                    menu_from_list(&list),
                ))
            }
        }
    }

    pub async fn choose(&self, provider: &str, mode: &str) -> Result<(), String> {
        match self {
            Self::Embedded { state, .. } => accounts::choose(state, provider, mode).await,
            Self::Remote(r) => r
                .request(
                    Method::PUT,
                    "/api/accounts/mode",
                    Some(&json!({ "provider": provider, "mode": mode })),
                )
                .await
                .map(drop),
        }
    }

    pub async fn set_global(&self, enabled: bool) -> Result<(), String> {
        match self {
            Self::Embedded { state, .. } => accounts::set_global(state, enabled).await,
            Self::Remote(r) => r
                .request(
                    Method::PUT,
                    "/api/accounts/global",
                    Some(&json!({ "enabled": enabled })),
                )
                .await
                .map(drop),
        }
    }

    pub fn accept_invite(&self, text: String) -> Result<(), &'static str> {
        match self {
            Self::Embedded { state, .. } => {
                state.mesh_ctx.set_pending_invite(text);
                Ok(())
            }
            Self::Remote(_) => Err(REMOTE_INVITE),
        }
    }

    pub fn shutdown(&self) {
        if let Self::Remote(r) = self {
            r.shutdown();
        }
    }
}

pub fn menu_from_list(list: &Value) -> Vec<MenuProvider> {
    let empty = Vec::new();
    let all = list["accounts"].as_array().unwrap_or(&empty);
    accounts::PROVIDERS
        .iter()
        .map(|provider| {
            let mut choices = vec![MenuChoice {
                mode: "auto".into(),
                label: "自动轮换".into(),
            }];
            for a in all
                .iter()
                .filter(|a| a["provider"].as_str() == Some(provider))
                .filter(|a| a["disabled"].as_bool() != Some(true))
            {
                let label = a["label"].as_str().unwrap_or_default();
                let label = match a["email"].as_str().filter(|e| !e.is_empty() && *e != label) {
                    Some(email) => format!("{label} · {email}"),
                    None => label.to_owned(),
                };
                choices.push(MenuChoice {
                    mode: if a["builtin"].as_bool() == Some(true) {
                        String::new()
                    } else {
                        a["id"].as_str().unwrap_or_default().to_owned()
                    },
                    label,
                });
            }
            if !choices.iter().any(|c| c.mode.is_empty()) {
                choices.insert(
                    1,
                    MenuChoice {
                        mode: String::new(),
                        label: "本机登录".into(),
                    },
                );
            }
            MenuProvider {
                provider,
                current: list["modes"][provider]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                choices,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_account_list_becomes_the_same_menu() {
        let list = json!({
            "accounts": [
                {"id": "claude-default", "provider": "claude", "label": "本机", "email": "me@x.com", "builtin": true, "disabled": false},
                {"id": "a1", "provider": "claude", "label": "工作", "email": "工作", "builtin": false, "disabled": false},
                {"id": "a2", "provider": "claude", "label": "停用", "builtin": false, "disabled": true},
                {"id": "c1", "provider": "codex", "label": "备用", "email": null, "builtin": false, "disabled": false}
            ],
            "modes": {"claude": "a1", "codex": "auto"}
        });
        let menu = menu_from_list(&list);
        assert_eq!(menu.len(), 2);
        assert_eq!(menu[0].provider, "claude");
        assert_eq!(menu[0].current, "a1");
        assert_eq!(
            menu[0]
                .choices
                .iter()
                .map(|c| (c.mode.as_str(), c.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("auto", "自动轮换"),
                ("", "本机 · me@x.com"),
                ("a1", "工作")
            ]
        );
        assert_eq!(
            menu[1]
                .choices
                .iter()
                .map(|c| (c.mode.as_str(), c.label.as_str()))
                .collect::<Vec<_>>(),
            [("auto", "自动轮换"), ("", "本机登录"), ("c1", "备用")]
        );
        assert_eq!(menu[1].current, "auto");
    }
}
