use super::*;

#[derive(Debug, Deserialize)]
pub struct PromptRequest {
    pub text: String,

    #[serde(default)]
    pub resume: bool,

    #[serde(default)]
    pub resume_session: Option<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,

    #[serde(default)]
    pub permission_mode: Option<String>,

    #[serde(default)]
    pub agent: Option<String>,

    #[serde(default)]
    pub wait_secs: u64,

    #[serde(default)]
    pub brain: Option<String>,

    #[serde(default)]
    pub profile: Option<String>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub effort: Option<String>,

    #[serde(default)]
    pub images: Vec<blazar_runtime::ImageInput>,

    #[serde(default)]
    pub output_style: Option<String>,

    #[serde(default)]
    pub fast_mode: Option<bool>,

    #[serde(default)]
    pub thinking: Option<bool>,

    #[serde(default)]
    pub context_file: Option<String>,

    #[serde(default)]
    pub resume_at_last: bool,

    #[serde(default)]
    pub retry_thread: Option<String>,
}

pub(crate) fn with_editor_context(text: &str, file: Option<&str>) -> String {
    let file = file
        .map(str::trim)
        .filter(|f| !f.is_empty() && f.chars().count() <= 400 && !f.chars().any(char::is_control));
    match file {
        Some(f) if !is_slash_command(text) => {
            format!("{text}\n\n（我在编辑器里正打开着 `{f}`，仅供参考；和问题无关就忽略。）")
        }
        _ => text.to_owned(),
    }
}

pub(crate) fn check_images(images: &[blazar_runtime::ImageInput]) -> Result<(), String> {
    const TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
    if images.len() > 8 {
        return Err("一次最多 8 张图片".into());
    }
    for i in images {
        if !TYPES.contains(&i.media_type.as_str()) {
            return Err(format!("不支持的图片类型 {}", i.media_type));
        }

        if i.data.len() > 5 * 1024 * 1024 * 4 / 3 + 16 {
            return Err("单张图片不能超过 5MB".into());
        }
        if !i
            .data
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        {
            return Err("图片数据不是合法的 base64".into());
        }
    }
    Ok(())
}

pub async fn prompt(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<PromptRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let (node, path) = locate(&st, &id).await?;
    let workspace_id = WorkspaceId(id.parse()?);
    if let Err(e) = check_images(&req.images) {
        return Err(ApiError(anyhow::anyhow!(e)));
    }

    let local_brain = node != "local" && req.brain.as_deref() != Some("node");
    let run_node = if local_brain {
        "local".to_owned()
    } else {
        node.clone()
    };

    let _admit = match st.admit(workspace_id).await {
        Ok(g) => g,
        Err(reason) => {
            let running_session = st
                .running
                .read()
                .await
                .iter()
                .find(|(_, r)| r.workspace_id == workspace_id)
                .map(|(sid, r)| {
                    (
                        sid.to_string(),
                        r.live.as_ref().is_some_and(|l| l.interactive),
                    )
                });
            return Ok(Json(serde_json::json!({
                "admitted": false,
                "reason": reason,
                "hint": "先中断正在跑的那个会话，或等它结束",
                "running_session_id": running_session.as_ref().map(|x| x.0.clone()),
                "running_interactive": running_session.is_some_and(|x| x.1),
            })));
        }
    };

    let now = Utc::now().to_rfc3339();

    let prior: Option<(String, Option<String>, Option<String>)> = if req.resume {
        sqlx::query_as(
            "SELECT COALESCE(thread_id, id) AS thread, provider_session_id, last_uuid FROM sessions
             WHERE workspace_id = ?1 AND provider_session_id IS NOT NULL
               AND rewound_at IS NULL
               AND COALESCE(node, ?2) = ?2
               AND (?3 IS NULL OR COALESCE(thread_id, id) = ?3)
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&id)
        .bind(&run_node)
        .bind(req.resume_session.as_deref().filter(|s| !s.is_empty()))
        .fetch_optional(st.db.pool())
        .await?
    } else {
        None
    };
    let resume_id = prior.as_ref().and_then(|(_, p, _)| p.clone());
    let resume_at = prior
        .as_ref()
        .filter(|_| req.resume_at_last && resume_id.is_some())
        .and_then(|(_, _, u)| u.clone());

    let profile = match req.profile.as_deref().filter(|p| !p.is_empty()) {
        Some(pid) => {
            let p = crate::agents::get(&st, pid)
                .await
                .ok_or_else(|| ApiError(anyhow::anyhow!("没有这个 Agent（可能已被删除）")))?;
            if p.archived_at.is_some() {
                return Err(ApiError(anyhow::anyhow!(
                    "Agent「{}」已归档，恢复后才能用",
                    p.name
                )));
            }
            Some(p)
        }
        None => None,
    };
    let agent_id: String = profile
        .as_ref()
        .map(|p| p.runtime.clone())
        .or_else(|| req.agent.clone())
        .unwrap_or_else(|| "claude".into());
    let agent_id = agent_id.as_str();

    if local_brain && !blazar_runtime::supports_remote_hands(agent_id) {
        return Err(ApiError(anyhow::anyhow!(
            "{} 只能用于本机工作区：它没法把文件与命令送到 {node} 上执行",
            blazar_runtime::spec::find(agent_id).map_or(agent_id, |s| s.label)
        )));
    }

    let session_id = SessionId::new();

    let thread_id = match (&prior, resume_id.is_some()) {
        (Some((t, _, _)), true) => t.clone(),
        _ => req
            .retry_thread
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| session_id.to_string()),
    };
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, agent_profile, thread_id)
         VALUES (?1, ?2, ?4, 'running', ?3, ?5, ?6)",
    )
    .bind(session_id.to_string())
    .bind(&id)
    .bind(&now)
    .bind(agent_id)
    .bind(profile.as_ref().map(|p| p.id.clone()))
    .bind(&thread_id)
    .execute(st.db.pool())
    .await?;

    crate::tasks::on_run_started(&st, &thread_id).await;

    let hook_ctx = blazar_hooks::HookContext {
        workspace: id.clone(),
        node: node.clone(),
        cwd: path.clone(),
        session_id: session_id.to_string(),
        tool_name: None,
        extra: Default::default(),
    };
    if let Some(reason) = fire_hooks(
        &st,
        blazar_hooks::HookEvent::TurnStart,
        hook_ctx.clone(),
        Some(st.transport(&node)),
    )
    .await
    {
        let entry = NormalizedEntry {
            seq: 1,
            ts: Utc::now(),
            parent_tool_use_id: None,
            kind: blazar_core_types::EntryKind::Finished(blazar_core_types::Outcome::Failed {
                message: reason.clone(),
            }),
        };
        st.db.append_event(session_id, workspace_id, &entry).await?;
        st.emit(ServerEvent::Entry {
            workspace_id,
            session_id,
            entry: Box::new(entry),
        });
        st.emit(ServerEvent::WorkspacesChanged);
        return Ok(Json(serde_json::json!({
            "session_id": session_id.to_string(),
            "blocked_by_hook": reason,
        })));
    }

    let cfg = effective_agent_config(&st, &run_node, agent_id).await;

    let cwd = if local_brain {
        brain_dir(&st, &id)?
    } else {
        path.clone()
    };
    let mut spec = SessionSpec::new(
        &cwd,
        with_editor_context(&req.text, req.context_file.as_deref()),
    );

    let mut env: std::collections::BTreeMap<String, String> = if local_brain {
        Default::default()
    } else {
        task_env_of(&st, &id)
            .await
            .ok()
            .flatten()
            .map(|te| te.env_vars())
            .unwrap_or_default()
    };
    env.extend(req.env);
    spec.env = env;
    spec.disallowed_tools = req.disallowed_tools;
    spec.permission_mode = req.permission_mode;

    spec.model = req.model.filter(|m| {
        !m.is_empty()
            && m.len() <= 80
            && m.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.:[]/".contains(c))
    });
    spec.effort = req
        .effort
        .filter(|e| blazar_runtime::cli_runtime::EFFORTS.contains(&e.as_str()));
    spec.images = req.images.clone();
    spec.output_style = req
        .output_style
        .clone()
        .filter(|s| blazar_runtime::valid_style(s.trim()));
    spec.fast_mode = req.fast_mode;
    spec.thinking = req.thinking;
    spec.resume_at = resume_at;

    if let Some(m) = crate::office::mcp_spec(&st).await {
        spec.mcp_servers.push(m);
    }

    if let Some(p) = &profile {
        for (k, v) in &p.env {
            spec.env.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if spec.model.is_none() {
            spec.model = p.model.clone();
        }
        if spec.permission_mode.is_none() {
            spec.permission_mode = p.permission_mode.clone();
        }
        if spec.effort.is_none() {
            spec.effort = p.thinking_level.clone();
        }
        spec.instructions = Some(p.instructions.clone()).filter(|s| !s.trim().is_empty());

        spec.extra_args.extend(p.custom_args.iter().cloned());

        let caps = crate::library::for_agent(&st, &p.id).await;
        for (k, v) in caps.env {
            spec.env.entry(k).or_insert(v);
        }
        spec.mcp_servers = caps.mcp;
        if agent_id == "claude" {
            spec.add_dirs.extend(caps.skill_root);
        } else {
            let brief = crate::library::skills_briefing(&caps.skills);
            if !brief.is_empty() {
                spec.instructions = Some(match spec.instructions.take() {
                    Some(i) => format!("{i}\n\n{brief}"),
                    None => brief,
                });
            }
        }
    }

    if let Some(c) = &cfg {
        for (k, v) in &c.custom_env {
            spec.env.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if spec.model.is_none() {
            spec.model = c.model.clone();
        }
        if spec.permission_mode.is_none() {
            spec.permission_mode = c.permission_mode.clone();
        }

        spec.extra_args = c.custom_args.clone();
    }
    if local_brain {
        let exe = std::env::current_exe()
            .map_err(|e| ApiError(anyhow::anyhow!("找不到本程序路径，无法挂远端工具: {e}")))?;
        spec.remote_hands = Some(blazar_runtime::RemoteHands {
            node: node.clone(),
            root: path.clone(),
            command: exe.display().to_string(),
            args: vec![
                blazar_mcp::remote::SUBCOMMAND.to_owned(),
                "--node".into(),
                node.clone(),
                "--root".into(),
                path.clone(),
            ],
        });
    }

    let mut runtime = CliRuntime::by_id(st.transport(&run_node), agent_id)
        .ok_or_else(|| ApiError(anyhow::anyhow!("未知 agent: {agent_id}")))?;

    if let Some(p) = cfg.as_ref().and_then(|c| c.program_path.clone()) {
        runtime = runtime.with_program(p);
    } else if agent_id == "dsh"
        && run_node == "local"
        && let Some(js) = dsh_entry()
    {
        runtime = runtime.with_program(js);
    }

    let interactive = runtime.interactive();
    let resume_pid = resume_id.clone().map(ProviderSessionId);
    let new_pid = resume_pid
        .is_none()
        .then(crate::run::new_provider_session_id);
    let plan = runtime.detached_plan(
        &spec,
        resume_pid.as_ref(),
        new_pid.as_ref().map(|p| p.0.as_str()),
    );

    if let Some(snap) = crate::checkpoint::snapshot(&st, &node, &path).await {
        crate::checkpoint::record(&st, &id, snap, Some(&session_id.to_string()), Some(1), "").await;
    }
    let transport = st.transport(&run_node);
    let run = blazar_transport::detached::DetachedRun::launch(
        transport,
        &session_id.to_string(),
        &plan.spec,
        plan.first_input.as_deref(),
        plan.mode,
    )
    .await
    .map_err(|e| ApiError(anyhow::anyhow!("在 {run_node} 上启动 agent 失败: {e}")))?;
    sqlx::query(
        "UPDATE sessions SET run_dir = ?1, node = ?2, interactive = ?3,
                provider_session_id = COALESCE(?4, provider_session_id)
         WHERE id = ?5",
    )
    .bind(&run.dir)
    .bind(&run_node)
    .bind(i64::from(interactive))
    .bind(new_pid.as_ref().map(|p| p.0.clone()))
    .bind(session_id.to_string())
    .execute(st.db.pool())
    .await?;

    let user_entry = NormalizedEntry {
        seq: 1,
        ts: Utc::now(),
        parent_tool_use_id: None,
        kind: blazar_core_types::EntryKind::UserMessage {
            text: with_image_note(&req.text, req.images.len()),
        },
    };
    st.db
        .append_event(session_id, workspace_id, &user_entry)
        .await?;
    st.emit(ServerEvent::Entry {
        workspace_id,
        session_id,
        entry: Box::new(user_entry),
    });

    let live = Arc::new(crate::run::Live {
        run,
        interactive,
        state: tokio::sync::Mutex::new(crate::run::LiveState {
            next_seq: 2,

            pending_user: u32::from(interactive && !is_slash_command(&req.text)),
            user_no: 1,
            eof_sent: false,
            interrupt_requested: false,
        }),
    });

    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let ctx = crate::run::Ctx {
        st: st.clone(),
        sid: session_id,
        ws: workspace_id,
        node: run_node.clone(),
        live: live.clone(),
        runtime: Arc::new(runtime),
        hook: hook_ctx,
    };
    let task = tokio::spawn(crate::run::supervise(ctx, 0, Some(started_tx)));
    st.running.write().await.insert(
        session_id,
        RunningSession {
            workspace_id,
            task,
            killer: None,
            live: Some(live),
        },
    );

    let activity = if req.wait_secs > 0 {
        match tokio::time::timeout(std::time::Duration::from_secs(req.wait_secs), started_rx).await
        {
            Ok(Ok(Ok(()))) => serde_json::json!({ "started": true }),
            Ok(Ok(Err(msg))) => serde_json::json!({
                "started": false, "failed": true, "reason": msg,
                "failure_class": blazar_core_types::FailureClass::classify(&msg),
            }),

            Ok(Err(_)) => serde_json::json!({
                "started": false, "stalled": true, "reason": "会话在产出任何事件前就结束了",
            }),
            Err(_) => serde_json::json!({
                "started": false, "stalled": true,
                "reason": format!("{} 秒内未观察到 agent 活动（检查 CLI 是否安装、凭据与网络出口）",
                                  req.wait_secs),
            }),
        }
    } else {
        serde_json::json!({ "started": null })
    };

    Ok(Json(serde_json::json!({
        "session_id": session_id.to_string(),
        "thread_id": thread_id,
        "resumed": resume_id.is_some(),
        "node": node,

        "brain": if local_brain { "local" } else { "node" },
        "run_node": run_node,
        "activity": activity,

        "interactive": interactive,
    })))
}

pub(crate) fn brain_dir(st: &Shared, workspace: &str) -> ApiResult<String> {
    let base = st
        .mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    let dir = base.join("brain").join(workspace);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError(anyhow::anyhow!("创建本机占位目录失败: {e}")))?;
    Ok(dir.display().to_string())
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    pub after: u64,
}

pub async fn session_events(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Vec<NormalizedEntry>>> {
    let sid = SessionId(id.parse()?);
    Ok(Json(st.db.events_since(sid, q.after).await?))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub session: Option<String>,
}

pub async fn workspace_history(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let one = q.session.filter(|s| !s.is_empty());
    let rows = sqlx::query(
        "SELECT e.seq, e.ts, e.payload, e.session_id, e.parent_tool_id, s.rewound_at
         FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE s.workspace_id = ?1 AND (?2 IS NULL OR COALESCE(s.thread_id, s.id) = ?2)
         ORDER BY s.created_at, e.seq",
    )
    .bind(&id)
    .bind(one)
    .fetch_all(st.db.pool())
    .await?;

    Ok(Json(
        rows.into_iter()
            .filter_map(|r| {
                let payload: String = r.try_get("payload").ok()?;
                let kind: serde_json::Value = serde_json::from_str(&payload).ok()?;
                Some(serde_json::json!({
                    "seq": r.try_get::<i64, _>("seq").unwrap_or(0),
                    "ts": r.try_get::<String, _>("ts").unwrap_or_default(),
                    "session_id": r.try_get::<String, _>("session_id").unwrap_or_default(),
                    "parent": r.try_get::<Option<String>, _>("parent_tool_id").ok().flatten(),

                    "rewound": r.try_get::<Option<String>, _>("rewound_at").ok().flatten().is_some(),
                    "kind": kind,
                }))
            })
            .collect(),
    ))
}
