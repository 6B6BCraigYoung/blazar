use std::sync::Arc;

use blazar_core_types::{SessionId, WorkspaceId};
use blazar_transport::detached::{DetachedRun, RunState};

use super::*;
use crate::bots::{Bot, NewBot};
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

    async fn spawn(&self, command: &str, owner: &Owner) -> Job {
        spawn(
            &self.st,
            Spawn {
                workspace_id: self.ws.clone(),
                command: command.into(),
                label: Some("test".into()),
                ..Spawn::default()
            },
            owner,
        )
        .await
        .unwrap()
    }

    async fn settle(&self, id: &str) -> Job {
        for _ in 0..60 {
            sweep(&self.st).await;
            let j = get(&self.st, id).await.unwrap();
            if j.status != "running" {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("任务 {id} 一直没结束: {:?}", get(&self.st, id).await);
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.st.db.pool())
            .await
            .unwrap()
    }

    async fn session(&self, thread: &str) -> String {
        let sid = SessionId::new().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, thread_id, node)
             VALUES (?1, ?2, 'claude', 'done', '0', ?3, 'local')",
        )
        .bind(&sid)
        .bind(&self.ws)
        .bind(thread)
        .execute(self.st.db.pool())
        .await
        .unwrap();
        sid
    }
}

fn caller(b: &Bot) -> Caller {
    Caller {
        bot: b.id.clone(),
        handle: b.handle.clone(),
        role: b.role.clone(),
        session: None,
    }
}

fn bot_owner(b: &Bot) -> Owner {
    Owner::Bot {
        bot: b.id.clone(),
        thread: None,
    }
}

fn detached_pid(secs: u32) -> i64 {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("sleep {secs} >/dev/null 2>&1 & echo $!"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[tokio::test]
async fn a_failing_command_is_marked_failed_with_its_code_and_log() {
    let h = hub().await;
    let job = h
        .spawn("sh -c 'echo hi; sleep 1; exit 3'", &Owner::User)
        .await;
    assert_eq!(job.status, "running");
    assert_eq!(job.owner_kind, "user");
    assert!(
        job.run_dir
            .as_deref()
            .unwrap()
            .starts_with(&*h.root.path().to_string_lossy())
    );
    let done = h.settle(&job.id).await;
    assert_eq!(done.status, "failed");
    assert_eq!(done.exit_code, Some(3));
    assert!(done.ended_at.is_some());
    let text = log(&h.st, &done, 200).await.unwrap();
    assert!(text.contains("hi"), "{text}");
    assert_eq!(
        h.count("SELECT COUNT(*) FROM inbox WHERE kind = 'job'")
            .await,
        1,
        "用户发起的进收件箱"
    );
    let body: String = sqlx::query_scalar("SELECT body FROM inbox WHERE kind = 'job'")
        .fetch_one(h.st.db.pool())
        .await
        .unwrap();
    assert!(body.contains("退出码：3") && body.contains("hi"), "{body}");
}

#[tokio::test]
async fn a_registered_process_ends_as_done_with_an_unknown_code_and_mails_its_bot() {
    let h = hub().await;
    let b = h.bot("trainer", "worker").await;
    let job = register(
        &h.st,
        Register {
            workspace_id: Some(b.home_ws_id.clone()),
            pid: detached_pid(1),
            label: "nap".into(),
            ..Register::default()
        },
        &bot_owner(&b),
        Some(&caller(&b)),
    )
    .await
    .unwrap();
    assert_eq!(job.kind, "register");
    let done = h.settle(&job.id).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.exit_code, None);
    let mail = crate::bots::pending(&h.st, &b.id).await;
    assert_eq!(mail.len(), 1, "bot 发起的写进 bot_mail");
    assert_eq!(mail[0].from_kind, "system");
    assert!(mail[0].body.contains("退出码未知"), "{}", mail[0].body);
    assert!(mail[0].body.contains("nap"));
    assert_eq!(
        h.count("SELECT COUNT(*) FROM inbox WHERE kind = 'job'")
            .await,
        0
    );
    assert!(
        register(
            &h.st,
            Register {
                node: Some("local".into()),
                pid: 999_999,
                label: "ghost".into(),
                ..Register::default()
            },
            &Owner::User,
            None,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn kill_marks_killed_and_workers_cannot_stop_others() {
    let h = hub().await;
    let mine = h.bot("trainer", "worker").await;
    let other = h.bot("viewer", "worker").await;
    let lead = h.bot("boss", "lead").await;
    let job = h.spawn("sleep 30", &bot_owner(&mine)).await;
    let theirs = Some(caller(&other));
    assert!(
        kill(&h.st, &job.id, theirs.as_ref(), &bot_owner(&other))
            .await
            .is_err()
    );
    assert!(
        list(&h.st, &Filter::default(), theirs.as_ref())
            .await
            .unwrap()
            .is_empty(),
        "worker 看不到别人的任务"
    );
    assert_eq!(
        list(&h.st, &Filter::default(), Some(&caller(&lead)))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        crate::fleet::allowed(
            &h.st,
            &caller(&other),
            "job_kill",
            &json!({ "job_id": job.id })
        )
        .await
        .is_err()
    );
    assert!(
        crate::fleet::allowed(
            &h.st,
            &caller(&mine),
            "job_kill",
            &json!({ "job_id": job.id })
        )
        .await
        .is_ok()
    );
    let fleet = crate::bots::status::fleet_status(&h.st, 0).await;
    assert_eq!(fleet["jobs"][0]["id"], json!(job.id));
    assert_eq!(fleet["jobs"][0]["owner"], "trainer");
    assert_eq!(
        crate::bots::status::bot_status(&h.st, &mine, 0).await["jobs"][0]["status"],
        "running"
    );
    assert_eq!(
        crate::bots::status::bot_status(&h.st, &other, 0).await["jobs"],
        json!([])
    );
    let killed = kill(&h.st, &job.id, Some(&caller(&mine)), &bot_owner(&mine))
        .await
        .unwrap();
    assert_eq!(killed.status, "killed");
    assert!(
        crate::bots::pending(&h.st, &mine.id).await.is_empty(),
        "自己停的不用再通知自己"
    );
    sweep(&h.st).await;
    assert_eq!(get(&h.st, &job.id).await.unwrap().status, "killed");
    assert!(
        kill(&h.st, &job.id, None, &Owner::User).await.is_err(),
        "已经结束的不能再停"
    );
}

#[tokio::test]
async fn workers_register_only_inside_their_claims_and_cannot_stop_registered_jobs() {
    let h = hub().await;
    let w = h.bot("trainer", "worker").await;
    let lead = h.bot("boss", "lead").await;
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES ('n-far', 'far', 'ssh', 'online', '0')",
    )
    .execute(h.st.db.pool())
    .await
    .unwrap();
    let pid = detached_pid(30);
    let req = |ws: Option<&str>, node: Option<&str>| Register {
        workspace_id: ws.map(str::to_owned),
        node: node.map(str::to_owned),
        pid,
        label: "watched".into(),
        ..Register::default()
    };
    let me = Some(caller(&w));
    let no_ws = register(&h.st, req(None, Some("local")), &bot_owner(&w), me.as_ref())
        .await
        .unwrap_err();
    assert!(no_ws.contains("workspace_id"), "{no_ws}");
    let outside = register(&h.st, req(Some(&h.ws), None), &bot_owner(&w), me.as_ref())
        .await
        .unwrap_err();
    assert!(outside.contains("认领"), "{outside}");
    let wrong_node = register(
        &h.st,
        req(Some(&w.home_ws_id), Some("far")),
        &bot_owner(&w),
        me.as_ref(),
    )
    .await
    .unwrap_err();
    assert!(wrong_node.contains("far"), "{wrong_node}");
    for args in [
        json!({ "node": "local", "pid": pid, "label": "x" }),
        json!({ "workspace_id": h.ws, "pid": pid, "label": "x" }),
    ] {
        assert!(
            crate::fleet::allowed(&h.st, &caller(&w), "job_register", &args)
                .await
                .is_err(),
            "{args}"
        );
    }
    assert!(
        crate::fleet::allowed(
            &h.st,
            &caller(&w),
            "job_register",
            &json!({ "workspace_id": w.home_ws_id, "pid": pid, "label": "x" })
        )
        .await
        .is_ok()
    );
    let job = register(
        &h.st,
        req(Some(&w.home_ws_id), None),
        &bot_owner(&w),
        me.as_ref(),
    )
    .await
    .unwrap();
    assert_eq!(
        (job.node.as_str(), job.kind.as_str()),
        ("local", "register")
    );
    let refused = kill(&h.st, &job.id, me.as_ref(), &bot_owner(&w))
        .await
        .unwrap_err();
    assert!(refused.contains("登记"), "{refused}");
    assert!(
        crate::fleet::allowed(&h.st, &caller(&w), "job_kill", &json!({ "job_id": job.id }))
            .await
            .is_err()
    );
    assert!(
        crate::fleet::allowed(&h.st, &caller(&w), "job_log", &json!({ "job_id": job.id }))
            .await
            .is_ok()
    );
    let mut headers = HeaderMap::new();
    let (k, v) = crate::bots::api::caller_header(&caller(&w));
    headers.insert(
        axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
        v.parse().unwrap(),
    );
    let r = crate::bots::api::kill_job(
        axum::extract::State(h.st.clone()),
        axum::extract::Path(job.id.clone()),
        headers.clone(),
    )
    .await;
    assert!(!r.status().is_success());
    let r = crate::bots::api::register_job(
        axum::extract::State(h.st.clone()),
        headers.clone(),
        axum::Json(req(None, Some("local"))),
    )
    .await;
    assert!(!r.status().is_success());
    let r = crate::bots::api::spawn_job(
        axum::extract::State(h.st.clone()),
        headers,
        axum::Json(Spawn {
            workspace_id: h.ws.clone(),
            command: "true".into(),
            ..Spawn::default()
        }),
    )
    .await;
    assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN);
    assert_eq!(get(&h.st, &job.id).await.unwrap().status, "running");
    let killed = kill(&h.st, &job.id, Some(&caller(&lead)), &bot_owner(&lead))
        .await
        .unwrap();
    assert_eq!(killed.status, "killed");
    let again = Register {
        pid: detached_pid(30),
        ..req(None, Some("local"))
    };
    let again = register(&h.st, again, &Owner::User, None).await.unwrap();
    assert_eq!(again.workspace_id, None);
    kill(&h.st, &again.id, None, &Owner::User).await.unwrap();
}

#[tokio::test]
async fn a_passed_deadline_is_noted_only_once() {
    let h = hub().await;
    let job = spawn(
        &h.st,
        Spawn {
            workspace_id: h.ws.clone(),
            command: "sleep 30".into(),
            label: Some("long".into()),
            deadline_mins: Some(60),
            ..Spawn::default()
        },
        &Owner::User,
    )
    .await
    .unwrap();
    sweep(&h.st).await;
    assert_eq!(
        h.count("SELECT COUNT(*) FROM inbox WHERE kind = 'job'")
            .await,
        0
    );
    sqlx::query("UPDATE jobs SET deadline_at = '2000-01-01T00:00:00Z' WHERE id = ?1")
        .bind(&job.id)
        .execute(h.st.db.pool())
        .await
        .unwrap();
    for _ in 0..3 {
        sweep(&h.st).await;
    }
    assert_eq!(
        h.count("SELECT COUNT(*) FROM inbox WHERE kind = 'job'")
            .await,
        1
    );
    let j = get(&h.st, &job.id).await.unwrap();
    assert_eq!(j.status, "running");
    assert!(j.deadline_noted_at.is_some());
    kill(&h.st, &job.id, None, &Owner::User).await.unwrap();
}

#[tokio::test]
async fn an_unreachable_node_keeps_jobs_running_with_the_error() {
    let h = hub().await;
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES ('n-gone', 'nowhere.invalid', 'ssh', 'offline', '0')",
    )
    .execute(h.st.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO jobs (id, owner_kind, node_id, kind, label, run_dir, created_at, updated_at)
         VALUES ('j-gone', 'user', 'n-gone', 'spawn', 'far', '/tmp/none', '0', '0')",
    )
    .execute(h.st.db.pool())
    .await
    .unwrap();
    sweep(&h.st).await;
    let j = get(&h.st, "j-gone").await.unwrap();
    assert_eq!(j.status, "running");
    assert!(j.last_error.is_some());
}

#[tokio::test]
async fn session_jobs_resume_an_idle_thread_and_interject_into_a_running_one() {
    let h = hub().await;
    let ws = WorkspaceId(h.ws.parse().unwrap());
    let sid = h.session("thread-a").await;
    h.session("thread-b").await;
    assert_eq!(route(&h.st, ws, "thread-a").await, Wake::Resume);

    let run_root = tempfile::tempdir().unwrap();
    let live = |interactive: bool| {
        Arc::new(crate::run::Live {
            run: DetachedRun::attach(
                Arc::new(blazar_transport::LocalTransport),
                run_root.path().display().to_string(),
                sid.clone(),
            ),
            interactive,
            state: tokio::sync::Mutex::new(crate::run::LiveState {
                next_seq: 1,
                pending_user: 0,
                user_no: 0,
                eof_sent: false,
                interrupt_requested: false,
                bg: Default::default(),
                idle: false,
                app: Default::default(),
            }),
        })
    };
    let running = |interactive: bool| crate::state::RunningSession {
        workspace_id: ws,
        task: tokio::spawn(async {}),
        killer: None,
        live: Some(live(interactive)),
    };
    let key = SessionId(sid.parse().unwrap());
    h.st.running.write().await.insert(key, running(true));
    assert_eq!(route(&h.st, ws, "thread-a").await, Wake::Interject);
    assert_eq!(
        route(&h.st, ws, "thread-b").await,
        Wake::Busy,
        "别的对话在跑时不插进去"
    );
    h.st.running.write().await.insert(key, running(false));
    assert_eq!(route(&h.st, ws, "thread-a").await, Wake::Busy);
    h.st.running.write().await.clear();
}

#[tokio::test]
async fn session_jobs_that_ask_for_the_inbox_land_there_with_their_thread() {
    let h = hub().await;
    let sid = h.session("thread-a").await;
    let mut headers = HeaderMap::new();
    headers.insert(SESSION_HEADER, sid.parse().unwrap());
    let owner = owner_of(&h.st, &headers).await;
    assert_eq!(
        owner,
        Owner::Session {
            thread: "thread-a".into(),
            workspace: h.ws.clone()
        }
    );
    let job = spawn(
        &h.st,
        Spawn {
            workspace_id: h.ws.clone(),
            command: "true".into(),
            label: Some("quick".into()),
            notify: Some("inbox".into()),
            ..Spawn::default()
        },
        &owner,
    )
    .await
    .unwrap();
    let done = h.settle(&job.id).await;
    assert_eq!((done.status.as_str(), done.exit_code), ("done", Some(0)));
    assert_eq!(done.owner_label(), "scratch");
    let thread: Option<String> =
        sqlx::query_scalar("SELECT thread_id FROM inbox WHERE kind = 'job'")
            .fetch_one(h.st.db.pool())
            .await
            .unwrap();
    assert_eq!(thread.as_deref(), Some("thread-a"));
    assert_eq!(owner_of(&h.st, &HeaderMap::new()).await, Owner::User);
}

#[tokio::test]
async fn wake_falls_back_to_the_inbox_when_the_thread_is_gone() {
    let h = hub().await;
    let job = h
        .spawn(
            "true",
            &Owner::Session {
                thread: "vanished".into(),
                workspace: String::new(),
            },
        )
        .await;
    h.settle(&job.id).await;
    assert_eq!(
        h.count("SELECT COUNT(*) FROM inbox WHERE kind = 'job'")
            .await,
        1
    );
}

#[tokio::test]
async fn waits_for_is_validated_and_normalized() {
    let h = hub().await;
    let spawn_waiting = |waits: Value| {
        spawn(
            &h.st,
            Spawn {
                workspace_id: h.ws.clone(),
                command: "sleep 30".into(),
                label: Some("watcher".into()),
                waits_for: waits.as_array().cloned().unwrap_or_default(),
                ..Spawn::default()
            },
            &Owner::User,
        )
    };
    let many: Vec<Value> = (0..21).map(|_| json!({ "node": "local" })).collect();
    for bad in [
        json!([{ "node": "local", "fact": "x" }]),
        json!([{ "path": "relative/file" }]),
        json!([{ "disk": "/" }]),
        json!([{ "node": "nowhere" }]),
        json!([{ "job": "no-such-job" }]),
        json!([{ "fact": "/bad" }]),
        json!([{ "node": "" }]),
        json!(["local"]),
        json!(many),
    ] {
        assert!(spawn_waiting(bad.clone()).await.is_err(), "{bad}");
    }
    let job = spawn_waiting(json!([
        { "node": " local " },
        { "path": "/tmp/x" },
        { "fact": "run/q01" }
    ]))
    .await
    .unwrap();
    assert_eq!(
        job.waits_for,
        json!([{ "node": "local" }, { "path": "/tmp/x" }, { "fact": "run/q01" }])
    );
    assert_eq!(job.wait_paths(), ["/tmp/x"]);
    let after = spawn_waiting(json!([{ "job": job.id }])).await.unwrap();
    assert_eq!(after.waits(), [("job".to_owned(), job.id.clone())]);
    let registered = register(
        &h.st,
        Register {
            node: Some("local".into()),
            pid: detached_pid(30),
            label: "adopted".into(),
            waits_for: vec![json!({ "node": "local" })],
            ..Register::default()
        },
        &Owner::User,
        None,
    )
    .await
    .unwrap();
    assert_eq!(registered.waits_for, json!([{ "node": "local" }]));
    for j in [&job, &after, &registered] {
        kill(&h.st, &j.id, None, &Owner::User).await.unwrap();
    }
}

#[tokio::test]
async fn missing_wait_paths_are_noted_by_the_sweep() {
    let h = hub().await;
    let file = h.root.path().join("ckpt.pt");
    std::fs::write(&file, "x").unwrap();
    let job = spawn(
        &h.st,
        Spawn {
            workspace_id: h.ws.clone(),
            command: "sleep 30".into(),
            label: Some("watcher".into()),
            waits_for: vec![json!({ "path": file.display().to_string() })],
            ..Spawn::default()
        },
        &Owner::User,
    )
    .await
    .unwrap();
    sweep(&h.st).await;
    assert!(missing_paths(&h.st, &job.id).is_empty());
    std::fs::remove_file(&file).unwrap();
    sweep(&h.st).await;
    assert_eq!(missing_paths(&h.st, &job.id), [file.display().to_string()]);
    std::fs::write(&file, "x").unwrap();
    sweep(&h.st).await;
    assert!(missing_paths(&h.st, &job.id).is_empty());
    std::fs::remove_file(&file).unwrap();
    sweep(&h.st).await;
    kill(&h.st, &job.id, None, &Owner::User).await.unwrap();
    sweep(&h.st).await;
    assert!(missing_paths(&h.st, &job.id).is_empty(), "结束的任务不再记");
}

#[test]
fn verdicts_follow_the_run_state() {
    let job = |kind: &str| Job {
        id: "j".into(),
        owner_kind: "user".into(),
        bot_id: None,
        bot_handle: None,
        bot_name: None,
        session_id: None,
        owner_workspace: None,
        workspace_id: None,
        workspace_name: None,
        node: "local".into(),
        kind: kind.into(),
        label: "x".into(),
        command: None,
        cwd: None,
        run_dir: None,
        pid: None,
        log_path: None,
        waits_for: json!([]),
        deadline_at: None,
        deadline_noted_at: None,
        status: "running".into(),
        exit_code: None,
        notify: "wake".into(),
        last_checked_at: None,
        last_error: None,
        created_at: "2026-10-20T00:00:00Z".into(),
        updated_at: String::new(),
        ended_at: Some("2026-10-20T01:02:03Z".into()),
    };
    let spawned = job("spawn");
    let adopted = job("register");
    assert_eq!(
        verdict(&spawned, &RunState::Exited { code: 0, size: 0 }),
        Some(("done", Some(0)))
    );
    assert_eq!(
        verdict(&spawned, &RunState::Exited { code: 2, size: 0 }),
        Some(("failed", Some(2)))
    );
    assert_eq!(
        verdict(&spawned, &RunState::Lost { size: 0 }),
        Some(("lost", None))
    );
    assert_eq!(verdict(&spawned, &RunState::Missing), Some(("lost", None)));
    assert_eq!(
        verdict(&adopted, &RunState::Lost { size: 0 }),
        Some(("done", None))
    );
    assert_eq!(
        verdict(
            &adopted,
            &RunState::Alive {
                size: 0,
                input_replaced: true
            }
        ),
        None
    );
    let lines: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    let mut ended = job("register");
    ended.status = "done".into();
    let text = summary(&ended, &lines);
    assert!(
        text.contains("退出码未知") && text.contains("1 小时 2 分"),
        "{text}"
    );
    assert!(text.contains("line 30") && text.contains("line 11") && !text.contains("line 10\n"));
}

#[tokio::test]
async fn a_bot_job_names_its_owner_by_display_name_for_people_and_by_handle_for_bots() {
    let h = hub().await;
    let b = crate::bots::create(
        &h.st,
        NewBot {
            handle: "demo-trainer".into(),
            display_name: Some("训练助手".into()),
            agent_id: "agent-a".into(),
            role: Some("worker".into()),
            home_path: Some(h.root.path().join("demo-trainer").display().to_string()),
            ..NewBot::default()
        },
    )
    .await
    .unwrap();
    let job = h.spawn("true", &bot_owner(&b)).await;
    let job = get(&h.st, &job.id).await.unwrap();
    assert_eq!(job.bot_name.as_deref(), Some("训练助手"));
    assert_eq!(job.owner_label(), "训练助手");
    assert_eq!(brief(&job)["owner"], "demo-trainer");
}
