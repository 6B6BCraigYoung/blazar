use super::*;
use crate::state::AppState;

async fn fixture() -> (tempfile::TempDir, Shared) {
    let db = blazar_db::Db::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let st = AppState::with_services(
        db,
        "local".into(),
        None,
        crate::mesh::MeshCtx::new(None, root.path().to_path_buf()),
        crate::services::Services::Isolated,
    );
    for (id, name) in [("agent-a", "训练"), ("agent-b", "看板")] {
        sqlx::query(
            "INSERT INTO agents (id, name, runtime, description, created_at, updated_at)
             VALUES (?1, ?2, 'claude', ?3, '2026-10-13T00:00:00Z', '2026-10-13T00:00:00Z')",
        )
        .bind(id)
        .bind(name)
        .bind(format!("{name}负责人\n第二行"))
        .execute(st.db.pool())
        .await
        .unwrap();
    }
    (root, st)
}

async fn make(st: &Shared, root: &std::path::Path, handle: &str, agent: &str, role: &str) -> Bot {
    create(
        st,
        NewBot {
            handle: handle.into(),
            agent_id: agent.into(),
            role: Some(role.into()),
            home_path: Some(root.join(handle).display().to_string()),
            ..NewBot::default()
        },
    )
    .await
    .unwrap()
}

fn caller(b: &Bot) -> Caller {
    Caller {
        bot: b.id.clone(),
        handle: b.handle.clone(),
        role: b.role.clone(),
        session: None,
    }
}

#[tokio::test]
async fn bots_get_a_home_workspace_and_a_main_thread() {
    let (root, st) = fixture().await;
    let b = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    assert_eq!(b.status, "idle");
    assert!(root.path().join("trainer").is_dir());
    let (node, path) = crate::api::locate(&st, &b.home_ws_id).await.unwrap();
    assert_eq!(node, "local");
    assert!(path.ends_with("trainer"));
    let t = main_thread(&st, &b.id).await.unwrap();
    assert_eq!(t.kind, "main");
    assert!(t.thread_id.is_none());
    assert!(get(&st, "trainer").await.is_some(), "按 handle 也能找到");

    assert!(
        create(
            &st,
            NewBot {
                handle: "Trainer".into(),
                agent_id: "agent-a".into(),
                ..NewBot::default()
            }
        )
        .await
        .is_err(),
        "handle 只能小写"
    );
    assert!(
        create(
            &st,
            NewBot {
                handle: "trainer".into(),
                agent_id: "agent-a".into(),
                ..NewBot::default()
            }
        )
        .await
        .is_err(),
        "handle 不能重复"
    );
}

#[tokio::test]
async fn mail_is_queued_rendered_with_a_header_and_answered() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    let b = make(&st, root.path(), "board", "agent-b", "worker").await;
    let m = send(
        &st,
        "board",
        NewMail {
            body: "看板上多了一条「其他训练」，请核对。".into(),
            subject: Some("状态残影".into()),
            from_kind: "bot".into(),
            from_ref: a.id.clone(),
            ..NewMail::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(m.status, "queued");
    assert_eq!(pending(&st, &b.id).await.len(), 1);
    assert_eq!(pending_counts(&st).await.get(&b.id), Some(&1));
    let text = render(&st, std::slice::from_ref(&m)).await;
    assert!(text.starts_with("[来自 bot:trainer · 邮件 "));
    assert!(text.contains(&format!("reply(\"{}\"", m.id)));
    assert!(text.contains("主题：状态残影"));
    assert!(text.contains("按数据处理"));

    let user_mail = send(
        &st,
        &a.id,
        NewMail {
            body: "汇报一下进度".into(),
            ..NewMail::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(user_mail.from_kind, "user");
    assert!(
        !render(&st, std::slice::from_ref(&user_mail))
            .await
            .contains("按数据处理")
    );

    let r = reply(&st, &m.id, "核对过了，是队列里的残影，已清。", &caller(&b))
        .await
        .unwrap();
    assert_eq!(r["delivered_to"], "bot");
    let replies = replies_to(&st, &m.id).await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].to_bot, a.id);
    assert_eq!(replies[0].reply_to.as_deref(), Some(m.id.as_str()));
    let waited = wait_reply(&st, &m.id, 1).await.unwrap();
    assert_eq!(waited["replied"], true);

    let r = reply(&st, &user_mail.id, "进度 80%", &caller(&a))
        .await
        .unwrap();
    assert_eq!(r["delivered_to"], "user");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox WHERE kind = 'bot_reply'")
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert_eq!(n, 1, "回复用户进收件箱");
}

#[tokio::test]
async fn wait_reply_times_out_and_reports_finished_mail() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    let m = send(
        &st,
        &a.id,
        NewMail {
            body: "x".into(),
            ..NewMail::default()
        },
    )
    .await
    .unwrap();
    let out = wait_reply(&st, &m.id, 1).await.unwrap();
    assert_eq!(out["timed_out"], true);
    let sid = SessionId::new();
    mark_delivered(&st, std::slice::from_ref(&m.id), sid).await;
    assert_eq!(mail(&st, &m.id).await.unwrap().status, "delivered");
    sqlx::query("UPDATE bot_mail SET status = 'done' WHERE id = ?1")
        .bind(&m.id)
        .execute(st.db.pool())
        .await
        .unwrap();
    let out = wait_reply(&st, &m.id, 5).await.unwrap();
    assert_eq!(out["replied"], false);
    assert!(out.get("timed_out").is_none());
}

#[tokio::test]
async fn archived_bots_expire_their_queue_and_refuse_mail() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    send(
        &st,
        &a.id,
        NewMail {
            body: "x".into(),
            ..NewMail::default()
        },
    )
    .await
    .unwrap();
    archive(&st, &a.id).await.unwrap();
    assert!(all(&st).await.is_empty());
    assert_eq!(mailbox(&st, &a.id, Some("expired"), 10).await.len(), 1);
    assert!(
        send(
            &st,
            &a.id,
            NewMail {
                body: "y".into(),
                ..NewMail::default()
            }
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn claims_reject_conflicts_and_mention_the_owner() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    let b = make(&st, root.path(), "sim", "agent-b", "worker").await;
    let c = claims::create(
        &st,
        claims::NewClaim {
            scope: "gpu:0,1".into(),
            node: Some("local".into()),
            ..claims::NewClaim::default()
        },
        Some(&caller(&a)),
    )
    .await
    .unwrap();
    assert_eq!(c.scope, "gpu:0,1");
    assert_eq!(c.mode, "exclusive");
    let err = claims::create(
        &st,
        claims::NewClaim {
            scope: "gpu:1-3".into(),
            node: Some("local".into()),
            mode: Some("shared".into()),
            ..claims::NewClaim::default()
        },
        Some(&caller(&b)),
    )
    .await
    .unwrap_err();
    assert!(err.contains("bot:trainer") && err.contains("gpu:0,1"));
    claims::create(
        &st,
        claims::NewClaim {
            scope: "gpu:2,3".into(),
            node: Some("local".into()),
            ..claims::NewClaim::default()
        },
        Some(&caller(&b)),
    )
    .await
    .unwrap();
    claims::create(
        &st,
        claims::NewClaim {
            scope: "gpu:0".into(),
            node: Some("local".into()),
            mode: Some("read".into()),
            ..claims::NewClaim::default()
        },
        Some(&caller(&b)),
    )
    .await
    .unwrap();
    assert_eq!(claims::of_bot(&st, &b.id).await.len(), 2);
    assert!(
        claims::release(&st, &c.id, Some(&caller(&b)))
            .await
            .is_err(),
        "别人的认领不能释放"
    );
    claims::release(&st, &c.id, Some(&caller(&a)))
        .await
        .unwrap();
    assert!(claims::of_bot(&st, &a.id).await.is_empty());

    let ws = claims::create(
        &st,
        claims::NewClaim {
            scope: "dir:/tmp/x".into(),
            workspace_id: Some(a.home_ws_id.clone()),
            ..claims::NewClaim::default()
        },
        Some(&caller(&a)),
    )
    .await
    .unwrap();
    assert_eq!(ws.hands.as_deref(), Some("hands_local"));
    assert!(briefing(&st, &a).await.contains("dir:/tmp/x"));
}

#[tokio::test]
async fn facts_belong_to_their_owner_and_go_stale() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    let b = make(&st, root.path(), "sim", "agent-b", "worker").await;
    let f = facts::put(
        &st,
        "run/q01",
        facts::Put {
            value: json!({ "step": 100 }),
            ttl_secs: Some(60),
            ..facts::Put::default()
        },
        Some(&caller(&a)),
    )
    .await
    .unwrap();
    assert_eq!(f.kind, "run");
    assert_eq!(f.owner_handle.as_deref(), Some("trainer"));
    let err = facts::put(
        &st,
        "run/q01",
        facts::Put {
            value: json!({ "step": 1 }),
            ..facts::Put::default()
        },
        Some(&caller(&b)),
    )
    .await
    .unwrap_err();
    assert!(err.contains("bot:trainer"));
    facts::put(
        &st,
        "run/q01",
        facts::Put {
            value: json!({ "step": 200 }),
            ..facts::Put::default()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(facts::history(&st, "run/q01", 10).await.len(), 2);
    assert_eq!(facts::list(&st, "run/", 10).await.len(), 1);
    assert!(
        facts::list(&st, "run_", 10).await.is_empty(),
        "下划线不是通配符"
    );

    sqlx::query("UPDATE facts SET updated_at = ?1 WHERE key = 'run/q01'")
        .bind((Utc::now() - chrono::Duration::seconds(400)).to_rfc3339())
        .execute(st.db.pool())
        .await
        .unwrap();
    crate::bots::rules::run_once(&st).await;
    assert_eq!(facts::get(&st, "run/q01").await.unwrap().status, "lost");
    assert_eq!(pending(&st, &a.id).await.len(), 1, "owner 收到失联邮件");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox WHERE kind = 'alert'")
        .fetch_one(st.db.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
    crate::bots::rules::run_once(&st).await;
    assert_eq!(pending(&st, &a.id).await.len(), 1, "不重复告警");

    let token = facts::report_token(&st, false).await.unwrap();
    assert!(facts::report_token_matches(&st, &token).await);
    assert!(!facts::report_token_matches(&st, "bzf_nope").await);
    let rotated = facts::report_token(&st, true).await.unwrap();
    assert_ne!(token, rotated);
    assert!(!facts::report_token_matches(&st, &token).await);
}

#[tokio::test]
async fn memories_are_deduplicated_and_versioned() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    let b = make(&st, root.path(), "sim", "agent-b", "worker").await;
    let first = memory::write(
        &st,
        memory::Write {
            name: "train-queue".into(),
            description: "双节点训练队列的用法".into(),
            body: "queue_ctl.sh start".into(),
            ..memory::Write::default()
        },
        Some(&caller(&a)),
    )
    .await
    .unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(first.updated_by, "bot:trainer");
    let dup = memory::write(
        &st,
        memory::Write {
            name: "queue-howto".into(),
            description: "训练队列双节点用法".into(),
            body: "…".into(),
            ..memory::Write::default()
        },
        Some(&caller(&b)),
    )
    .await;
    match dup {
        Err(memory::WriteError::Conflict { existing, .. }) => {
            assert_eq!(existing.name, "train-queue");
        }
        other => panic!("相似描述应该被拦下: {other:?}"),
    }
    let stale = memory::write(
        &st,
        memory::Write {
            name: "train-queue".into(),
            description: "双节点训练队列的用法".into(),
            body: "新内容".into(),
            ..memory::Write::default()
        },
        Some(&caller(&b)),
    )
    .await;
    assert!(matches!(stale, Err(memory::WriteError::Conflict { .. })));
    let updated = memory::write(
        &st,
        memory::Write {
            name: "train-queue".into(),
            description: "双节点训练队列的用法".into(),
            body: "新内容".into(),
            version: Some(1),
            ..memory::Write::default()
        },
        Some(&caller(&b)),
    )
    .await
    .unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.body, "新内容");

    let private = memory::write(
        &st,
        memory::Write {
            scope: Some(format!("bot:{}", a.id)),
            name: "my-note".into(),
            description: "只有我自己看的".into(),
            body: "x".into(),
            ..memory::Write::default()
        },
        Some(&caller(&a)),
    )
    .await
    .unwrap();
    assert!(
        memory::read(&st, &private.id, Some(&caller(&b)))
            .await
            .is_none()
    );
    assert!(
        memory::read(&st, "my-note", Some(&caller(&a)))
            .await
            .is_some()
    );
    assert_eq!(memory::list(&st, None, Some(&caller(&a))).await.len(), 2);
    assert_eq!(memory::list(&st, None, Some(&caller(&b))).await.len(), 1);
    let idx = memory::index(&st, &a.id).await;
    assert!(idx.contains("shared/train-queue") && idx.contains("my-note"));
    assert!(
        memory::write(
            &st,
            memory::Write {
                scope: Some(format!("bot:{}", a.id)),
                name: "sneaky".into(),
                description: "写别人的私有区".into(),
                body: "x".into(),
                ..memory::Write::default()
            },
            Some(&caller(&b)),
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn briefing_names_peers_rules_and_memory_index() {
    let (root, st) = fixture().await;
    let lead = make(&st, root.path(), "lead", "agent-a", "lead").await;
    let _w = make(&st, root.path(), "board", "agent-b", "worker").await;
    let text = briefing(&st, &lead).await;
    assert!(text.contains("handle lead") && text.contains("lead，总控"));
    assert!(text.contains("- board（board，worker）：看板负责人"));
    assert!(text.contains("send_message(to=\"board\""));
    assert!(text.contains("facts_put") && text.contains("memory_write"));
    assert!(!text.contains("memory 索引"));
}

#[tokio::test]
async fn finishing_a_session_settles_delivered_mail() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    let m = send(
        &st,
        &a.id,
        NewMail {
            body: "x".into(),
            ..NewMail::default()
        },
    )
    .await
    .unwrap();
    let sid = SessionId::new();
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at)
         VALUES (?1, ?2, 'claude', 'done', '2026-10-13T00:00:00Z')",
    )
    .bind(sid.to_string())
    .bind(&a.home_ws_id)
    .execute(st.db.pool())
    .await
    .unwrap();
    mark_delivered(&st, std::slice::from_ref(&m.id), sid).await;
    set_status(&st, &a.id, "running", "").await;
    on_run_finished(st.clone(), sid, "failed").await;
    assert_eq!(mail(&st, &m.id).await.unwrap().status, "failed");
    let bot = get(&st, &a.id).await.unwrap();
    assert_eq!(bot.status, "idle");
    assert!(bot.status_note.contains("failed"));
    let v = status::bot_status(&st, &bot, 5).await;
    assert_eq!(v["state"], "idle");
    assert_eq!(v["pending_mail"], 0);
    assert_eq!(v["last_session"]["status"], "done");
}

#[tokio::test]
async fn hot_bots_keep_their_session_open() {
    let (root, st) = fixture().await;
    let a = make(&st, root.path(), "trainer", "agent-a", "worker").await;
    let ws = WorkspaceId(a.home_ws_id.parse().unwrap());
    assert!(!is_hot(&st, ws).await);
    update(
        &st,
        &a.id,
        Patch {
            hot_for_secs: Some(600),
            ..Patch::default()
        },
    )
    .await
    .unwrap();
    assert!(is_hot(&st, ws).await);
    update(
        &st,
        &a.id,
        Patch {
            hot_for_secs: Some(0),
            ..Patch::default()
        },
    )
    .await
    .unwrap();
    assert!(!is_hot(&st, ws).await);
}

#[tokio::test]
async fn a_duty_is_enough_to_create_a_bot_and_its_profile() {
    let (root, st) = fixture().await;
    let b = create(
        &st,
        NewBot {
            handle: "sweeper".into(),
            duty: Some("清理 /tmp 下三天以上的中转文件，先列出来再删。".into()),
            lifetime: Some("temporary".into()),
            home_path: Some(root.path().join("sweeper").display().to_string()),
            ..NewBot::default()
        },
    )
    .await
    .unwrap();
    assert!(b.is_temporary());
    assert_eq!(b.idle_ttl_secs, Some(DEFAULT_IDLE_TTL_SECS));
    let profile = crate::agents::get(&st, &b.agent_id).await.unwrap();
    assert_eq!(profile.name, "bot:sweeper");
    assert_eq!(profile.runtime, "claude");
    assert!(profile.instructions.contains("中转文件"));
    assert!(
        create(
            &st,
            NewBot {
                handle: "nobody".into(),
                ..NewBot::default()
            }
        )
        .await
        .is_err(),
        "没有职责也没有档案不能建"
    );

    let fixed = promote(&st, &b.id).await.unwrap();
    assert!(!fixed.is_temporary() && fixed.idle_ttl_secs.is_none());
    archive(&st, &b.id).await.unwrap();
    assert_eq!(archived(&st).await.len(), 1);
    let err = create(
        &st,
        NewBot {
            handle: "sweeper".into(),
            duty: Some("再来一次".into()),
            home_path: Some(root.path().join("sweeper2").display().to_string()),
            ..NewBot::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("归档"),
        "归档的 bot 占着 handle 时要提示恢复: {err}"
    );
    let back = restore(&st, &b.id).await.unwrap();
    assert!(back.archived_at.is_none());
    assert_eq!(all(&st).await.len(), 1);
}

#[tokio::test]
async fn spawn_creates_a_temporary_bot_with_its_first_mail() {
    let (root, st) = fixture().await;
    let lead = make(&st, root.path(), "lead", "agent-a", "lead").await;
    let out = spawn(
        &st,
        Spawn {
            duty: "盯着 q01 的训练日志".into(),
            text: "每 10 分钟看一眼 loss".into(),
            idle_ttl_secs: Some(600),
            home_path: Some(root.path().join("spawned").display().to_string()),
            ..Spawn::default()
        },
        Some(&caller(&lead)),
    )
    .await;
    let (b, m) = out.unwrap();
    assert!(b.handle.starts_with("tmp-") && b.is_temporary());
    assert_eq!(b.idle_ttl_secs, Some(600));
    assert_eq!(m.from_kind, "bot");
    assert_eq!(m.from_ref, lead.id);
    assert_eq!(pending(&st, &b.id).await.len(), 1);
    assert!(root.path().join("spawned").is_dir());
}

#[tokio::test]
async fn idle_temporary_bots_are_archived_but_busy_ones_stay() {
    let (root, st) = fixture().await;
    let b = create(
        &st,
        NewBot {
            handle: "tmp-a".into(),
            duty: Some("x".into()),
            lifetime: Some("temporary".into()),
            idle_ttl_secs: Some(60),
            home_path: Some(root.path().join("tmp-a").display().to_string()),
            ..NewBot::default()
        },
    )
    .await
    .unwrap();
    dispatch::retire_idle(&st).await;
    assert_eq!(all(&st).await.len(), 1, "刚建的还没到期");
    sqlx::query("UPDATE bots SET last_active_at = ?2 WHERE id = ?1")
        .bind(&b.id)
        .bind((Utc::now() - chrono::Duration::seconds(120)).to_rfc3339())
        .execute(st.db.pool())
        .await
        .unwrap();
    send(
        &st,
        &b.id,
        NewMail {
            body: "还有活".into(),
            ..NewMail::default()
        },
    )
    .await
    .unwrap();
    dispatch::retire_idle(&st).await;
    assert_eq!(all(&st).await.len(), 1, "有待处理邮件不归档");
    sqlx::query("UPDATE bot_mail SET status = 'done' WHERE to_bot = ?1")
        .bind(&b.id)
        .execute(st.db.pool())
        .await
        .unwrap();
    dispatch::retire_idle(&st).await;
    assert!(all(&st).await.is_empty(), "闲置到期归档");
    assert_eq!(archived(&st).await.len(), 1);
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inbox WHERE title LIKE '临时 bot:tmp-a%'")
            .fetch_one(st.db.pool())
            .await
            .unwrap();
    assert_eq!(n, 1);
}
