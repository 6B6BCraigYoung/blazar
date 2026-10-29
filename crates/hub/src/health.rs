//! 机器在线状态：定期探测所有远端机器，连续两次不通才算离线，一次通就算在线；
//! 也记着哪些会话的跟读暂时连不上机器（“重连中”），以及哪些会话正在因机器回来而续跑。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Weak};
use std::time::Duration;

use blazar_core_types::SessionId;
use blazar_transport::{ExecSpec, NodeTransport, SshTransport};
use chrono::Utc;
use futures::StreamExt;

use crate::api::Shared;
use crate::state::{AppState, ServerEvent};

/// 两轮探测之间隔多久。
pub const EVERY: Duration = Duration::from_secs(45);

/// 单台机器一次探测最多等多久（含 SSH 握手）。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 连续几次探测失败才判离线。
pub const OFFLINE_AFTER: u32 = 2;

/// 同时探测几台机器。
pub const PARALLEL: usize = 8;

#[derive(Default)]
pub struct HealthState {
    stopping: AtomicBool,
    reconnecting: Mutex<HashMap<SessionId, String>>,
    resuming: Mutex<HashSet<SessionId>>,
}

/// 持有期间同一个会话不会再被“机器回来了”续跑一次。
pub struct ResumeClaim<'a> {
    set: &'a Mutex<HashSet<SessionId>>,
    sid: SessionId,
}

impl Drop for ResumeClaim<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.set.lock() {
            s.remove(&self.sid);
        }
    }
}

impl HealthState {
    /// hub 要退出了：之后不再探测机器。
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// 记下会话的跟读连不上 `node` 了；第一次记下时返回 true。
    pub fn set_reconnecting(&self, sid: SessionId, node: &str) -> bool {
        self.reconnecting
            .lock()
            .is_ok_and(|mut m| m.insert(sid, node.to_owned()).is_none())
    }

    /// 跟读重新连上（或会话已收尾）；原来在重连中时返回 true。
    pub fn clear_reconnecting(&self, sid: SessionId) -> bool {
        self.reconnecting
            .lock()
            .is_ok_and(|mut m| m.remove(&sid).is_some())
    }

    /// 会话正在重连的机器名。
    #[must_use]
    pub fn reconnecting(&self, sid: SessionId) -> Option<String> {
        self.reconnecting
            .lock()
            .ok()
            .and_then(|m| m.get(&sid).cloned())
    }

    /// 占住这个会话的续跑；已经有人在续跑时返回 None。
    pub fn claim_resume(&self, sid: SessionId) -> Option<ResumeClaim<'_>> {
        if !self.resuming.lock().ok()?.insert(sid) {
            return None;
        }
        Some(ResumeClaim {
            set: &self.resuming,
            sid,
        })
    }
}

/// 后台探测循环。hub 的状态被释放或开始退出时结束。
pub async fn watch(st: Weak<AppState>) {
    let mut misses = HashMap::new();
    loop {
        tokio::time::sleep(EVERY).await;
        let Some(st) = st.upgrade() else {
            return;
        };
        if st.health.stopping() {
            return;
        }
        tick(&st, &mut misses, |name| reachable(name, PROBE_TIMEOUT)).await;
    }
}

/// 一轮探测：并发探所有远端机器，再按两次失败的规则改状态。
pub async fn tick<F, Fut>(st: &Shared, misses: &mut HashMap<String, u32>, probe: F)
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = bool>,
{
    let names = remote_nodes(st).await;
    misses.retain(|n, _| names.contains(n));
    if names.is_empty() {
        return;
    }
    let results: Vec<(String, bool)> = futures::stream::iter(names)
        .map(|name| {
            let probing = probe(name.clone());
            async move { (name, probing.await) }
        })
        .buffer_unordered(PARALLEL)
        .collect()
        .await;
    if st.health.stopping() {
        return;
    }
    apply(st, misses, results).await;
}

/// 把一轮探测的结果落到 `nodes.status` 上。
pub async fn apply(st: &Shared, misses: &mut HashMap<String, u32>, results: Vec<(String, bool)>) {
    for (name, ok) in results {
        if ok {
            misses.remove(&name);
            mark_online(st, &name).await;
        } else {
            let n = misses.entry(name.clone()).or_insert(0);
            *n = n.saturating_add(1);
            if *n >= OFFLINE_AFTER {
                mark_offline(st, &name).await;
            }
        }
    }
}

async fn remote_nodes(st: &Shared) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT name FROM nodes WHERE name <> 'local' AND transport = 'ssh'
           AND (CASE WHEN json_valid(labels) THEN lower(COALESCE(json_extract(labels, '$.cost'), '')) END)
               IS NOT 'local'
         ORDER BY name",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
}

/// 机器连得上：刷新 `last_seen_at`，不在线就改成在线并通知页面；
/// 然后走“机器回来了”的流程（续跑因它中断的活，或者把卡片改成可以继续）。
/// 状态从别的值变成在线时返回 true。
pub async fn mark_online(st: &Shared, name: &str) -> bool {
    if name == "local" {
        return false;
    }
    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query("UPDATE nodes SET last_seen_at = ?1 WHERE name = ?2")
        .bind(&now)
        .bind(name)
        .execute(st.db.pool())
        .await;
    let changed = sqlx::query(
        "UPDATE nodes SET status = 'online', last_seen_at = ?1 WHERE name = ?2 AND status IS NOT 'online'",
    )
    .bind(&now)
    .bind(name)
    .execute(st.db.pool())
    .await
    .is_ok_and(|r| r.rows_affected() > 0);
    if changed {
        tracing::info!(target: "blazar::health", "{name} 连上了");
        st.emit(ServerEvent::NodesChanged);
    }
    crate::failures::node_back(st, name).await;
    changed
}

/// 改成离线并通知页面；状态真的变了时返回 true。
pub async fn mark_offline(st: &Shared, name: &str) -> bool {
    if name == "local" {
        return false;
    }
    let changed = sqlx::query(
        "UPDATE nodes SET status = 'offline' WHERE name = ?1 AND status IS NOT 'offline'",
    )
    .bind(name)
    .execute(st.db.pool())
    .await
    .is_ok_and(|r| r.rows_affected() > 0);
    if changed {
        tracing::info!(target: "blazar::health", "{name} 连不上了");
        st.emit(ServerEvent::NodesChanged);
    }
    changed
}

/// 轻量探测：经复用的 SSH 连接跑一次 `true`，`limit` 内没回来就算不通。
pub(crate) async fn reachable(name: String, limit: Duration) -> bool {
    let secs = u32::try_from(limit.as_secs().max(1)).unwrap_or(u32::MAX);
    let t = SshTransport::new(name)
        .connect_timeout(secs)
        .multiplexed(crate::state::ssh_control_dir());
    tokio::time::timeout(limit, t.exec(ExecSpec::new("true")))
        .await
        .is_ok_and(|r| r.is_ok_and(|o| o.code == 0))
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
