//! 跟 hub 的实时连接（`/api/ws`）。断了就按 1、2、4…最多 15 秒的间隔重连；连回来时所有话题都算变过，页面各自重新拉一次。
//!
//! 页面不直接处理事件：每个话题是一个计数器，页面在取数的地方读它，计数一变就自动重新取。

use std::pin::Pin;
use std::time::Duration;

use blazar_core_types::api::ServerEvent;
use futures::{Sink, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::{Message, State};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[derive(Clone, Copy)]
pub struct Bus {
    pub connected: RwSignal<bool>,
    pub accounts: RwSignal<u32>,
    pub workspaces: RwSignal<u32>,
    pub nodes: RwSignal<u32>,
}

fn bump(s: RwSignal<u32>) {
    s.update(|n| *n = n.wrapping_add(1));
}

impl Bus {
    fn dispatch(self, ev: &ServerEvent) {
        match ev {
            ServerEvent::AccountsChanged => bump(self.accounts),
            ServerEvent::WorkspacesChanged | ServerEvent::SessionTitled { .. } => {
                bump(self.workspaces);
            }
            ServerEvent::NodesChanged => bump(self.nodes),
            _ => {}
        }
    }

    fn bump_all(self) {
        bump(self.accounts);
        bump(self.workspaces);
        bump(self.nodes);
    }
}

fn ws_url() -> String {
    let loc = window().location();
    let scheme = if loc.protocol().as_deref() == Ok("https:") {
        "wss"
    } else {
        "ws"
    };
    format!("{scheme}://{}/api/ws", loc.host().unwrap_or_default())
}

/// 建总线、放进上下文、起后台连接。只在根组件里调一次。
pub fn provide() -> Bus {
    let bus = Bus {
        connected: RwSignal::new(false),
        accounts: RwSignal::new(0),
        workspaces: RwSignal::new(0),
        nodes: RwSignal::new(0),
    };
    provide_context(bus);
    spawn_local(run(bus));
    bus
}

async fn run(bus: Bus) {
    let mut backoff = 1u64;
    let mut first = true;
    loop {
        if let Ok(mut ws) = WebSocket::open(&ws_url()) {
            // 等握手结束（不再是 CONNECTING）；结束后不是 OPEN 就是连不上，直接走重连。
            let _ = futures::future::poll_fn(|cx| Pin::new(&mut ws).poll_ready(cx)).await;
            if matches!(ws.state(), State::Open) {
                backoff = 1;
                bus.connected.set(true);
                // 断开期间的事件收不到了：重连上就当所有话题都变过。
                if !first {
                    bus.bump_all();
                }
                first = false;
                let (_tx, mut rx) = ws.split();
                while let Some(Ok(msg)) = rx.next().await {
                    let text = match msg {
                        Message::Text(t) => t,
                        Message::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                    };
                    // 新版 hub 可能多出这里还不认识的事件，认不出就跳过。
                    if let Ok(ev) = serde_json::from_str::<ServerEvent>(&text) {
                        bus.dispatch(&ev);
                    }
                }
            }
        }
        bus.connected.set(false);
        gloo_timers::future::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(15);
    }
}

pub fn use_bus() -> Bus {
    expect_context::<Bus>()
}
