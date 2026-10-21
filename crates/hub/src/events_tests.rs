use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws;
use blazar_core_types::{SessionId, WorkspaceId};
use futures::StreamExt;
use futures::channel::mpsc as fmpsc;

use super::*;
use crate::state::AppState;

async fn state_with(db: Db) -> (Arc<AppState>, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let st = AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
        crate::services::Services::Isolated,
    );
    (st, root)
}

async fn memory_state() -> (Arc<AppState>, tempfile::TempDir) {
    state_with(Db::open_in_memory().await.unwrap()).await
}

fn titled(n: usize) -> ServerEvent {
    ServerEvent::SessionTitled {
        workspace_id: WorkspaceId::new(),
        session_id: SessionId::new(),
        title: format!("t{n}"),
    }
}

async fn booted(st: &AppState) -> i64 {
    for _ in 0..500 {
        if let Some(boot) = st.events.boot() {
            return boot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("hub_events 没有写进启动标记");
}

async fn settle(st: &AppState, want: i64) {
    for _ in 0..500 {
        if head(&st.db).await.unwrap() >= want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("事件没有在时限内写进 hub_events");
}

fn connect(st: &Arc<AppState>, since: Option<i64>) -> fmpsc::UnboundedReceiver<ws::Message> {
    let (tx, rx) = fmpsc::unbounded();
    let bus = st.bus.subscribe();
    tokio::spawn(crate::api::forward(st.clone(), since, bus, tx));
    rx
}

async fn next_frame(rx: &mut fmpsc::UnboundedReceiver<ws::Message>) -> EventFrame {
    let msg = tokio::time::timeout(Duration::from_secs(5), rx.next())
        .await
        .expect("等 WS 消息超时")
        .expect("WS 已关闭");
    let ws::Message::Text(text) = msg else {
        panic!("不是文本帧");
    };
    serde_json::from_str(text.as_str()).unwrap()
}

fn title_of(frame: &EventFrame) -> String {
    match &frame.event {
        ServerEvent::SessionTitled { title, .. } => title.clone(),
        other => format!("{other:?}"),
    }
}

#[tokio::test]
async fn events_missed_while_disconnected_are_replayed_in_order() {
    let (st, _root) = memory_state().await;
    let boot = booted(&st).await;
    st.emit(titled(0));
    settle(&st, boot + 1).await;
    let since = head(&st.db).await.unwrap();
    for n in 1..=5 {
        st.emit(titled(n));
    }
    st.emit(ServerEvent::InboxChanged);
    settle(&st, since + 6).await;

    let mut rx = connect(&st, Some(since));
    let mut got = Vec::new();
    for _ in 0..6 {
        got.push(next_frame(&mut rx).await);
    }
    let ids: Vec<i64> = got.iter().map(|f| f.event_id.unwrap()).collect();
    assert_eq!(ids, (since + 1..=since + 6).collect::<Vec<_>>());
    let titles: Vec<String> = got[..5].iter().map(title_of).collect();
    assert_eq!(titles, ["t1", "t2", "t3", "t4", "t5"]);
    assert!(matches!(got[5].event, ServerEvent::InboxChanged));

    st.emit(titled(9));
    let live = next_frame(&mut rx).await;
    assert_eq!(live.event_id, Some(since + 7));
    assert_eq!(title_of(&live), "t9");
}

#[tokio::test]
async fn repeated_notifications_are_replayed_once_at_their_latest_position() {
    let (st, _root) = memory_state().await;
    let since = booted(&st).await;
    for event in [
        ServerEvent::InboxChanged,
        ServerEvent::BotsChanged,
        ServerEvent::InboxChanged,
        titled(1),
        ServerEvent::InboxChanged,
        titled(1),
        ServerEvent::NodesChanged,
    ] {
        st.emit(event);
    }
    settle(&st, since + 7).await;
    let Replay::Events(frames) = replay(&st.db, since, Some(since)).await.unwrap() else {
        panic!("不该有缺口");
    };
    let got: Vec<(i64, String)> = frames
        .iter()
        .map(|f| {
            let kind = serde_json::to_value(&f.event).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned();
            (f.event_id.unwrap() - since, kind)
        })
        .collect();
    let want: Vec<(i64, String)> = [
        (2, "bots_changed"),
        (4, "session_titled"),
        (5, "inbox_changed"),
        (6, "session_titled"),
        (7, "nodes_changed"),
    ]
    .into_iter()
    .map(|(i, k)| (i, k.to_owned()))
    .collect();
    assert_eq!(got, want);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replay_and_live_events_meet_without_loss_or_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("hub.sqlite")).await.unwrap();
    let (st, _root) = state_with(db).await;
    let boot = booted(&st).await;
    const TOTAL: usize = 400;

    let writers: Vec<_> = (0..4)
        .map(|w| {
            let st = st.clone();
            tokio::spawn(async move {
                for n in 0..TOTAL / 4 {
                    st.emit(titled(w * 1000 + n));
                    st.emit(ServerEvent::QueueChanged {
                        workspace_id: WorkspaceId::new(),
                    });
                    if n % 7 == 0 {
                        tokio::task::yield_now().await;
                    }
                }
            })
        })
        .collect();

    let mut clients = Vec::new();
    for k in 0..24 {
        tokio::time::sleep(Duration::from_micros(300 + k * 150)).await;
        let now = head(&st.db).await.unwrap();
        let since = (now - (k as i64 % 5) * 3).max(boot);
        clients.push((since, connect(&st, Some(since))));
    }
    for w in writers {
        w.await.unwrap();
    }
    settle(&st, boot + TOTAL as i64).await;
    st.emit(ServerEvent::InboxChanged);
    let last = boot + TOTAL as i64 + 1;
    settle(&st, last).await;
    assert_eq!(head(&st.db).await.unwrap(), last);
    let mut fresh = 0;

    for (since, mut rx) in clients {
        let mut ids = Vec::new();
        while ids.last() != Some(&last) {
            let frame = next_frame(&mut rx).await;
            if let Some(id) = frame.event_id {
                ids.push(id);
            } else {
                assert!(
                    matches!(frame.event, ServerEvent::QueueChanged { .. }),
                    "{:?}",
                    frame.event
                );
            }
        }
        assert_eq!(
            ids,
            (since + 1..=last).collect::<Vec<_>>(),
            "从 {since} 接上的连接丢了或重了事件"
        );
        if since > boot && since < boot + TOTAL as i64 {
            fresh += 1;
        }
    }
    assert!(fresh > 0, "没有一条连接是在写入过程中接上的");
}

#[tokio::test]
async fn a_cursor_older_than_the_retained_window_requires_a_resync() {
    let (st, _root) = memory_state().await;
    let boot = booted(&st).await;
    assert_eq!(boot, 1);
    for n in 0..10 {
        st.emit(titled(n));
    }
    settle(&st, 11).await;
    assert_eq!(prune(&st.db, Utc::now(), KEEP_SECS, 4).await, 7);

    let b = Some(boot);
    assert!(matches!(replay(&st.db, 6, b).await.unwrap(), Replay::Gap));
    assert!(matches!(replay(&st.db, 12, b).await.unwrap(), Replay::Gap));
    let Replay::Events(frames) = replay(&st.db, 7, b).await.unwrap() else {
        panic!("刚好接在最老一条之前不算缺口");
    };
    assert_eq!(frames.len(), 4);

    let mut rx = connect(&st, Some(3));
    let first = next_frame(&mut rx).await;
    assert!(matches!(first.event, ServerEvent::ResyncRequired));
    assert_eq!(first.event_id, None);
    st.emit(ServerEvent::BotsChanged);
    let live = next_frame(&mut rx).await;
    assert_eq!(live.event_id, Some(12));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cursor_from_before_the_hub_restarted_requires_a_resync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.sqlite");
    let (st, root) = state_with(Db::open(&path).await.unwrap()).await;
    let boot = booted(&st).await;
    st.emit(ServerEvent::InboxChanged);
    settle(&st, boot + 1).await;
    let before = head(&st.db).await.unwrap();
    drop(st);
    drop(root);

    let (st, _root) = state_with(Db::open(&path).await.unwrap()).await;
    let restarted = booted(&st).await;
    assert!(restarted > before);
    assert!(matches!(
        replay(&st.db, before, Some(restarted)).await.unwrap(),
        Replay::Gap
    ));
    assert!(matches!(
        replay(&st.db, restarted, Some(restarted)).await.unwrap(),
        Replay::Events(ref f) if f.is_empty()
    ));
    let mut rx = connect(&st, Some(before));
    assert!(matches!(
        next_frame(&mut rx).await.event,
        ServerEvent::ResyncRequired
    ));
}

#[tokio::test]
async fn pruning_enforces_both_the_age_and_the_row_limits() {
    let (st, _root) = memory_state().await;
    booted(&st).await;
    sqlx::query("DELETE FROM hub_events")
        .execute(st.db.pool())
        .await
        .unwrap();
    let now = Utc::now();
    for hours in [30, 26, 25, 3, 2, 1, 0] {
        sqlx::query(
            "INSERT INTO hub_events (kind, payload, created_at) VALUES ('inbox_changed', '{\"kind\":\"inbox_changed\"}', ?1)",
        )
        .bind(stamp(now - chrono::Duration::hours(hours)))
        .execute(st.db.pool())
        .await
        .unwrap();
    }
    assert_eq!(prune(&st.db, now, KEEP_SECS, KEEP_ROWS).await, 3);
    let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM hub_events ORDER BY id")
        .fetch_all(st.db.pool())
        .await
        .unwrap();
    assert_eq!(left, [5, 6, 7, 8]);

    assert_eq!(prune(&st.db, now, KEEP_SECS, 2).await, 2);
    let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM hub_events ORDER BY id")
        .fetch_all(st.db.pool())
        .await
        .unwrap();
    assert_eq!(left, [7, 8]);
    assert_eq!(prune(&st.db, now, KEEP_SECS, 2).await, 0);
    assert_eq!(head(&st.db).await.unwrap(), 8);
}

#[tokio::test]
async fn streaming_and_ui_events_are_never_stored() {
    let (st, _root) = memory_state().await;
    let boot = booted(&st).await;
    let mut bus = st.bus.subscribe();
    let ws = WorkspaceId::new();
    st.emit(ServerEvent::QueueChanged { workspace_id: ws });
    st.emit(ServerEvent::ScriptsChanged { workspace_id: ws });
    st.emit(ServerEvent::ResyncRequired);
    st.emit(ServerEvent::UiCommand {
        id: 1,
        command: serde_json::json!({"op": "errors"}),
    });
    st.emit(ServerEvent::QueueSent {
        workspace_id: ws,
        queued_thread: None,
        thread_id: "t".into(),
        session_id: "s".into(),
    });
    st.emit(ServerEvent::FactsChanged);
    let mut frames = Vec::new();
    while frames.len() < 6 {
        frames.push(
            tokio::time::timeout(Duration::from_secs(5), bus.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(frames[..5].iter().all(|f| f.event_id.is_none()));
    assert_eq!(frames[5].event_id, Some(boot + 1));
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM hub_events WHERE id > ?1")
        .bind(boot)
        .fetch_all(st.db.pool())
        .await
        .unwrap();
    assert_eq!(kinds, ["facts_changed"]);
}

#[tokio::test]
async fn connections_without_a_cursor_only_receive_live_events() {
    let (st, _root) = memory_state().await;
    let boot = booted(&st).await;
    for n in 0..3 {
        st.emit(titled(n));
    }
    settle(&st, boot + 3).await;
    let mut rx = connect(&st, None);
    tokio::task::yield_now().await;
    st.emit(ServerEvent::NodesChanged);
    let first = next_frame(&mut rx).await;
    assert!(matches!(first.event, ServerEvent::NodesChanged));
    assert_eq!(first.event_id, Some(boot + 4));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rx.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_failed_write_still_broadcasts_without_an_id() {
    let (st, _root) = memory_state().await;
    booted(&st).await;
    sqlx::query("DROP TABLE hub_events")
        .execute(st.db.pool())
        .await
        .unwrap();
    let mut bus = st.bus.subscribe();
    st.emit(ServerEvent::InboxChanged);
    let frame = tokio::time::timeout(Duration::from_secs(5), bus.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(frame.event, ServerEvent::InboxChanged));
    assert_eq!(frame.event_id, None);
}
