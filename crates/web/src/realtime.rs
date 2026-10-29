use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

use blazar_core_types::api::{EventFrame, ServerEvent};
use futures::channel::oneshot;
use futures::future::{Either, select};
use futures::{Sink, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::{Message, State};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;

use crate::link_state::{
    BANNER_GRACE_MS, HEARTBEAT_EVERY_MS, HEARTBEAT_TIMEOUT_MS, Heartbeat, banner_text,
};

type Listener = Rc<dyn Fn(&ServerEvent)>;

#[derive(Clone, Copy)]
pub struct Bus {
    pub connected: RwSignal<bool>,
    pub reconnects: RwSignal<u32>,
    pub resyncs: RwSignal<u32>,
    last_event: StoredValue<Option<i64>>,
    listeners: StoredValue<Vec<(u32, Listener)>, LocalStorage>,
    next_listener: StoredValue<u32>,
    pub accounts: RwSignal<u32>,
    pub workspaces: RwSignal<u32>,
    pub nodes: RwSignal<u32>,
    workspaces_bump_pending: StoredValue<bool>,
    /// 叫醒正在等的连接（或退避中的重连），让它马上断开重连。
    kick: StoredValue<Option<oneshot::Sender<()>>, LocalStorage>,
    /// 下一次连上时把所有东西重新拉一遍（切回前台、联网、心跳超时之后）。
    refetch: StoredValue<bool>,
    /// 桌面端说和远程 hub 的连接断了：机器名。
    pub desktop_down: RwSignal<Option<String>>,
    /// 断开超过一小会儿：正文顶部显示“正在重新连接…”。
    pub banner: RwSignal<bool>,
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
                bump(self.resyncs);
                bump(self.reconnects);
            }
            ServerEvent::AccountsChanged => bump(self.accounts),
            ServerEvent::WorkspacesChanged | ServerEvent::SessionTitled { .. } => {
                self.bump_workspaces_soon();
            }
            ServerEvent::NodesChanged => bump(self.nodes),
            ServerEvent::UiCommand { id, command } => crate::ui_driver::handle(*id, command),
            _ => {}
        }
    }

    /// 不管现在看起来连没连着，都断开重连，连上后重新拉取。
    pub fn force_reconnect(self) {
        let _ = self.refetch.try_set_value(true);
        if let Some(Some(tx)) = self.kick.try_update_value(Option::take) {
            let _ = tx.send(());
        }
    }

    fn arm(self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.kick.set_value(Some(tx));
        rx
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

    fn receive(self, frame: &EventFrame) {
        if let Some(id) = frame.event_id {
            self.last_event
                .update_value(|last| *last = Some(last.map_or(id, |l| l.max(id))));
        }
        self.dispatch(&frame.event);
    }

    async fn cursor(self) -> Option<i64> {
        if let Some(id) = self.last_event.get_value() {
            return Some(id);
        }
        let head = crate::api::get::<serde_json::Value>("/api/events/head")
            .await
            .ok()?["id"]
            .as_i64()?;
        self.last_event.set_value(Some(head));
        Some(head)
    }
}

fn ws_url(since: Option<i64>) -> String {
    let url = crate::api::websocket_url("/api/ws");
    match since {
        Some(id) => format!("{url}?since={id}"),
        None => url,
    }
}

pub fn provide() -> Bus {
    let bus = Bus {
        connected: RwSignal::new(false),
        reconnects: RwSignal::new(0),
        resyncs: RwSignal::new(0),
        last_event: StoredValue::new(None),
        listeners: StoredValue::new_local(Vec::new()),
        next_listener: StoredValue::new(0),
        accounts: RwSignal::new(0),
        workspaces: RwSignal::new(0),
        nodes: RwSignal::new(0),
        workspaces_bump_pending: StoredValue::new(false),
        kick: StoredValue::new_local(None),
        refetch: StoredValue::new(false),
        desktop_down: RwSignal::new(None),
        banner: RwSignal::new(false),
    };
    provide_context(bus);
    spawn_local(run(bus));
    spawn_local(heartbeat(bus));
    wake_on_return(bus);
    listen_to_desktop(bus);
    show_banner_when_down(bus);
    bus
}

/// 切回前台、重新联网：别信“看起来连着”，强制重连并重新拉取。
fn wake_on_return(bus: Bus) {
    let hidden_at = StoredValue::new(None::<f64>);
    let visibility = window_event_listener(leptos::ev::visibilitychange, move |_| {
        if document().hidden() {
            hidden_at.set_value(Some(js_sys::Date::now()));
        } else if hidden_at.get_value().is_some() {
            hidden_at.set_value(None);
            bus.force_reconnect();
        }
    });
    let online = window_event_listener_untyped("online", move |_| bus.force_reconnect());
    // App 活多久它们就活多久。
    std::mem::forget(visibility);
    std::mem::forget(online);
}

/// 桌面端连远程 hub 时，用 `window.__blazarHubLink(up, machine)` 告诉页面连接断了 / 回来了。
fn listen_to_desktop(bus: Bus) {
    let hook =
        Closure::<dyn Fn(bool, Option<String>)>::new(move |up: bool, machine: Option<String>| {
            if up {
                let _ = bus.desktop_down.try_set(None);
                bus.force_reconnect();
            } else {
                let _ = bus.desktop_down.try_set(Some(machine.unwrap_or_default()));
            }
        });
    let _ = js_sys::Reflect::set(&window(), &"__blazarHubLink".into(), hook.as_ref());
    hook.forget();
}

fn show_banner_when_down(bus: Bus) {
    let seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        let down = !bus.connected.get() || bus.desktop_down.with(Option::is_some);
        seq.update_value(|n| *n = n.wrapping_add(1));
        if !down {
            bus.banner.set(false);
            return;
        }
        let mine = seq.get_value();
        gloo_timers::callback::Timeout::new(BANNER_GRACE_MS, move || {
            if seq.try_get_value() == Some(mine) {
                let _ = bus.banner.try_set(true);
            }
        })
        .forget();
    });
}

/// 每 15 秒问一次 hub；连着两次没回应，就当实时连接已经停了（睡眠唤醒、SSH 隧道死了），重连。
async fn heartbeat(bus: Bus) {
    let mut hb = Heartbeat::default();
    loop {
        gloo_timers::future::TimeoutFuture::new(HEARTBEAT_EVERY_MS).await;
        if !bus.connected.get_untracked() {
            hb.reset();
            continue;
        }
        let ping = crate::api::get::<serde_json::Value>("/api/events/head");
        let timeout = gloo_timers::future::TimeoutFuture::new(HEARTBEAT_TIMEOUT_MS);
        futures::pin_mut!(ping);
        let answered = match select(ping, timeout).await {
            // hub 回了话（哪怕是错误码）就算活着；只有请求根本没到才算没回应。
            Either::Left((r, _)) => r.map_or_else(|e| e.status.is_some(), |_| true),
            Either::Right(_) => false,
        };
        if hb.beat(answered) {
            crate::client_log::note("realtime", "heartbeat missed twice, reconnecting");
            bus.force_reconnect();
        }
    }
}

/// 横条的字：桌面端告诉了机器名就带上。
pub fn banner_line(bus: Bus) -> String {
    banner_text(bus.desktop_down.get().as_deref())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// 一直没打开。
    NeverOpened,
    /// 开过，后来断了。
    Closed,
    /// 被叫醒，主动断开。
    Kicked,
}

async fn run(bus: Bus) {
    let mut backoff = 1u64;
    let mut first = true;
    loop {
        let mut kick = bus.arm();
        let mut ended = Ended::NeverOpened;
        if crate::api::session().await.is_ok() {
            let since = bus.cursor().await;
            if let Ok(ws) = WebSocket::open(&ws_url(since)) {
                ended = listen(bus, ws, since, first, &mut kick).await;
                if ended != Ended::NeverOpened {
                    backoff = 1;
                    first = false;
                }
            }
        }
        bus.connected.set(false);
        crate::api::reset_session();
        if ended == Ended::Kicked {
            continue;
        }
        let kick = bus.arm();
        let _ = select(
            gloo_timers::future::sleep(Duration::from_secs(backoff)),
            kick,
        )
        .await;
        backoff = (backoff * 2).min(15);
    }
}

async fn listen(
    bus: Bus,
    mut ws: WebSocket,
    since: Option<i64>,
    first: bool,
    kick: &mut oneshot::Receiver<()>,
) -> Ended {
    let ready = futures::future::poll_fn(|cx| Pin::new(&mut ws).poll_ready(cx));
    let opening = gloo_timers::future::sleep(Duration::from_secs(10));
    futures::pin_mut!(ready);
    match select(select(ready, opening), &mut *kick).await {
        Either::Left(_) => {}
        Either::Right(_) => return Ended::Kicked,
    }
    if !matches!(ws.state(), State::Open) {
        return Ended::NeverOpened;
    }
    bus.connected.set(true);
    let refetch = bus
        .refetch
        .try_update_value(std::mem::take)
        .unwrap_or(false);
    if !first {
        if since.is_none() || refetch {
            bus.bump_all();
            bump(bus.resyncs);
        }
        bump(bus.reconnects);
    }
    let (_tx, mut rx) = ws.split();
    loop {
        let next = rx.next();
        match select(next, &mut *kick).await {
            Either::Left((Some(Ok(msg)), _)) => {
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                };
                if let Ok(frame) = serde_json::from_str::<EventFrame>(&text) {
                    bus.receive(&frame);
                }
            }
            Either::Left(_) => return Ended::Closed,
            Either::Right(_) => return Ended::Kicked,
        }
    }
}

pub fn use_bus() -> Bus {
    expect_context::<Bus>()
}
