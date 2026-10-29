use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use blazar_core_types::{SessionId, WorkspaceId};
use serde_json::{Value, json};

use super::*;

async fn state() -> Shared {
    let st = AppState::with_services(
        blazar_db::Db::open_in_memory().await.unwrap(),
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-health-tests")),
        crate::services::Services::Isolated,
    );
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n0', 'local', 'local', 'online', '0')",
    )
    .execute(st.db.pool())
    .await
    .unwrap();
    st
}

async fn add_node(st: &Shared, name: &str, status: &str) {
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, network, status, created_at) VALUES (?1, ?1, 'ssh', 'ssh', ?2, '0')",
    )
    .bind(name)
    .bind(status)
    .execute(st.db.pool())
    .await
    .unwrap();
}

async fn status(st: &Shared, name: &str) -> String {
    sqlx::query_scalar("SELECT status FROM nodes WHERE name = ?1")
        .bind(name)
        .fetch_one(st.db.pool())
        .await
        .unwrap()
}

/// 一个工作区在 `node` 上，最新一轮因为机器连不上失败了。
async fn failed_on(st: &Shared, node: &str) -> (WorkspaceId, SessionId) {
    let ws = WorkspaceId::new();
    let sid = SessionId::new();
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, activity, created_at)
         VALUES (?1, ?2, 'repo', '/home/me/repo', 'errored', '0')",
    )
    .bind(ws.to_string())
    .bind(node)
    .execute(st.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, failure_cause, request, created_at)
         VALUES (?1, ?2, 'claude', 'failed', 'node', '{\"text\":\"fix the bug\",\"permission_mode\":\"acceptEdits\"}', '1')",
    )
    .bind(sid.to_string())
    .bind(ws.to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    (ws, sid)
}

fn counting(
    calls: Arc<AtomicUsize>,
    reply: Value,
) -> impl Fn(
    Shared,
    String,
    crate::api::PromptRequest,
) -> futures::future::BoxFuture<'static, Result<Value, String>> {
    move |_, _, req| {
        assert_eq!(req.text, "Continue");
        assert!(req.resume);
        assert_eq!(req.permission_mode.as_deref(), Some("acceptEdits"));
        calls.fetch_add(1, Ordering::SeqCst);
        let reply = reply.clone();
        Box::pin(async move { Ok(reply) })
    }
}

async fn notices(st: &Shared, sid: SessionId) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT payload FROM events WHERE session_id = ?1 AND payload LIKE '{\"type\":\"notice\"%' ORDER BY seq",
    )
    .bind(sid.to_string())
    .fetch_all(st.db.pool())
    .await
    .unwrap()
    .into_iter()
    .filter_map(|p| serde_json::from_str::<Value>(&p).ok())
    .filter_map(|v| v["message"].as_str().map(str::to_owned))
    .collect()
}

#[tokio::test]
async fn a_machine_goes_offline_only_after_two_failed_probes_in_a_row() {
    let st = state().await;
    add_node(&st, "gpu-x", "online").await;
    let mut misses = HashMap::new();
    apply(&st, &mut misses, vec![("gpu-x".into(), false)]).await;
    assert_eq!(status(&st, "gpu-x").await, "online", "一次不通不算离线");
    apply(&st, &mut misses, vec![("gpu-x".into(), true)]).await;
    apply(&st, &mut misses, vec![("gpu-x".into(), false)]).await;
    assert_eq!(
        status(&st, "gpu-x").await,
        "online",
        "中间通过一次就重新计数"
    );
    apply(&st, &mut misses, vec![("gpu-x".into(), false)]).await;
    assert_eq!(status(&st, "gpu-x").await, "offline");
}

#[tokio::test]
async fn one_successful_probe_brings_a_machine_back_online() {
    let st = state().await;
    add_node(&st, "gpu-x", "offline").await;
    let mut events = st.bus.subscribe();
    let mut misses = HashMap::from([("gpu-x".to_owned(), 5)]);
    apply(&st, &mut misses, vec![("gpu-x".into(), true)]).await;
    assert_eq!(status(&st, "gpu-x").await, "online");
    assert!(misses.is_empty());
    let seen: Option<String> =
        sqlx::query_scalar("SELECT last_seen_at FROM nodes WHERE name = 'gpu-x'")
            .fetch_one(st.db.pool())
            .await
            .unwrap();
    assert!(seen.is_some());
    let got = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(f) if matches!(f.event, ServerEvent::NodesChanged) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(got, "状态变了要通知页面");
}

#[tokio::test]
async fn a_tick_probes_every_remote_machine_but_not_this_one() {
    let st = state().await;
    add_node(&st, "gpu-x", "online").await;
    add_node(&st, "gpu-y", "offline").await;
    let probed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut misses = HashMap::new();
    for _ in 0..2 {
        let seen = probed.clone();
        tick(&st, &mut misses, move |name| {
            seen.lock().unwrap().push(name.clone());
            async move { name == "gpu-y" }
        })
        .await;
    }
    let mut names = probed.lock().unwrap().clone();
    names.sort();
    assert_eq!(names, ["gpu-x", "gpu-x", "gpu-y", "gpu-y"]);
    assert_eq!(status(&st, "gpu-x").await, "offline");
    assert_eq!(status(&st, "gpu-y").await, "online");
    assert_eq!(status(&st, "local").await, "online");

    let empty = state().await;
    let mut misses = HashMap::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    tick(&empty, &mut misses, move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { true }
    })
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 0, "没有远端机器时不探测");
}

#[tokio::test]
async fn a_successful_probe_request_marks_the_machine_online() {
    let st = state().await;
    add_node(&st, "gpu-x", "offline").await;
    let caps = crate::api::record_probe(
        &st,
        "gpu-x",
        &blazar_transport::NodeHealth::default(),
        &std::collections::BTreeMap::new(),
    )
    .await
    .map_err(|e| e.message())
    .unwrap();
    assert!(caps["probed_at"].is_string());
    assert_eq!(status(&st, "gpu-x").await, "online");
}

#[tokio::test]
async fn a_machine_coming_back_resumes_the_interrupted_run_exactly_once() {
    let st = state().await;
    add_node(&st, "gpu-x", "offline").await;
    let (ws, sid) = failed_on(&st, "gpu-x").await;
    let calls = Arc::new(AtomicUsize::new(0));
    let send = counting(calls.clone(), json!({ "admitted": true }));
    tokio::join!(
        crate::failures::node_back_with(&st, "gpu-x", &send),
        crate::failures::node_back_with(&st, "gpu-x", &send),
    );
    crate::failures::node_back_with(&st, "gpu-x", &send).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "同一个会话只续跑一次");
    let handled: Option<String> =
        sqlx::query_scalar("SELECT handled_at FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_one(st.db.pool())
            .await
            .unwrap();
    assert!(handled.is_some());
    let activity: String = sqlx::query_scalar("SELECT activity FROM workspaces WHERE id = ?1")
        .bind(ws.to_string())
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert_eq!(activity, "idle");
    assert_eq!(
        notices(&st, sid).await,
        ["gpu-x is back. Continuing automatically (attempt 1)"]
    );
}

#[tokio::test]
async fn with_auto_resume_off_the_card_stays_and_offers_to_continue() {
    let st = state().await;
    crate::office::kv_put(
        &st,
        "prefs",
        &json!({ "auto_resume": { "enabled": false } }),
    )
    .await
    .unwrap();
    add_node(&st, "gpu-x", "offline").await;
    let (ws, sid) = failed_on(&st, "gpu-x").await;
    sqlx::query(
        "INSERT INTO inbox (id, kind, title, workspace_id, session_id, read_at, created_at)
         VALUES ('i1', 'run_failed', 'repo · 没跑成', ?1, ?2, '2', '2')",
    )
    .bind(ws.to_string())
    .bind(sid.to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let send = counting(calls.clone(), json!({ "admitted": true }));
    assert!(mark_online(&st, "gpu-x").await);
    crate::failures::node_back_with(&st, "gpu-x", &send).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "关了自动续跑就不替用户续跑"
    );
    let (title, read, resolved): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT title, read_at, resolved_at FROM inbox WHERE id = 'i1'")
            .fetch_one(st.db.pool())
            .await
            .unwrap();
    assert!(title.ends_with("gpu-x 回来了，可以继续"), "{title}");
    assert!(read.is_none(), "改完重新标成未读");
    assert!(resolved.is_none(), "卡片保留着");
    let view = crate::failures::of_workspace(&st, &ws.to_string(), true).await;
    assert!(view.is_some(), "失败没有被悄悄清掉");
    assert_eq!(
        notices(&st, sid).await,
        ["gpu-x is back. Press Continue to pick up where it stopped"],
        "再回来一次也不重复提示"
    );
}

#[tokio::test]
async fn with_auto_resume_off_a_missing_card_is_added_back() {
    let st = state().await;
    crate::office::kv_put(
        &st,
        "prefs",
        &json!({ "auto_resume": { "enabled": false } }),
    )
    .await
    .unwrap();
    add_node(&st, "gpu-x", "online").await;
    let (_, sid) = failed_on(&st, "gpu-x").await;
    crate::failures::node_back(&st, "gpu-x").await;
    let titles: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM inbox WHERE kind = 'run_failed' AND session_id = ?1 AND resolved_at IS NULL",
    )
    .bind(sid.to_string())
    .fetch_all(st.db.pool())
    .await
    .unwrap();
    assert_eq!(titles.len(), 1);
    assert!(titles[0].ends_with("gpu-x 回来了，可以继续"));
}

#[tokio::test]
async fn reconnecting_is_shown_while_the_follower_has_lost_the_machine() {
    let st = state().await;
    add_node(&st, "gpu-x", "online").await;
    let ws = WorkspaceId::new();
    let sid = SessionId::new();
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, activity, created_at)
         VALUES (?1, 'gpu-x', 'repo', '/home/me/repo', 'running', '0')",
    )
    .bind(ws.to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at)
         VALUES (?1, ?2, 'claude', 'running', '1')",
    )
    .bind(sid.to_string())
    .bind(ws.to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    let step = |snap: &blazar_core_types::api::StateSnapshot| {
        snap.workspaces
            .iter()
            .find(|w| w.id == ws.to_string())
            .and_then(|w| w.step.clone())
    };

    assert!(st.health.reconnecting(sid).is_none());
    crate::run::link_lost(&st, sid, "gpu-x", "no answer within 8 s");
    assert!(!st.health.set_reconnecting(sid, "gpu-x"), "只记一次");
    assert_eq!(st.health.reconnecting(sid).as_deref(), Some("gpu-x"));
    let snap = crate::api::snapshot(&st).await.unwrap();
    let shown = step(&snap).unwrap();
    assert_eq!(shown.kind, crate::api::RECONNECTING_STEP);
    assert_eq!(shown.text, "gpu-x");
    let w = snap
        .workspaces
        .iter()
        .find(|w| w.id == ws.to_string())
        .unwrap();
    assert_eq!(w.activity, "running", "活还在远端跑，不算失败");

    crate::run::link_back(&st, sid);
    assert!(st.health.reconnecting(sid).is_none());
    let snap = crate::api::snapshot(&st).await.unwrap();
    assert!(step(&snap).is_none_or(|s| s.kind != crate::api::RECONNECTING_STEP));
    assert!(!st.health.clear_reconnecting(sid));
}

#[test]
fn a_resume_claim_is_exclusive_until_dropped() {
    let h = HealthState::default();
    let sid = SessionId::new();
    let first = h.claim_resume(sid);
    assert!(first.is_some());
    assert!(h.claim_resume(sid).is_none());
    drop(first);
    assert!(h.claim_resume(sid).is_some());
    h.begin_shutdown();
    assert!(h.stopping());
}

#[tokio::test]
async fn a_run_that_cannot_be_resumed_is_reported_once_however_often_the_machine_answers() {
    let st = state().await;
    add_node(&st, "gpu-x", "online").await;
    let (_, sid) = failed_on(&st, "gpu-x").await;
    sqlx::query("UPDATE sessions SET request = NULL WHERE id = ?1")
        .bind(sid.to_string())
        .execute(st.db.pool())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let send = counting(calls.clone(), json!({ "admitted": true }));
    for _ in 0..3 {
        crate::failures::node_back_with(&st, "gpu-x", &send).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let cards: Vec<String> =
        sqlx::query_scalar("SELECT title FROM inbox WHERE kind = 'run_failed' AND session_id = ?1")
            .bind(sid.to_string())
            .fetch_all(st.db.pool())
            .await
            .unwrap();
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert!(cards[0].ends_with("gpu-x 回来了，没能自动继续"));
    assert_eq!(notices(&st, sid).await.len(), 1);
}
