use std::sync::Arc;

use blazar_core_types::{SessionId, WorkspaceId};
use blazar_transport::detached::{DetachedRun, RunMode, RunState};
use blazar_transport::{ExecSpec, LocalTransport};
use chrono::Utc;

use super::*;
use crate::state::AppState;

struct Hub {
    st: Shared,
    root: tempfile::TempDir,
    ws: String,
    runs: Vec<DetachedRun>,
}

async fn hub() -> Hub {
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let st = AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
        crate::services::Services::Isolated,
    );
    let ws = WorkspaceId::new().to_string();
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES ('n-local', 'local', 'local', 'online', '0')",
    )
    .execute(st.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, created_at)
         VALUES (?1, 'n-local', 'scratch', ?2, '0')",
    )
    .bind(&ws)
    .bind(root.path().display().to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    Hub {
        st,
        root,
        ws,
        runs: Vec::new(),
    }
}

impl Hub {
    async fn insert(
        &self,
        sid: &str,
        thread: &str,
        minutes_ago: i64,
        status: &str,
        dir: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, thread_id,
                                   node, run_dir)
             VALUES (?1, ?2, 'claude', ?3, ?4, ?5, 'local', ?6)",
        )
        .bind(sid)
        .bind(&self.ws)
        .bind(status)
        .bind((Utc::now() - chrono::Duration::minutes(minutes_ago)).to_rfc3339())
        .bind(thread)
        .bind(dir)
        .execute(self.st.db.pool())
        .await
        .unwrap();
    }

    async fn stale_round(&mut self, thread: &str, minutes_ago: i64) -> (String, DetachedRun) {
        let sid = SessionId::new().to_string();
        let run = DetachedRun::launch_in(
            Arc::new(LocalTransport),
            self.root.path(),
            &sid,
            &ExecSpec::new("sleep").arg("30"),
            None,
            RunMode::Null,
        )
        .await
        .unwrap();
        self.insert(&sid, thread, minutes_ago, "failed", Some(&run.dir))
            .await;
        self.runs.push(run.clone());
        (sid, run)
    }

    async fn round(&self, thread: &str, minutes_ago: i64, status: &str) -> String {
        let sid = SessionId::new().to_string();
        self.insert(&sid, thread, minutes_ago, status, None).await;
        sid
    }

    async fn status(&self, sid: &str) -> String {
        sqlx::query_scalar("SELECT status FROM sessions WHERE id = ?1")
            .bind(sid)
            .fetch_one(self.st.db.pool())
            .await
            .unwrap()
    }

    async fn notices(&self, sid: &str) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT payload FROM events WHERE session_id = ?1
               AND payload LIKE '{\"type\":\"notice\"%' ORDER BY seq",
        )
        .bind(sid)
        .fetch_all(self.st.db.pool())
        .await
        .unwrap()
        .into_iter()
        .filter_map(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
        .filter_map(|v| v["message"].as_str().map(str::to_owned))
        .collect()
    }

    async fn reports(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT title FROM inbox WHERE kind = 'report' ORDER BY title")
            .fetch_all(self.st.db.pool())
            .await
            .unwrap()
    }

    async fn release(&self, sid: &str) -> bool {
        let taken = self
            .st
            .running
            .write()
            .await
            .remove(&SessionId(sid.parse().unwrap()));
        if let Some(t) = &taken {
            t.task.abort();
        }
        taken.is_some()
    }

    async fn cleanup(&self) {
        for run in &self.runs {
            let _ = run.hard_kill().await;
        }
    }
}

async fn state(run: &DetachedRun) -> RunState {
    run.probe().await.unwrap()
}

#[test]
fn only_the_newest_round_of_an_idle_conversation_is_taken_back() {
    assert_eq!(
        verdict("b", Some("b"), false, false, true),
        Verdict::TakeBack
    );
    assert_eq!(
        verdict("a", Some("b"), false, false, true),
        Verdict::Stop,
        "对话已经往下走了，旧的一轮不接回"
    );
    assert_eq!(
        verdict("b", Some("b"), true, false, true),
        Verdict::Stop,
        "对话里有一轮正在跑，不能再接回一个"
    );
    assert_eq!(verdict("b", Some("b"), false, true, true), Verdict::Stop);
    assert_eq!(
        verdict("b", Some("b"), false, false, false),
        Verdict::Stop,
        "用户手动开新一轮时一律停掉"
    );
    assert_eq!(verdict("b", None, false, false, true), Verdict::Stop);
}

#[tokio::test]
async fn a_new_round_stops_every_earlier_run_still_going_in_its_conversation() {
    let mut h = hub().await;
    let (old, old_run) = h.stale_round("thread-a", 3).await;
    let (last, last_run) = h.stale_round("thread-a", 2).await;
    let (_, other_run) = h.stale_round("thread-b", 1).await;
    let cleared = clear(&h.st, &h.ws, "thread-a").await;
    let states = [
        state(&old_run).await,
        state(&last_run).await,
        state(&other_run).await,
    ];
    h.cleanup().await;
    assert!(cleared.is_ok(), "{cleared:?}");
    assert!(matches!(states[0], RunState::Exited { .. }), "{states:?}");
    assert!(matches!(states[1], RunState::Exited { .. }), "{states:?}");
    assert!(
        matches!(states[2], RunState::Alive { .. }),
        "别的对话里的运行不能动: {states:?}"
    );
    assert_eq!(h.notices(&old).await, vec![STOPPED.to_owned()]);
    assert_eq!(h.notices(&last).await, vec![STOPPED.to_owned()]);
    assert_eq!(h.status(&last).await, "failed");
    assert!(h.reports().await.is_empty(), "用户自己开的新一轮不进收件箱");
}

#[tokio::test]
async fn a_conversation_without_leftovers_starts_without_probing() {
    let h = hub().await;
    h.round("thread-a", 2, "failed").await;
    assert!(clear(&h.st, &h.ws, "thread-a").await.is_ok());
    assert!(!take_back(&h.st, &h.ws, "thread-a").await);
}

#[tokio::test]
async fn the_startup_sweep_takes_back_the_newest_live_round_and_stops_the_rest() {
    let mut h = hub().await;
    let (old, old_run) = h.stale_round("thread-a", 4).await;
    let (last, last_run) = h.stale_round("thread-a", 3).await;
    let (busy, busy_run) = h.stale_round("thread-b", 2).await;
    h.round("thread-b", 1, "running").await;
    sweep(h.st.clone()).await;
    let taken = h.release(&last).await;
    let states = [
        state(&old_run).await,
        state(&last_run).await,
        state(&busy_run).await,
    ];
    h.cleanup().await;
    assert!(taken, "最新的一轮要接回来继续跟");
    assert_eq!(h.status(&last).await, "running");
    assert_eq!(h.notices(&last).await, vec![TAKEN_BACK.to_owned()]);
    assert!(matches!(states[0], RunState::Exited { .. }), "{states:?}");
    assert!(matches!(states[1], RunState::Alive { .. }), "{states:?}");
    assert!(
        matches!(states[2], RunState::Exited { .. }),
        "对话里已经有一轮在跑，残留的要停掉: {states:?}"
    );
    assert_eq!(h.notices(&old).await, vec![STOPPED.to_owned()]);
    assert_eq!(h.notices(&busy).await, vec![STOPPED.to_owned()]);
    let reports = h.reports().await;
    assert_eq!(reports.len(), 3, "{reports:?}");
    assert_eq!(
        reports
            .iter()
            .filter(|t| t.ends_with(TAKEN_BACK_TITLE))
            .count(),
        1,
        "{reports:?}"
    );
}

#[tokio::test]
async fn auto_resume_takes_the_round_back_instead_of_starting_another() {
    let mut h = hub().await;
    let (last, last_run) = h.stale_round("thread-a", 2).await;
    let took = take_back(&h.st, &h.ws, "thread-a").await;
    let taken = h.release(&last).await;
    let alive = state(&last_run).await;
    h.cleanup().await;
    assert!(took && taken);
    assert!(matches!(alive, RunState::Alive { .. }), "{alive:?}");
    assert_eq!(h.status(&last).await, "running");
}
