use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use blazar_core_types::api::ServerEvent;
use futures::{Sink, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::{Message, State};
use leptos::prelude::*;
use leptos::task::spawn_local;

type Listener = Rc<dyn Fn(&ServerEvent)>;

#[derive(Clone, Copy)]
pub struct Bus {
    pub connected: RwSignal<bool>,
    pub reconnects: RwSignal<u32>,
    listeners: StoredValue<Vec<(u32, Listener)>, LocalStorage>,
    next_listener: StoredValue<u32>,
    pub accounts: RwSignal<u32>,
    pub workspaces: RwSignal<u32>,
    pub nodes: RwSignal<u32>,
    workspaces_bump_pending: StoredValue<bool>,
}

const WORKSPACES_BUMP_DELAY_MS: u32 = 400;

fn bump(s: RwSignal<u32>) {
    s.update(|n| *n = n.wrapping_add(1));
}

impl Bus {
    pub fn subscribe(self, f: impl Fn(&ServerEvent) + 'static) -> u32 {
        let id = self.next_listener.get_value();
        self.next_listener.set_value(id.wrapping_add(1));
        self.listeners.update_value(|l| l.push((id, Rc::new(f))));
        id
    }

    pub fn unsubscribe(self, id: u32) {
        self.listeners
            .try_update_value(|l| l.retain(|(i, _)| *i != id));
    }

    fn dispatch(self, ev: &ServerEvent) {
        let ls: Vec<Listener> = self
            .listeners
            .with_value(|l| l.iter().map(|(_, f)| f.clone()).collect());
        for f in ls {
            f(ev);
        }
        match ev {
            ServerEvent::ResyncRequired => {
                self.bump_all();
                bump(self.reconnects);
            }
            ServerEvent::AccountsChanged => bump(self.accounts),
            ServerEvent::WorkspacesChanged | ServerEvent::SessionTitled { .. } => {
                self.bump_workspaces_soon();
            }
            ServerEvent::NodesChanged => bump(self.nodes),
            _ => {}
        }
    }

    fn bump_workspaces_soon(self) {
        if self.workspaces_bump_pending.get_value() {
            return;
        }
        self.workspaces_bump_pending.set_value(true);
        gloo_timers::callback::Timeout::new(WORKSPACES_BUMP_DELAY_MS, move || {
            let _ = self.workspaces_bump_pending.try_set_value(false);
            let _ = self.workspaces.try_update(|n| *n = n.wrapping_add(1));
        })
        .forget();
    }

    fn bump_all(self) {
        bump(self.accounts);
        bump(self.workspaces);
        bump(self.nodes);
    }
}

fn ws_url() -> String {
    crate::api::websocket_url("/api/ws")
}

pub fn provide() -> Bus {
    let bus = Bus {
        connected: RwSignal::new(false),
        reconnects: RwSignal::new(0),
        listeners: StoredValue::new_local(Vec::new()),
        next_listener: StoredValue::new(0),
        accounts: RwSignal::new(0),
        workspaces: RwSignal::new(0),
        nodes: RwSignal::new(0),
        workspaces_bump_pending: StoredValue::new(false),
    };
    provide_context(bus);
    spawn_local(run(bus));
    bus
}

async fn run(bus: Bus) {
    let mut backoff = 1u64;
    let mut first = true;
    loop {
        if crate::api::session().await.is_ok()
            && let Ok(mut ws) = WebSocket::open(&ws_url())
        {
            let _ = futures::future::poll_fn(|cx| Pin::new(&mut ws).poll_ready(cx)).await;
            if matches!(ws.state(), State::Open) {
                backoff = 1;
                bus.connected.set(true);
                if !first {
                    bus.bump_all();
                    bus.reconnects.update(|n| *n = n.wrapping_add(1));
                }
                first = false;
                let (_tx, mut rx) = ws.split();
                while let Some(Ok(msg)) = rx.next().await {
                    let text = match msg {
                        Message::Text(t) => t,
                        Message::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                    };
                    if let Ok(ev) = serde_json::from_str::<ServerEvent>(&text) {
                        bus.dispatch(&ev);
                    }
                }
            }
        }
        bus.connected.set(false);
        crate::api::reset_session();
        gloo_timers::future::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(15);
    }
}

pub fn use_bus() -> Bus {
    expect_context::<Bus>()
}
