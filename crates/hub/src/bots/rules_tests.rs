use std::sync::Arc;

use axum::http::StatusCode;
use blazar_core_types::SessionId;
use blazar_transport::detached::DetachedRun;

use super::*;
use crate::bots::jobs::{Owner, Spawn};
use crate::bots::{NewBot, NewMail};
use crate::state::AppState;

struct Hub {
    st: Shared,
    root: tempfile::TempDir,
    ws: String,
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
    *st.bots.jobs.root.lock().unwrap() = Some(root.path().join("jobs"));
    let ws = WorkspaceId::new().to_string();
    for q in [
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES ('n-local', 'local', 'local', 'online', '0')",
        "INSERT INTO nodes (id, name, transport, network, status, created_at)
         VALUES ('n-far', 'far', 'ssh', 'ssh', 'online', '0')",
        "INSERT INTO agents (id, name, runtime, created_at, updated_at)
         VALUES ('agent-a', 'a', 'claude', '0', '0')",
    ] {
        sqlx::query(q).execute(st.db.pool()).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, created_at)
         VALUES (?1, 'n-local', 'scratch', ?2, '0')",
    )
    .bind(&ws)
    .bind(root.path().display().to_string())
    .execute(st.db.pool())
    .await
    .unwrap();
    Hub { st, root, ws }
}

impl Hub {
    async fn bot(&self, handle: &str, role: &str) -> Bot {
        crate::bots::create(
            &self.st,
            NewBot {
                handle: handle.into(),
                agent_id: "agent-a".into(),
                role: Some(role.into()),
                home_path: Some(self.root.path().join(handle).display().to_string()),
                ..NewBot::default()
            },
        )
        .await
        .unwrap()
    }

    async fn sql(&self, q: &str) {
        sqlx::query(q).execute(self.st.db.pool()).await.unwrap();
    }

    async fn alerts(&self, rule: &str) -> Vec<(String, Option<String>)> {
        sqlx::query_as(
            "SELECT title, resolved_at FROM inbox WHERE kind = 'alert' AND ref_id LIKE ?1
             ORDER BY created_at",
        )
        .bind(format!("rule:{rule}:%"))
        .fetch_all(self.st.db.pool())
        .await
        .unwrap()
    }

    async fn open(&self, rule: &str) -> usize {
        self.alerts(rule)
            .await
            .iter()
            .filter(|(_, r)| r.is_none())
            .count()
    }

    async fn backdate(&self, rule: &str, mins: i64) {
        sqlx::query("UPDATE rule_alerts SET since = ?2 WHERE rule = ?1 AND resolved_at IS NULL")
            .bind(rule)
            .bind((Utc::now() - chrono::Duration::minutes(mins)).to_rfc3339())
            .execute(self.st.db.pool())
            .await
            .unwrap();
    }

    async fn set(&self, rules: Value) -> Response {
        crate::office::put_prefs(State(self.st.clone()), Json(json!({ "rules": rules }))).await
    }

    async fn rule_mail(&self, bot: &Bot) -> Vec<crate::bots::Mail> {
        crate::bots::pending(&self.st, &bot.id)
            .await
            .into_iter()
            .filter(|m| m.from_kind == "rule")
            .collect()
    }
}

fn ago(mins: i64) -> String {
    (Utc::now() - chrono::Duration::minutes(mins)).to_rfc3339()
}

#[tokio::test]
async fn settings_have_defaults_validate_and_round_trip() {
    let h = hub().await;
    let cfg = settings(&h.st).await;
    assert_eq!(
        cfg["bot_stuck"],
        Setting {
            enabled: true,
            threshold: 20
        }
    );
    assert_eq!(cfg["approval_waiting"].threshold, 15);
    assert_eq!(cfg["node_offline"].threshold, 10);
    assert_eq!(cfg["quota_high"].threshold, 90);
    assert_eq!(cfg["claim_expired"].threshold, 24);
    assert!(cfg["job_orphan"].enabled && cfg["fact_stale"].enabled);

    let r = h
        .set(json!({ "bot_stuck": { "threshold": 45 }, "quota_high": { "enabled": false } }))
        .await;
    assert_eq!(r.status(), StatusCode::OK);
    let cfg = settings(&h.st).await;
    assert_eq!(
        (cfg["bot_stuck"].threshold, cfg["quota_high"].enabled),
        (45, false)
    );
    let prefs = crate::office::prefs(&h.st).await;
    assert_eq!(prefs["rules"]["bot_stuck"]["threshold"], 45);
    assert_eq!(prefs["rules"]["job_orphan"], json!({ "enabled": true }));

    for bad in [
        json!({ "bot_stuck": { "threshold": 0 } }),
        json!({ "quota_high": { "threshold": 101 } }),
        json!({ "job_orphan": { "threshold": 5 } }),
        json!({ "nope": { "enabled": true } }),
        json!({ "bot_stuck": { "enabled": "yes" } }),
        json!([1]),
    ] {
        assert_eq!(
            h.set(bad.clone()).await.status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(settings(&h.st).await["bot_stuck"].threshold, 45);

    let r = crate::office::put_prefs(
        State(h.st.clone()),
        Json(json!({ "fleet": { "enabled": true } })),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        settings(&h.st).await["bot_stuck"].threshold,
        45,
        "改别的设置不会冲掉规则"
    );

    let listed = list(State(h.st.clone())).await;
    let body = axum::body::to_bytes(listed.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    let rules = v["rules"].as_array().unwrap();
    assert_eq!(rules.len(), SPECS.len());
    let stuck = rules.iter().find(|r| r["key"] == "bot_stuck").unwrap();
    assert_eq!(
        (stuck["threshold"].as_i64(), stuck["unit"].as_str()),
        (Some(45), Some("分钟"))
    );
    let orphan = rules.iter().find(|r| r["key"] == "job_orphan").unwrap();
    assert!(orphan["threshold"].is_null());
    assert!(
        rules
            .iter()
            .all(|r| !r["about"].as_str().unwrap().is_empty())
    );
}

#[tokio::test]
async fn node_offline_waits_for_the_threshold_alerts_once_resolves_and_comes_back() {
    let h = hub().await;
    let lead = h.bot("boss", "lead").await;
    let worker = h.bot("trainer", "worker").await;
    sqlx::query("UPDATE workspaces SET node_id = 'n-far' WHERE id = ?1")
        .bind(&worker.home_ws_id)
        .execute(h.st.db.pool())
        .await
        .unwrap();
    h.sql(
        "INSERT INTO jobs (id, owner_kind, node_id, kind, label, run_dir, created_at, updated_at)
         VALUES ('j-far', 'user', 'n-far', 'spawn', 'train-far', '/tmp/none', '0', '0')",
    )
    .await;
    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;

    run_once(&h.st).await;
    assert!(h.alerts("node_offline").await.is_empty(), "刚离线不提醒");
    h.backdate("node_offline", 9).await;
    run_once(&h.st).await;
    assert!(h.alerts("node_offline").await.is_empty(), "9 分钟还不到");
    h.backdate("node_offline", 11).await;
    run_once(&h.st).await;
    run_once(&h.st).await;
    let alerts = h.alerts("node_offline").await;
    assert_eq!(alerts.len(), 1, "持续期间只提醒一次");
    assert!(alerts[0].0.contains("far"), "{alerts:?}");
    let body: String =
        sqlx::query_scalar("SELECT body FROM inbox WHERE ref_id = 'rule:node_offline:far'")
            .fetch_one(h.st.db.pool())
            .await
            .unwrap();
    assert!(
        body.contains("train-far") && body.contains("相关的 bot：") && !body.contains("bot:"),
        "{body}"
    );
    let mail = h.rule_mail(&lead).await;
    assert_eq!(mail.len(), 1, "严重的给总控写信");
    assert_eq!(mail[0].from_ref, "node_offline");
    assert!(h.rule_mail(&worker).await.is_empty());

    h.sql("UPDATE nodes SET status = 'online' WHERE name = 'far'")
        .await;
    run_once(&h.st).await;
    let alerts = h.alerts("node_offline").await;
    assert!(alerts[0].1.is_some(), "恢复在线后标为已解决");
    assert_eq!(h.open("node_offline").await, 0);

    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("node_offline").await.len(), 1, "再次离线也要先等");
    h.backdate("node_offline", 11).await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("node_offline").await.len(), 2, "再次出现再提醒");
    assert_eq!(h.rule_mail(&lead).await.len(), 2);

    h.sql("UPDATE nodes SET status = 'online' WHERE name = 'far'")
        .await;
    run_once(&h.st).await;
    h.set(json!({ "node_offline": { "threshold": 30 } })).await;
    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;
    run_once(&h.st).await;
    h.backdate("node_offline", 11).await;
    run_once(&h.st).await;
    assert_eq!(
        h.alerts("node_offline").await.len(),
        2,
        "阈值调大后 11 分钟不提醒"
    );
    h.backdate("node_offline", 31).await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("node_offline").await.len(), 3);
}

#[tokio::test]
async fn severe_alerts_skip_the_mail_when_there_is_no_lead_and_switches_silence_rules() {
    let h = hub().await;
    let worker = h.bot("trainer", "worker").await;
    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;
    h.set(json!({ "node_offline": { "enabled": false } })).await;
    run_once(&h.st).await;
    h.backdate("node_offline", 60).await;
    run_once(&h.st).await;
    assert!(h.alerts("node_offline").await.is_empty(), "关掉就不提醒");

    h.set(json!({ "node_offline": { "enabled": true } })).await;
    run_once(&h.st).await;
    h.backdate("node_offline", 11).await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("node_offline").await.len(), 1);
    assert!(
        h.rule_mail(&worker).await.is_empty(),
        "没有总控就只进收件箱"
    );
    let mails: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bot_mail WHERE from_kind = 'rule'")
        .fetch_one(h.st.db.pool())
        .await
        .unwrap();
    assert_eq!(mails, 0);
}

#[tokio::test]
async fn bots_on_an_offline_node_get_no_work_until_it_is_back() {
    let h = hub().await;
    let b = h.bot("trainer", "worker").await;
    sqlx::query("UPDATE workspaces SET node_id = 'n-far' WHERE id = ?1")
        .bind(&b.home_ws_id)
        .execute(h.st.db.pool())
        .await
        .unwrap();
    crate::bots::send(
        &h.st,
        &b.id,
        NewMail {
            body: "do it".into(),
            ..NewMail::default()
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE bot_mail SET thread_hint = 'closed-thread' WHERE to_bot = ?1")
        .bind(&b.id)
        .execute(h.st.db.pool())
        .await
        .unwrap();
    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;
    assert_eq!(offline_node_of(&h.st, &b).await.as_deref(), Some("far"));
    crate::bots::dispatch::tick(&h.st).await;
    crate::bots::dispatch::dispatch_one(&h.st, &b).await;
    assert_eq!(
        crate::bots::pending(&h.st, &b.id).await.len(),
        1,
        "邮件留着"
    );
    let tried: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox WHERE title LIKE '%没能启动%'")
        .fetch_one(h.st.db.pool())
        .await
        .unwrap();
    assert_eq!(tried, 0, "离线时不去启动");

    h.sql("UPDATE nodes SET status = 'online' WHERE name = 'far'")
        .await;
    assert_eq!(offline_node_of(&h.st, &b).await, None);
    crate::bots::dispatch::dispatch_one(&h.st, &b).await;
    assert!(
        crate::bots::pending(&h.st, &b.id).await.is_empty(),
        "恢复后照常处理邮件"
    );
    let tried: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox WHERE title LIKE '%没能启动%'")
        .fetch_one(h.st.db.pool())
        .await
        .unwrap();
    assert_eq!(
        tried, 1,
        "这封邮件指向关掉的线程，派活时失败，说明确实去派了"
    );

    let brain = h.bot("sim", "worker").await;
    let far_ws = WorkspaceId::new().to_string();
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, created_at)
         VALUES (?1, 'n-far', 'far', '/far', '0')",
    )
    .bind(&far_ws)
    .execute(h.st.db.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE bots SET brain = 'node', brain_ws_id = ?2 WHERE id = ?1")
        .bind(&brain.id)
        .bind(&far_ws)
        .execute(h.st.db.pool())
        .await
        .unwrap();
    let brain = crate::bots::get(&h.st, &brain.id).await.unwrap();
    assert_eq!(offline_node_of(&h.st, &brain).await, None);
    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;
    assert_eq!(
        offline_node_of(&h.st, &brain).await.as_deref(),
        Some("far"),
        "思考位置在离线机器上也不派"
    );
}

fn live(dir: &std::path::Path, sid: &str, idle: bool) -> Arc<crate::run::Live> {
    Arc::new(crate::run::Live {
        run: DetachedRun::attach(
            Arc::new(blazar_transport::LocalTransport),
            dir.display().to_string(),
            sid.to_owned(),
        ),
        interactive: true,
        state: tokio::sync::Mutex::new(crate::run::LiveState {
            next_seq: 1,
            pending_user: 0,
            user_no: 0,
            eof_sent: false,
            interrupt_requested: false,
            bg: Default::default(),
            idle,
            app: Default::default(),
        }),
    })
}

async fn running_session(h: &Hub, ws: &str, last_event_mins: i64) -> String {
    let sid = SessionId::new().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at)
         VALUES (?1, ?2, 'claude_cli', 'running', ?3)",
    )
    .bind(&sid)
    .bind(ws)
    .bind(ago(120))
    .execute(h.st.db.pool())
    .await
    .unwrap();
    event(h, &sid, 1, last_event_mins).await;
    sid
}

async fn event(h: &Hub, sid: &str, seq: i64, mins: i64) {
    sqlx::query("INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, ?2, ?3, '{}')")
        .bind(sid)
        .bind(seq)
        .bind(ago(mins))
        .execute(h.st.db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_stuck_bot_is_reported_then_escalated_and_resolved_by_new_events() {
    let h = hub().await;
    let lead = h.bot("boss", "lead").await;
    let b = h.bot("trainer", "worker").await;
    let sid = running_session(&h, &b.home_ws_id, 25).await;
    let run_root = tempfile::tempdir().unwrap();
    let ws = WorkspaceId(b.home_ws_id.parse().unwrap());
    let key = SessionId(sid.parse().unwrap());
    let put = |idle: bool| crate::state::RunningSession {
        workspace_id: ws,
        task: tokio::spawn(async {}),
        killer: None,
        live: Some(live(run_root.path(), &sid, idle)),
    };
    h.st.running.write().await.insert(key, put(false));
    crate::bots::set_status(&h.st, &b.id, "running", "").await;

    run_once(&h.st).await;
    run_once(&h.st).await;
    let alerts = h.alerts("bot_stuck").await;
    assert_eq!(alerts.len(), 1, "{alerts:?}");
    assert!(alerts[0].0.contains("trainer") && alerts[0].0.contains("20"));
    assert!(h.rule_mail(&lead).await.is_empty(), "还没到两倍");

    event(&h, &sid, 2, 41).await;
    sqlx::query("DELETE FROM events WHERE session_id = ?1 AND seq = 1")
        .bind(&sid)
        .execute(h.st.db.pool())
        .await
        .unwrap();
    run_once(&h.st).await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("bot_stuck").await.len(), 1);
    let mail = h.rule_mail(&lead).await;
    assert_eq!(mail.len(), 1, "超过两倍写信给总控");
    assert_eq!(mail[0].from_ref, "bot_stuck");

    event(&h, &sid, 3, 0).await;
    run_once(&h.st).await;
    assert!(h.alerts("bot_stuck").await[0].1.is_some(), "有新事件就解除");

    event(&h, &sid, 4, -0).await;
    sqlx::query("UPDATE events SET ts = ?2 WHERE session_id = ?1")
        .bind(&sid)
        .bind(ago(22))
        .execute(h.st.db.pool())
        .await
        .unwrap();
    run_once(&h.st).await;
    assert_eq!(h.alerts("bot_stuck").await.len(), 2, "又卡住再提醒");
    assert_eq!(h.open("bot_stuck").await, 1);

    h.st.running.write().await.insert(key, put(true));
    run_once(&h.st).await;
    assert_eq!(h.open("bot_stuck").await, 0, "空闲等输入不算卡住");

    h.set(json!({ "bot_stuck": { "threshold": 30 } })).await;
    h.st.running.write().await.insert(key, put(false));
    run_once(&h.st).await;
    assert_eq!(
        h.alerts("bot_stuck").await.len(),
        2,
        "阈值 30 分钟时 22 分钟不算"
    );
    h.set(json!({ "bot_stuck": { "threshold": 20, "enabled": false } }))
        .await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("bot_stuck").await.len(), 2, "关掉不提醒");
    h.st.running.write().await.clear();
}

#[tokio::test]
async fn the_lead_is_not_mailed_about_itself() {
    let h = hub().await;
    let lead = h.bot("boss", "lead").await;
    let sid = running_session(&h, &lead.home_ws_id, 50).await;
    let run_root = tempfile::tempdir().unwrap();
    h.st.running.write().await.insert(
        SessionId(sid.parse().unwrap()),
        crate::state::RunningSession {
            workspace_id: WorkspaceId(lead.home_ws_id.parse().unwrap()),
            task: tokio::spawn(async {}),
            killer: None,
            live: Some(live(run_root.path(), &sid, false)),
        },
    );
    crate::bots::set_status(&h.st, &lead.id, "running", "").await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("bot_stuck").await.len(), 1);
    assert!(h.rule_mail(&lead).await.is_empty());
    h.st.running.write().await.clear();
}

#[tokio::test]
async fn a_waiting_approval_is_reminded_once_and_resolved_when_decided() {
    let h = hub().await;
    let sid = SessionId::new().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at)
         VALUES (?1, ?2, 'claude_cli', 'running', '0')",
    )
    .bind(&sid)
    .bind(&h.ws)
    .execute(h.st.db.pool())
    .await
    .unwrap();
    let approve = |id: &str, mins: i64| {
        sqlx::query(
            "INSERT INTO approvals (id, session_id, request, created_at)
             VALUES (?1, ?2, '{\"tool_name\":\"Bash\"}', ?3)",
        )
        .bind(id.to_owned())
        .bind(sid.clone())
        .bind(ago(mins))
        .execute(h.st.db.pool())
    };
    approve("a-young", 5).await.unwrap();
    approve("a-old", 16).await.unwrap();
    run_once(&h.st).await;
    run_once(&h.st).await;
    let alerts = h.alerts("approval_waiting").await;
    assert_eq!(alerts.len(), 1, "只有等了 15 分钟以上的");
    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT body, thread_id FROM inbox WHERE ref_id = 'rule:approval_waiting:a-old'",
    )
    .fetch_one(h.st.db.pool())
    .await
    .unwrap();
    assert!(
        row.0.contains("Bash") && row.0.contains("scratch"),
        "{}",
        row.0
    );
    assert_eq!(row.1.as_deref(), Some(sid.as_str()));

    h.sql("UPDATE approvals SET decision = 'allow' WHERE id = 'a-old'")
        .await;
    run_once(&h.st).await;
    assert!(h.alerts("approval_waiting").await[0].1.is_some());
    h.sql("UPDATE approvals SET decision = NULL WHERE id = 'a-old'")
        .await;
    run_once(&h.st).await;
    assert_eq!(
        h.alerts("approval_waiting").await.len(),
        2,
        "再次出现再提醒"
    );

    h.set(json!({ "approval_waiting": { "threshold": 3 } }))
        .await;
    run_once(&h.st).await;
    assert_eq!(
        h.alerts("approval_waiting").await.len(),
        3,
        "阈值调小后年轻的也算"
    );

    h.sql("UPDATE sessions SET status = 'done'").await;
    run_once(&h.st).await;
    assert_eq!(h.open("approval_waiting").await, 0, "会话结束就解除");
    h.set(json!({ "approval_waiting": { "enabled": false } }))
        .await;
    h.sql("UPDATE sessions SET status = 'running'").await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("approval_waiting").await.len(), 3, "关掉不提醒");
}

async fn spawn_waiting(
    h: &Hub,
    owner: &Owner,
    waits: Value,
) -> Result<crate::bots::jobs::Job, String> {
    crate::bots::jobs::spawn(
        &h.st,
        Spawn {
            workspace_id: h.ws.clone(),
            command: "sleep 30".into(),
            label: Some("watcher".into()),
            waits_for: waits.as_array().cloned().unwrap_or_default(),
            ..Spawn::default()
        },
        owner,
    )
    .await
}

async fn check(h: &Hub, what: &str) -> Vec<(String, Option<String>)> {
    crate::bots::jobs::sweep(&h.st).await;
    run_once(&h.st).await;
    run_once(&h.st).await;
    let body: String = sqlx::query_scalar(
        "SELECT body FROM inbox WHERE kind = 'alert' AND ref_id LIKE 'rule:job_orphan:%'
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(h.st.db.pool())
    .await
    .unwrap();
    assert!(body.contains(what), "{what}: {body}");
    h.alerts("job_orphan").await
}

async fn resolved(h: &Hub) {
    crate::bots::jobs::sweep(&h.st).await;
    run_once(&h.st).await;
    assert_eq!(h.open("job_orphan").await, 0);
}

#[tokio::test]
async fn an_orphaned_job_tells_its_bot_once_for_each_kind_of_target() {
    let h = hub().await;
    let b = h.bot("trainer", "worker").await;
    let owner = Owner::Bot {
        bot: b.id.clone(),
        thread: None,
    };
    let file = h.root.path().join("ckpt.pt");
    std::fs::write(&file, "x").unwrap();
    crate::bots::facts::put(
        &h.st,
        "run/q01",
        crate::bots::facts::Put {
            value: json!({ "step": 1 }),
            ..crate::bots::facts::Put::default()
        },
        None,
    )
    .await
    .unwrap();
    let target = spawn_waiting(&h, &Owner::User, json!([])).await.unwrap();
    let job = spawn_waiting(
        &h,
        &owner,
        json!([
            { "node": "far" },
            { "fact": "run/q01" },
            { "path": file.display().to_string() },
            { "job": target.id },
        ]),
    )
    .await
    .unwrap();

    crate::bots::jobs::sweep(&h.st).await;
    run_once(&h.st).await;
    assert!(h.alerts("job_orphan").await.is_empty(), "都还在");

    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;
    assert_eq!(check(&h, "far").await.len(), 1);
    let mail = h.rule_mail(&b).await;
    assert_eq!(mail.len(), 1, "通知发起的 bot");
    assert_eq!(mail[0].from_ref, "job_orphan");
    assert!(mail[0].body.contains("watcher"));
    h.sql("UPDATE nodes SET status = 'online' WHERE name = 'far'")
        .await;
    resolved(&h).await;

    h.sql("UPDATE facts SET status = 'lost' WHERE key = 'run/q01'")
        .await;
    assert_eq!(check(&h, "run/q01").await.len(), 2);
    h.sql("UPDATE facts SET status = 'ok' WHERE key = 'run/q01'")
        .await;
    resolved(&h).await;

    std::fs::remove_file(&file).unwrap();
    assert_eq!(check(&h, "ckpt.pt").await.len(), 3);
    std::fs::write(&file, "x").unwrap();
    resolved(&h).await;

    crate::bots::jobs::kill(&h.st, &target.id, None, &Owner::User)
        .await
        .unwrap();
    assert_eq!(check(&h, "killed").await.len(), 4);
    assert_eq!(h.rule_mail(&b).await.len(), 4);

    h.set(json!({ "job_orphan": { "enabled": false } })).await;
    h.sql("UPDATE nodes SET status = 'offline' WHERE name = 'far'")
        .await;
    crate::bots::jobs::sweep(&h.st).await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("job_orphan").await.len(), 4, "关掉不提醒");

    crate::bots::jobs::kill(&h.st, &job.id, None, &Owner::User)
        .await
        .unwrap();
    h.set(json!({ "job_orphan": { "enabled": true } })).await;
    run_once(&h.st).await;
    assert_eq!(h.open("job_orphan").await, 0, "任务结束就解除");
}

#[tokio::test]
async fn quota_high_reminds_once_per_window() {
    let h = hub().await;
    h.sql(
        "INSERT INTO accounts (id, provider, display_name, auth_mode, created_at)
         VALUES ('acc', 'anthropic', 'work', 'oauth', '0')",
    )
    .await;
    let snap = |used: f64, resets_mins: i64| {
        sqlx::query(
            "INSERT INTO rate_limit_snapshots (account_id, window_name, utilization, resets_at, observed_at)
             VALUES ('acc', 'five_hour', ?1, ?2, '0')
             ON CONFLICT (account_id, window_name) DO UPDATE SET
               utilization = excluded.utilization, resets_at = excluded.resets_at",
        )
        .bind(used)
        .bind(ago(-resets_mins))
        .execute(h.st.db.pool())
    };
    snap(0.85, 60).await.unwrap();
    run_once(&h.st).await;
    assert!(h.alerts("quota_high").await.is_empty());
    snap(0.93, 60).await.unwrap();
    run_once(&h.st).await;
    snap(0.97, 50).await.unwrap();
    run_once(&h.st).await;
    let alerts = h.alerts("quota_high").await;
    assert_eq!(alerts.len(), 1, "同一个窗口只提醒一次");
    assert!(
        alerts[0].0.contains("work")
            && alerts[0].0.contains("5 小时窗口")
            && alerts[0].0.contains("93%"),
        "{alerts:?}"
    );

    snap(0.97, -1).await.unwrap();
    run_once(&h.st).await;
    assert!(
        h.alerts("quota_high").await[0].1.is_some(),
        "窗口过去就解除"
    );
    snap(0.95, 300).await.unwrap();
    run_once(&h.st).await;
    assert_eq!(h.alerts("quota_high").await.len(), 2, "新窗口再提醒");
    snap(0.10, 290).await.unwrap();
    run_once(&h.st).await;
    assert_eq!(h.open("quota_high").await, 0);

    h.set(json!({ "quota_high": { "threshold": 96 } })).await;
    snap(0.95, 280).await.unwrap();
    run_once(&h.st).await;
    assert_eq!(h.alerts("quota_high").await.len(), 2, "没过 96%");
    h.set(json!({ "quota_high": { "threshold": 90 } })).await;
    run_once(&h.st).await;
    assert_eq!(h.open("quota_high").await, 1);
    h.set(json!({ "quota_high": { "enabled": false } })).await;
    run_once(&h.st).await;
    assert_eq!(
        h.open("quota_high").await,
        0,
        "关掉开关就不再跟踪，未解决的一并解除"
    );
    snap(0.99, 270).await.unwrap();
    run_once(&h.st).await;
    assert_eq!(h.alerts("quota_high").await.len(), 3, "关掉不提醒");
    h.set(json!({ "quota_high": { "enabled": true } })).await;
    run_once(&h.st).await;
    assert_eq!(
        h.alerts("quota_high").await.len(),
        4,
        "重新打开时还在就再提醒"
    );
}

#[tokio::test]
async fn fact_stale_resolves_when_the_fact_is_fresh_again_and_can_be_switched_off() {
    let h = hub().await;
    let b = h.bot("trainer", "worker").await;
    let caller = crate::bots::Caller {
        bot: b.id.clone(),
        handle: b.handle.clone(),
        role: b.role.clone(),
        session: None,
    };
    let put = |key: &'static str| {
        let caller = caller.clone();
        let st = h.st.clone();
        async move {
            crate::bots::facts::put(
                &st,
                key,
                crate::bots::facts::Put {
                    value: json!({ "step": 1 }),
                    ttl_secs: Some(60),
                    ..crate::bots::facts::Put::default()
                },
                Some(&caller),
            )
            .await
            .unwrap();
        }
    };
    let age = |key: &'static str| {
        sqlx::query("UPDATE facts SET updated_at = ?2 WHERE key = ?1")
            .bind(key)
            .bind(ago(10))
            .execute(h.st.db.pool())
    };
    put("run/a").await;
    age("run/a").await.unwrap();
    run_once(&h.st).await;
    run_once(&h.st).await;
    assert_eq!(h.alerts("fact_stale").await.len(), 1);
    assert_eq!(h.rule_mail(&b).await.len(), 1);
    put("run/a").await;
    run_once(&h.st).await;
    assert!(h.alerts("fact_stale").await[0].1.is_some(), "更新后解除");
    age("run/a").await.unwrap();
    run_once(&h.st).await;
    assert_eq!(h.alerts("fact_stale").await.len(), 2, "再次失联再提醒");

    h.set(json!({ "fact_stale": { "enabled": false } })).await;
    put("run/b").await;
    age("run/b").await.unwrap();
    run_once(&h.st).await;
    assert_eq!(
        crate::bots::facts::get(&h.st, "run/b")
            .await
            .unwrap()
            .status,
        "lost",
        "关掉提醒也照样标状态"
    );
    assert_eq!(h.alerts("fact_stale").await.len(), 2);
    assert_eq!(h.rule_mail(&b).await.len(), 2);
}

#[tokio::test]
async fn claim_expiry_follows_its_switch_and_release_threshold() {
    let h = hub().await;
    let b = h.bot("trainer", "worker").await;
    sqlx::query(
        "INSERT INTO claims (id, bot_id, node_id, scope, mode, expires_at, created_at)
         VALUES ('c1', ?1, 'n-local', 'gpu:0', 'exclusive', ?2, '0')",
    )
    .bind(&b.id)
    .bind(ago(5))
    .execute(h.st.db.pool())
    .await
    .unwrap();
    h.set(json!({ "claim_expired": { "enabled": false } }))
        .await;
    run_once(&h.st).await;
    assert!(h.rule_mail(&b).await.is_empty(), "关掉不通知");
    h.set(json!({ "claim_expired": { "enabled": true, "threshold": 2 } }))
        .await;
    run_once(&h.st).await;
    run_once(&h.st).await;
    let mail = h.rule_mail(&b).await;
    assert_eq!(mail.len(), 1);
    assert!(mail[0].body.contains("2 hours"), "{}", mail[0].body);
    h.sql("UPDATE claims SET notified_at = '2000-01-01T00:00:00+00:00'")
        .await;
    run_once(&h.st).await;
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM claims")
        .fetch_one(h.st.db.pool())
        .await
        .unwrap();
    assert_eq!(left, 0, "过了释放时间自动释放");
}

#[tokio::test]
async fn periodic_node_probes_run_every_two_minutes_regardless_of_the_offline_rule() {
    let h = hub().await;
    assert!(probe_due(None), "启动后第一轮就探测");
    let now = std::time::Instant::now();
    assert!(!probe_due(Some(now)), "两分钟内不重复探测");
    let long_ago = now.checked_sub(PROBE_EVERY).unwrap();
    assert!(probe_due(Some(long_ago)));
    assert_eq!(PROBE_EVERY, std::time::Duration::from_secs(120));
    h.set(json!({ "node_offline": { "enabled": false } })).await;
    let cfg = settings(&h.st).await;
    assert!(!cfg["node_offline"].enabled, "规则关掉只影响提醒");
    assert!(probe_due(Some(long_ago)), "规则关掉也照常探测");
    assert!(probe_due(None));
}
