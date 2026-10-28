use super::*;

mod glance;
mod history;
mod install;

pub use glance::workspace_glance;
pub use history::{EventsQuery, HistoryQuery, session_events, workspace_history};
use install::{ensure_installed, install_agent};

#[derive(Debug, Default, Deserialize, Serialize)]
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
    pub interject: bool,

    #[serde(default)]
    pub retry_thread: Option<String>,

    #[serde(default)]
    pub account: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_accounts: Vec<String>,

    #[serde(skip)]
    pub bot: Option<String>,
}

const MAX_STORED_REQUEST: usize = 2 * 1024 * 1024;

const INHERITED_API_ENV: [&str; 3] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
];

pub(crate) fn with_editor_context(text: &str, file: Option<&str>) -> String {
    let file = file
        .map(str::trim)
        .filter(|f| !f.is_empty() && f.chars().count() <= 400 && !f.chars().any(char::is_control));
    match file {
        Some(f) if !is_slash_command(text) => {
            format!(
                "{text}\n\n(I have `{f}` open in my editor, for reference only; ignore it if it's unrelated.)"
            )
        }
        _ => text.to_owned(),
    }
}

pub(crate) fn check_images(images: &[blazar_runtime::ImageInput]) -> Result<(), String> {
    const TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
    if images.len() > 8 {
        return Err("At most 8 images per message".into());
    }
    for i in images {
        if !TYPES.contains(&i.media_type.as_str()) {
            return Err(format!("Unsupported image type {}", i.media_type));
        }

        if i.data.len() > 5 * 1024 * 1024 * 4 / 3 + 16 {
            return Err("Each image must be 5MB or smaller".into());
        }
        if !i
            .data
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        {
            return Err("Image data is not valid base64".into());
        }
    }
    Ok(())
}

fn apply_profile(spec: &mut SessionSpec, p: &crate::agents::Agent, caps: crate::library::Caps) {
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

    for (k, v) in caps.env {
        spec.env.entry(k).or_insert(v);
    }
    spec.mcp_servers.extend(caps.mcp);
    if p.runtime == "claude" {
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

fn apply_runtime_config(spec: &mut SessionSpec, c: &AgentConfigView) {
    for (k, v) in &c.custom_env {
        spec.env.entry(k.clone()).or_insert_with(|| v.clone());
    }
    if spec.model.is_none() {
        spec.model = c.model.clone();
    }
    if spec.permission_mode.is_none() {
        spec.permission_mode = c.permission_mode.clone();
    }

    spec.extra_args.splice(0..0, c.custom_args.iter().cloned());
}

pub async fn prompt(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<PromptRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let transports = st.clone();
    prompt_using(st, id, req, move |node| transports.transport(node)).await
}

pub(crate) async fn prompt_for_bot(
    st: Shared,
    id: String,
    req: PromptRequest,
) -> Result<serde_json::Value, String> {
    let transports = st.clone();
    prompt_using(st, id, req, move |node| transports.transport(node))
        .await
        .map(|Json(v)| v)
        .map_err(|e| e.message())
}

async fn prompt_using(
    st: Shared,
    id: String,
    req: PromptRequest,
    transport_for: impl Fn(&str) -> Arc<dyn blazar_transport::NodeTransport> + Send + Sync + 'static,
) -> ApiResult<Json<serde_json::Value>> {
    tokio::spawn(prompt_inner(st, id, req, transport_for))
        .await
        .map_err(ApiError::from)?
}

async fn prompt_inner(
    st: Shared,
    id: String,
    req: PromptRequest,
    transport_for: impl Fn(&str) -> Arc<dyn blazar_transport::NodeTransport> + Send + Sync,
) -> ApiResult<Json<serde_json::Value>> {
    for key in req.env.keys() {
        blazar_transport::validate_env_key(key)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
    }
    let (node, path) = locate(&st, &id).await?;
    let workspace_id = WorkspaceId(id.parse()?);
    let assistant = super::assistant::is_assistant(&st, &id).await;
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
            if let Some((sid, thread)) = crate::run::send_to_idle(
                &st,
                workspace_id,
                req.resume_session.as_deref().filter(|s| !s.is_empty()),
                &req.text,
                &with_editor_context(&req.text, req.context_file.as_deref()),
                &req.images,
            )
            .await
            {
                return Ok(Json(serde_json::json!({
                    "session_id": sid.to_string(),
                    "thread_id": thread,
                    "resumed": true,
                    "interjected": true,
                    "node": node,
                    "activity": { "started": true },
                    "interactive": true,
                })));
            }
            let running_session = st
                .running
                .read()
                .await
                .iter()
                .find(|(_, r)| r.workspace_id == workspace_id)
                .map(|(sid, r)| (*sid, r.live.as_ref().is_some_and(|l| l.interactive)));
            if req.interject
                && let Some((sid, true)) = running_session
                && let Some(thread) = sqlx::query_scalar::<_, String>(
                    "SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1",
                )
                .bind(sid.to_string())
                .fetch_optional(st.db.pool())
                .await?
                && req
                    .resume_session
                    .as_deref()
                    .is_none_or(|t| t.is_empty() || t == thread)
            {
                let sent = with_editor_context(&req.text, req.context_file.as_deref());
                return Ok(Json(
                    match crate::run::interject(&st, sid, &req.text, &sent, &req.images).await {
                        Ok(_) => serde_json::json!({
                            "session_id": sid.to_string(),
                            "thread_id": thread,
                            "resumed": true,
                            "interjected": true,
                            "node": node,
                            "activity": { "started": true },
                            "interactive": true,
                        }),
                        Err(why) => serde_json::json!({
                            "admitted": false,
                            "reason": why,
                            "running_session_id": sid.to_string(),
                            "running_interactive": true,
                        }),
                    },
                ));
            }
            let running_session =
                running_session.map(|(sid, interactive)| (sid.to_string(), interactive));
            return Ok(Json(serde_json::json!({
                "admitted": false,
                "reason": reason,
                "hint": "Interrupt the running session first, or wait for it to finish",
                "running_session_id": running_session.as_ref().map(|x| x.0.clone()),
                "running_interactive": running_session.is_some_and(|x| x.1),
            })));
        }
    };

    let now = Utc::now().to_rfc3339();

    type Prior = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        String,
    );
    let prior: Option<Prior> = if req.resume {
        sqlx::query_as(
            "SELECT COALESCE(thread_id, id) AS thread, provider_session_id, last_uuid, account_id, runtime_kind,
                    COALESCE(node, ?2) AS node
             FROM sessions
             WHERE workspace_id = ?1 AND provider_session_id IS NOT NULL
               AND rewound_at IS NULL
               AND (?3 IS NULL OR COALESCE(thread_id, id) = ?3)
               AND EXISTS (SELECT 1 FROM events e WHERE e.session_id = sessions.id
                           AND e.payload LIKE '{\"type\":\"session_started\"%')
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
    let profile = match req.profile.as_deref().filter(|p| !p.is_empty()) {
        Some(pid) => {
            let p = crate::agents::get(&st, pid).await.ok_or_else(|| {
                ApiError(anyhow::anyhow!("No such agent (it may have been deleted)"))
            })?;
            if p.archived_at.is_some() {
                return Err(ApiError(anyhow::anyhow!(
                    "Agent \"{}\" is archived; restore it to use it",
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

    let switched = prior
        .as_ref()
        .is_some_and(|p| p.4 != agent_id || p.5 != run_node);
    let resume_id = prior
        .as_ref()
        .filter(|_| !switched)
        .and_then(|p| p.1.clone());
    let resume_at = prior
        .as_ref()
        .filter(|_| req.resume_at_last && resume_id.is_some())
        .and_then(|p| p.2.clone());

    if local_brain && !blazar_runtime::supports_remote_hands(agent_id) {
        return Err(ApiError(anyhow::anyhow!(
            "{} only works in local workspaces: it can't send files and commands to {node}",
            blazar_runtime::spec::find(agent_id).map_or(agent_id, |s| s.label)
        )));
    }

    let want_model = req
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| profile.as_ref().and_then(|p| p.model.clone()));
    let account = crate::accounts::resolve(
        &st,
        agent_id,
        &run_node,
        req.account
            .as_deref()
            .or_else(|| profile.as_ref().and_then(|p| p.account.as_deref())),
        prior
            .as_ref()
            .filter(|_| !switched)
            .and_then(|p| p.3.as_deref()),
        want_model.as_deref(),
        &req.exclude_accounts,
    )
    .await
    .map_err(|e| ApiError(anyhow::anyhow!(e)))?;
    let proxy_env = match account.as_ref().filter(|a| a.proxy) {
        Some(a) => {
            let port = crate::proxy::ensure_tunnel(&st, &run_node)
                .await
                .map_err(|e| ApiError(anyhow::anyhow!(e)))?;
            let secret = crate::proxy::secret(&st, &a.id)
                .await
                .map_err(|e| ApiError(anyhow::anyhow!(e)))?;
            Some((port, a.id.clone(), secret))
        }
        None => None,
    };
    let stored_request = serde_json::to_string(&req)
        .ok()
        .filter(|s| s.len() <= MAX_STORED_REQUEST);

    let session_id = SessionId::new();

    let named_thread: Option<String> = match req.resume_session.as_deref().filter(|s| !s.is_empty())
    {
        Some(t) if req.resume => {
            sqlx::query_scalar(
                "SELECT COALESCE(thread_id, id) FROM sessions
             WHERE workspace_id = ?1 AND COALESCE(thread_id, id) = ?2 LIMIT 1",
            )
            .bind(&id)
            .bind(t)
            .fetch_optional(st.db.pool())
            .await?
        }
        _ => None,
    };
    let thread_id = match (&prior, resume_id.is_some() || switched) {
        (Some(p), true) => p.0.clone(),
        _ => req
            .retry_thread
            .clone()
            .filter(|t| !t.is_empty())
            .or(named_thread)
            .unwrap_or_else(|| session_id.to_string()),
    };
    sqlx::query(
        "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, agent_profile, thread_id,
                               account_id, account_auto, request)
         VALUES (?1, ?2, ?4, 'running', ?3, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(session_id.to_string())
    .bind(&id)
    .bind(&now)
    .bind(agent_id)
    .bind(profile.as_ref().map(|p| p.id.clone()))
    .bind(&thread_id)
    .bind(account.as_ref().map(|a| a.id.clone()))
    .bind(i64::from(account.as_ref().is_some_and(|a| a.auto)))
    .bind(&stored_request)
    .execute(st.db.pool())
    .await?;

    let mut attempted: Option<crate::run::Ctx> = None;
    let result: ApiResult<Json<serde_json::Value>> = async {
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
            Some(transport_for(&node)),
        )
        .await
        {
            fail_start(&st, session_id, workspace_id, &reason).await?;
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
        let handover = match prior.as_ref().filter(|_| switched) {
            Some(p) => {
                crate::chat::preface(
                    &st,
                    &p.0,
                    &now,
                    "Earlier turns were handled by another AI assistant; the workspace files reflect its changes.",
                )
                .await
            }
            None => String::new(),
        };
        let mut spec = SessionSpec::new(
            &cwd,
            format!(
                "{handover}{}",
                with_editor_context(&req.text, req.context_file.as_deref())
            ),
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
        spec.env_unset
            .extend(INHERITED_API_ENV.iter().map(|k| (*k).to_owned()));
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
        let bot = match req.bot.as_deref() {
            Some(b) => crate::bots::get(&st, b).await,
            None => crate::bots::by_home(&st, &id).await,
        };
        let caller = bot.as_ref().map(|b| crate::bots::Caller {
            bot: b.id.clone(),
            handle: b.handle.clone(),
            role: b.role.clone(),
            session: Some(session_id.to_string()),
        });
        let children = crate::fleet::children_of(&st, &id).await;
        match crate::fleet::mcp_spec(
            &st,
            session_id,
            &run_node,
            assistant || !children.is_empty() || bot.is_some(),
            caller,
        )
        .await
        {
            Ok(Some(m)) => spec.mcp_servers.push(m),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                target: "blazar::fleet",
                "{run_node} 的调度回程没打通，这轮不挂调度工具: {error}"
            ),
        }

        if let Some(p) = &profile {
            let caps = crate::library::for_agent(&st, &p.id).await;
            apply_profile(&mut spec, p, caps);
        }
        if let Some(c) = &cfg {
            apply_runtime_config(&mut spec, c);
        }
        let mut brief = if assistant {
            super::assistant::briefing(&st).await
        } else {
            String::new()
        };
        let kids = crate::fleet::briefing(&id, &children);
        if !kids.is_empty() {
            if !brief.is_empty() {
                brief.push_str("\n\n");
            }
            brief.push_str(&kids);
        }
        if let Some(b) = &bot {
            let block = crate::bots::briefing(&st, b).await;
            if !brief.is_empty() {
                brief.push_str("\n\n");
            }
            brief.push_str(&block);
            spec.mcp_servers.extend(crate::bots::hands(&st, b).await);
        }
        if !brief.is_empty() {
            spec.instructions = Some(match spec.instructions.take() {
                Some(i) => format!("{i}\n\n{brief}"),
                None => brief,
            });
        }
        if let Some((k, v)) = account.as_ref().and_then(|a| a.env.clone()) {
            spec.env.entry(k).or_insert(v);
        }
        if let Some((k, p)) = account.as_ref().and_then(|a| a.token_file.clone())
            && !spec.env.contains_key(&k)
        {
            spec.env_files.insert(k, p);
        }
        if run_node != "local" {
            for (k, v) in crate::proxy::node_net_env(&st, &run_node).await {
                spec.env.entry(k).or_insert(v);
            }
        }
        if let Some((port, account_id, secret)) = proxy_env {
            if agent_id == "codex" {
                let remote =
                    crate::codex_proxy::prepare_remote(&st, &run_node, &account_id, port)
                        .await
                        .map_err(|e| ApiError(anyhow::anyhow!(e)))?;
                spec.env.extend(remote.env);
                spec.extra_args.extend(remote.args);
            } else {
                spec.env
                    .insert("ANTHROPIC_BASE_URL".into(), format!("http://127.0.0.1:{port}"));
                spec.env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), secret);
                spec.env.remove("ANTHROPIC_API_KEY");
                spec.env.remove("ANTHROPIC_AUTH_TOKEN");
            }
            crate::proxy::bypass_loopback(&mut spec.env);
        }
        if local_brain {
            let exe = std::env::current_exe()
                .map_err(|e| ApiError(anyhow::anyhow!("Can't find this program's path to attach remote tools: {e}")))?;
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

        let mut runtime = CliRuntime::by_id(transport_for(&run_node), agent_id)
            .ok_or_else(|| ApiError(anyhow::anyhow!("Unknown agent: {agent_id}")))?;

        if let Some(p) = cfg.as_ref().and_then(|c| c.program_path.clone()) {
            runtime = runtime.with_program(p);
        } else if agent_id == "dsh"
            && run_node == "local"
            && let Some(js) = dsh_entry()
        {
            runtime = runtime.with_program(js);
        }
        let needs_install = cfg.as_ref().and_then(|c| c.program_path.as_ref()).is_none()
            && run_node != "local"
            && ensure_installed(&st, &run_node, agent_id).await?;

        let interactive = runtime.interactive();
        let app_server = runtime.app_server();
        let first_text = spec.prompt.clone();
        let first_images = spec.images.clone();
        let resume_pid = resume_id.clone().map(ProviderSessionId);
        let new_pid = resume_pid
            .is_none()
            .then(crate::run::new_provider_session_id);
        let plan = runtime.detached_plan(
            &spec,
            resume_pid.as_ref(),
            new_pid.as_ref().map(|p| p.0.as_str()),
        );

        if st.services == crate::services::Services::Interactive
            && let Some(snap) = crate::checkpoint::snapshot(&st, &node, &path).await
        {
            crate::checkpoint::record(&st, &id, snap, Some(&session_id.to_string()), Some(1), "").await;
        }
        let transport = transport_for(&run_node);
        let root = resolve_run_root(transport.clone(), &run_node).await?;
        let run = blazar_transport::detached::DetachedRun::attach(
            transport.clone(), root.join(session_id.to_string()).display().to_string(), session_id.to_string(),
        );
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
        let mut next_seq = 2;
        if needs_install {
            install_agent(&st, session_id, workspace_id, &run_node, agent_id, &mut next_seq).await?;
        }

        let live = Arc::new(crate::run::Live {
            run,
            interactive,
            state: tokio::sync::Mutex::new(crate::run::LiveState {
                next_seq,

                pending_user: u32::from(interactive && !is_slash_command(&req.text)),
                user_no: 1,
                eof_sent: false,
                interrupt_requested: false,
                bg: std::collections::HashSet::new(),
                idle: false,
                app: crate::run::AppTurn {
                    enabled: app_server,
                    first: app_server.then(|| (first_text.clone(), first_images.clone())),
                    ..crate::run::AppTurn::default()
                },
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
        attempted = Some(ctx.clone());
        blazar_transport::detached::DetachedRun::launch_in(
            transport,
            &root,
            &session_id.to_string(),
            &plan.spec,
            plan.first_input.as_deref(),
            plan.mode,
        )
        .await
        .map_err(|e| ApiError(anyhow::anyhow!("Failed to start agent on {run_node}: {e}")))?;
        crate::tasks::on_run_started(&st, &thread_id).await;
        crate::inbox::resolve_asks(&st, &thread_id).await;
        st.services
            .spawn(crate::titles::maybe_title(st.clone(), session_id, workspace_id));
        crate::run::register(ctx, 0, Some(started_tx)).await;
        attempted = None;

        let activity = if req.wait_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(req.wait_secs), started_rx).await
            {
                Ok(Ok(Ok(()))) => serde_json::json!({ "started": true }),
                Ok(Ok(Err(msg))) => serde_json::json!({
                    "started": false, "failed": true, "reason": msg,
                    "failure_class": blazar_core_types::FailureClass::classify(&msg),
                }),

                Ok(Err(_)) => serde_json::json!({
                    "started": false, "stalled": true, "reason": "The session ended before producing any events",
                }),
                Err(_) => serde_json::json!({
                    "started": false, "stalled": true,
                    "reason": format!("No agent activity within {} seconds (check that the CLI is installed, credentials, and network egress)",
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
            "account": account.as_ref().map(|a| a.id.clone()),
            "run_node": run_node,
            "activity": activity,

            "interactive": interactive,
        })))
    }
    .await;
    if let Err(error) = result {
        if let Some(ctx) = attempted
            && let Err(cleanup) = ctx.live.run.hard_kill().await
        {
            crate::run::register(ctx, 0, None).await;
            return Err(ApiError(anyhow::anyhow!(
                "{}; failed to stop the starting process: {cleanup}. The run record is kept and still being checked; don't start it again",
                error.message(),
            )));
        }
        if let Err(compensation) = fail_start(&st, session_id, workspace_id, &error.message()).await
        {
            return Err(ApiError(anyhow::anyhow!(
                "{}; failed to save the start failure: {}",
                error.message(),
                compensation.message()
            )));
        }
        return Err(error);
    }
    result
}

async fn resolve_run_root(
    transport: Arc<dyn blazar_transport::NodeTransport>,
    node: &str,
) -> ApiResult<std::path::PathBuf> {
    let output = transport
        .exec(
            blazar_transport::ExecSpec::new("bash")
                .arg("-lc")
                .arg("printf '__BLAZAR_RUNS_ROOT__ %s\\n' \"$HOME/.blazar/runs\""),
        )
        .await?
        .ok()?;
    let root = output
        .lines()
        .find_map(|line| line.strip_prefix("__BLAZAR_RUNS_ROOT__ "))
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            ApiError(anyhow::anyhow!(
                "Couldn't determine the run directory on {node}"
            ))
        })?;
    Ok(root)
}

async fn fail_start(st: &Shared, sid: SessionId, ws: WorkspaceId, message: &str) -> ApiResult<()> {
    let mut tx = st.db.pool().begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE sessions SET status = 'failed' WHERE id = ?1")
        .bind(sid.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE workspaces SET activity = 'idle' WHERE id = ?1")
        .bind(ws.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let entry = NormalizedEntry {
        seq: st.db.max_seq(sid).await? + 1,
        ts: Utc::now(),
        parent_tool_use_id: None,
        kind: blazar_core_types::EntryKind::Finished(blazar_core_types::Outcome::Failed {
            message: message.to_owned(),
        }),
    };
    st.db.append_event(sid, ws, &entry).await?;
    crate::failures::record(st, sid).await;
    st.emit(ServerEvent::Entry {
        workspace_id: ws,
        session_id: sid,
        entry: Box::new(entry),
    });
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(())
}

pub(crate) fn brain_dir(st: &Shared, workspace: &str) -> ApiResult<String> {
    let base = st
        .mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    let dir = base.join("brain").join(workspace);
    std::fs::create_dir_all(&dir).map_err(|e| {
        ApiError(anyhow::anyhow!(
            "Failed to create the local placeholder directory: {e}"
        ))
    })?;
    Ok(dir.display().to_string())
}

#[cfg(test)]
#[path = "sessions_configuration_tests.rs"]
mod configuration_tests;

#[cfg(test)]
#[path = "sessions_start_tests.rs"]
mod startup_tests;
