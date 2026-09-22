use super::*;

#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub event: String,
    pub command: String,
    #[serde(default = "hub_target")]
    pub target: String,
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub blocking: bool,
    #[serde(default = "thirty")]
    pub timeout_secs: i64,
    #[serde(default = "tru")]
    pub enabled: bool,
}

pub(crate) fn hub_target() -> String {
    "hub".into()
}
pub(crate) const fn thirty() -> i64 {
    30
}
pub(crate) const fn tru() -> bool {
    true
}

pub async fn list_hooks(State(st): State<Shared>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let rows = sqlx::query(
        "SELECT id, event, command, target, matcher, blocking, timeout_secs, enabled
         FROM hooks ORDER BY event, created_at",
    )
    .fetch_all(st.db.pool())
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<String,_>("id").unwrap_or_default(),
                    "event": r.try_get::<String,_>("event").unwrap_or_default(),
                    "command": r.try_get::<String,_>("command").unwrap_or_default(),
                    "target": r.try_get::<String,_>("target").unwrap_or_default(),
                    "matcher": r.try_get::<Option<String>,_>("matcher").unwrap_or(None),
                    "blocking": r.try_get::<i64,_>("blocking").unwrap_or(0) != 0,
                    "timeout_secs": r.try_get::<i64,_>("timeout_secs").unwrap_or(30),
                    "enabled": r.try_get::<i64,_>("enabled").unwrap_or(1) != 0,
                })
            })
            .collect(),
    ))
}

pub async fn create_hook(
    State(st): State<Shared>,
    Json(h): Json<HookInput>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO hooks (id, event, command, target, matcher, blocking, timeout_secs,
                            enabled, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
    )
    .bind(&id)
    .bind(&h.event)
    .bind(&h.command)
    .bind(&h.target)
    .bind(&h.matcher)
    .bind(i64::from(h.blocking))
    .bind(h.timeout_secs)
    .bind(i64::from(h.enabled))
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn delete_hook(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM hooks WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn load_hooks(
    st: &AppState,
    event: blazar_hooks::HookEvent,
) -> Vec<blazar_hooks::Hook> {
    let name = serde_json::to_string(&event)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned();
    let rows = sqlx::query(
        "SELECT command, target, matcher, blocking, timeout_secs
         FROM hooks WHERE enabled = 1 AND event = ?1 ORDER BY created_at",
    )
    .bind(&name)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();

    rows.into_iter()
        .map(|r| blazar_hooks::Hook {
            event,
            command: r.try_get("command").unwrap_or_default(),
            target: if r.try_get::<String, _>("target").unwrap_or_default() == "node" {
                blazar_hooks::HookTarget::Node
            } else {
                blazar_hooks::HookTarget::Hub
            },
            matcher: r.try_get("matcher").unwrap_or(None),
            blocking: r.try_get::<i64, _>("blocking").unwrap_or(0) != 0,
            timeout_secs: r.try_get::<i64, _>("timeout_secs").unwrap_or(30) as u64,
            enabled: true,
        })
        .collect()
}

pub(crate) async fn fire_hooks(
    st: &AppState,
    event: blazar_hooks::HookEvent,
    ctx: blazar_hooks::HookContext,
    node: Option<Arc<dyn blazar_transport::NodeTransport>>,
) -> Option<String> {
    let hooks = load_hooks(st, event).await;
    if hooks.is_empty() {
        return None;
    }
    let runner = blazar_hooks::HookRunner::new(hooks, st.transport("local"));
    let report = runner.fire(event, &ctx, node).await;
    for o in &report.outcomes {
        tracing::debug!(target: "blazar::hooks", "钩子 {} → {}", o.command, o.code);
    }
    report.block_reason()
}

pub(crate) async fn local_program(st: &Shared, runtime: &str) -> String {
    effective_agent_config(st, "local", runtime)
        .await
        .and_then(|c| c.program_path)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| runtime.to_owned())
}

pub async fn runtime_models(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    if id == "claude"
        && let Some(cat) = crate::catalog::claude(&st, false).await
        && let Some(list) = cat["models"].as_array().filter(|l| !l.is_empty())
    {
        let models: Vec<serde_json::Value> = list
            .iter()
            .map(|m| {
                let v = m["value"].as_str().unwrap_or_default();
                serde_json::json!({

                    "id": if v == "default" { "" } else { v },
                    "label": m["displayName"].as_str().unwrap_or(v),
                    "desc": m["description"].as_str().unwrap_or_default(),
                    "efforts": m.get("supportedEffortLevels").cloned().unwrap_or(serde_json::json!([])),
                })
            })
            .collect();
        return Json(serde_json::json!({ "runtime": id, "models": models, "source": "cli" }));
    }
    let claude_efforts = ["low", "medium", "high", "xhigh", "max"];
    let models: Vec<serde_json::Value> = match id.as_str() {
        "claude" => [
            ("", "默认", "跟随 Claude Code 的推荐（当前为 Opus）"),
            ("opus", "Opus", "日常与复杂任务"),
            ("fable", "Fable", "最强，适合最难、最长的任务"),
            ("sonnet", "Sonnet", "高效，适合常规任务"),
            ("haiku", "Haiku", "最快，适合简单问题"),
        ]
        .iter()
        .map(|(id, label, desc)| {
            serde_json::json!({ "id": id, "label": label, "desc": desc, "efforts": claude_efforts })
        })
        .collect(),
        "codex" => {
            let mut out = vec![serde_json::json!({
                "id": "", "label": "默认", "desc": "跟随 Codex 配置", "efforts": ["low", "medium", "high", "xhigh"],
            })];
            let path = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|h| h.join(".codex/models_cache.json"));
            if let Some(v) = path
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            {
                let list = v.get("models").cloned().unwrap_or(v);
                for m in list.as_array().into_iter().flatten() {
                    let slug = m.get("slug").and_then(serde_json::Value::as_str).unwrap_or_default();

                    if slug.is_empty() || slug.contains("review") {
                        continue;
                    }
                    let efforts: Vec<&str> = m
                        .get("supported_reasoning_levels")
                        .and_then(serde_json::Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.get("effort").and_then(serde_json::Value::as_str))
                                .collect()
                        })
                        .unwrap_or_default();
                    out.push(serde_json::json!({
                        "id": slug,
                        "label": m.get("display_name").and_then(serde_json::Value::as_str).unwrap_or(slug),
                        "desc": m.get("description").and_then(serde_json::Value::as_str).unwrap_or_default(),
                        "efforts": efforts,
                    }));
                }
            }
            out
        }
        _ => vec![serde_json::json!({ "id": "", "label": "默认", "desc": "", "efforts": [] })],
    };
    Json(serde_json::json!({ "runtime": id, "models": models }))
}

pub async fn runtimes(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let t = st.transport("local");
    let found = blazar_runtime::discover(&t).await?;

    let since = (Utc::now() - chrono::Duration::days(7)).to_rfc3339();
    let rows = sqlx::query(
        "SELECT s.runtime_kind AS kind, s.status AS status, e.payload AS payload
         FROM sessions s LEFT JOIN events e
           ON e.session_id = s.id AND e.payload LIKE '%token_usage%'
         WHERE COALESCE(s.node, 'local') = 'local' AND s.created_at >= ?1",
    )
    .bind(&since)
    .fetch_all(st.db.pool())
    .await?;
    let mut cost: std::collections::HashMap<String, f64> = Default::default();
    for r in &rows {
        let kind: String = r.try_get("kind").unwrap_or_default();
        let payload: Option<String> = r.try_get("payload").ok().flatten();
        if let Some(v) = payload.and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok()) {
            *cost.entry(kind).or_default() += v
                .get("cost_usd")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
        }
    }
    let counts = sqlx::query(
        "SELECT runtime_kind AS kind, COUNT(*) AS n,
                SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END) AS running
         FROM sessions WHERE COALESCE(node, 'local') = 'local' AND created_at >= ?1
         GROUP BY runtime_kind",
    )
    .bind(&since)
    .fetch_all(st.db.pool())
    .await?;
    let count_of = |id: &str| {
        counts
            .iter()
            .find(|r| r.try_get::<String, _>("kind").is_ok_and(|k| k == id))
            .map_or((0, 0), |r| {
                (
                    r.try_get::<i64, _>("n").unwrap_or(0),
                    r.try_get::<i64, _>("running").unwrap_or(0),
                )
            })
    };

    let host = t
        .exec(blazar_transport::ExecSpec::new("hostname"))
        .await
        .map(|o| o.stdout.trim().to_owned())
        .unwrap_or_default();
    let list: Vec<serde_json::Value> = found
        .iter()
        .map(|a| {
            let (sessions, running) = count_of(&a.id);
            serde_json::json!({
                "id": a.id, "label": a.label, "path": a.path, "version": a.version,
                "authed": a.authed, "auth_hint": a.auth_hint,
                "installed": a.path.is_some(),

                "remote_hands": blazar_runtime::supports_remote_hands(&a.id),
                "cost_7d": cost.get(&a.id).copied().unwrap_or(0.0),
                "sessions_7d": sessions, "running": running,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "machine": {
            "hostname": host,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "blazar_version": env!("CARGO_PKG_VERSION"),
        },
        "runtimes": list,
    })))
}

pub async fn node_agents(
    State(st): State<Shared>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<blazar_runtime::DiscoveredAgent>>> {
    let t = st.transport(&name);
    let found = blazar_runtime::discover(&t).await?;

    let json = serde_json::to_string(&found).unwrap_or_else(|_| "[]".into());
    let _ = sqlx::query(
        "UPDATE nodes SET capabilities = json_set(
             CASE WHEN capabilities = '' OR capabilities IS NULL THEN '{}' ELSE capabilities END,
             '$.agents', json(?1))
         WHERE name = ?2",
    )
    .bind(&json)
    .bind(&name)
    .execute(st.db.pool())
    .await;

    Ok(Json(found))
}
