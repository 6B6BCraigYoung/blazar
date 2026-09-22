use super::*;

pub async fn usage(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let rows = sqlx::query(
        "SELECT e.payload, s.runtime_kind, n.name AS node, w.name AS workspace, w.id AS ws_id
         FROM events e
         JOIN sessions s ON s.id = e.session_id
         JOIN workspaces w ON w.id = s.workspace_id
         JOIN nodes n ON n.id = w.node_id
         WHERE e.payload LIKE '%token_usage%'",
    )
    .fetch_all(st.db.pool())
    .await?;

    #[derive(Default, Clone)]
    struct Agg {
        input: u64,
        output: u64,
        cache_read: u64,
        cost: f64,
        calls: u64,
    }
    let mut total = Agg::default();
    let mut by_agent: std::collections::BTreeMap<String, Agg> = Default::default();
    let mut by_node: std::collections::BTreeMap<String, Agg> = Default::default();
    let mut by_ws: std::collections::BTreeMap<String, (String, Agg)> = Default::default();

    for r in &rows {
        let payload: String = r.try_get("payload").unwrap_or_default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) else {
            continue;
        };
        if v.get("type").and_then(serde_json::Value::as_str) != Some("token_usage") {
            continue;
        }
        let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        let cost = v
            .get("cost_usd")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);

        let add = |a: &mut Agg| {
            a.input += n("input");
            a.output += n("output");
            a.cache_read += n("cache_read");
            a.cost += cost;
            a.calls += 1;
        };
        add(&mut total);
        add(by_agent
            .entry(r.try_get("runtime_kind").unwrap_or_default())
            .or_default());
        add(by_node
            .entry(r.try_get("node").unwrap_or_default())
            .or_default());
        let ws_id: String = r.try_get("ws_id").unwrap_or_default();
        let ws_name: String = r.try_get("workspace").unwrap_or_default();
        let e = by_ws.entry(ws_id).or_insert((ws_name, Agg::default()));
        add(&mut e.1);
    }

    let dump = |a: &Agg| {
        serde_json::json!({
            "input": a.input, "output": a.output, "cache_read": a.cache_read,
            "cost_usd": a.cost, "calls": a.calls,
        })
    };

    Ok(Json(serde_json::json!({
        "total": dump(&total),
        "by_agent": by_agent.iter().map(|(k, v)| (k.clone(), dump(v)))
            .collect::<serde_json::Map<_, _>>(),
        "by_node": by_node.iter().map(|(k, v)| (k.clone(), dump(v)))
            .collect::<serde_json::Map<_, _>>(),
        "by_workspace": by_ws.values()
            .map(|(name, a)| serde_json::json!({ "workspace": name, "usage": dump(a) }))
            .collect::<Vec<_>>(),
    })))
}

pub async fn workspace_detail(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let row = sqlx::query(
        "SELECT w.id, w.name, w.path, w.branch, w.base_commit, w.origin, w.status,
                w.activity, w.last_active_at, w.created_at, w.repo_root,
                n.name AS node, p.name AS project
         FROM workspaces w
         JOIN nodes n ON n.id = w.node_id
         LEFT JOIN projects p ON p.id = w.project_id
         WHERE w.id = ?1",
    )
    .bind(&id)
    .fetch_one(st.db.pool())
    .await?;

    let sessions = sqlx::query(
        "SELECT s.id, s.runtime_kind, s.status, s.provider_session_id, s.created_at,
                (SELECT COUNT(*) FROM events e WHERE e.session_id = s.id) AS events
         FROM sessions s WHERE s.workspace_id = ?1 ORDER BY s.created_at DESC LIMIT 20",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await?;

    Ok(Json(serde_json::json!({
        "id": row.try_get::<String,_>("id").unwrap_or_default(),
        "name": row.try_get::<String,_>("name").unwrap_or_default(),
        "node": row.try_get::<String,_>("node").unwrap_or_default(),
        "path": row.try_get::<String,_>("path").unwrap_or_default(),
        "project": row.try_get::<Option<String>,_>("project").unwrap_or(None),
        "branch": row.try_get::<Option<String>,_>("branch").unwrap_or(None),
        "base_commit": row.try_get::<Option<String>,_>("base_commit").unwrap_or(None),
        "repo_root": row.try_get::<Option<String>,_>("repo_root").unwrap_or(None),
        "origin": row.try_get::<String,_>("origin").unwrap_or_default(),
        "status": row.try_get::<String,_>("status").unwrap_or_default(),
        "activity": row.try_get::<String,_>("activity").unwrap_or_default(),
        "last_active_at": row.try_get::<Option<String>,_>("last_active_at").unwrap_or(None),
        "created_at": row.try_get::<String,_>("created_at").unwrap_or_default(),
        "isolated": row.try_get::<Option<String>,_>("branch").unwrap_or(None).is_some(),
        "sessions": sessions.iter().map(|s| serde_json::json!({
            "id": s.try_get::<String,_>("id").unwrap_or_default(),
            "runtime": s.try_get::<String,_>("runtime_kind").unwrap_or_default(),
            "status": s.try_get::<String,_>("status").unwrap_or_default(),
            "provider_session_id": s.try_get::<Option<String>,_>("provider_session_id").unwrap_or(None),
            "created_at": s.try_get::<String,_>("created_at").unwrap_or_default(),
            "events": s.try_get::<i64,_>("events").unwrap_or(0),
        })).collect::<Vec<_>>(),
    })))
}

pub async fn workspace_sessions(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let rows = sqlx::query(
        "SELECT t.thread, r.runtime_kind, r.agent_profile, r.title, r.created_at,
                (SELECT x.status FROM sessions x WHERE COALESCE(x.thread_id, x.id) = t.thread
                   ORDER BY x.created_at DESC LIMIT 1) AS status,
                (SELECT x.id FROM sessions x WHERE COALESCE(x.thread_id, x.id) = t.thread
                   ORDER BY x.created_at DESC LIMIT 1) AS latest,
                (SELECT COUNT(*) FROM events e JOIN sessions x ON x.id = e.session_id
                   WHERE COALESCE(x.thread_id, x.id) = t.thread) AS events,
                (SELECT e.payload FROM events e WHERE e.session_id = t.thread
                   AND e.payload LIKE '%user_message%' ORDER BY e.seq LIMIT 1) AS first_msg,
                (SELECT MAX(e.ts) FROM events e JOIN sessions x ON x.id = e.session_id
                   WHERE COALESCE(x.thread_id, x.id) = t.thread) AS last_at,
                (SELECT MAX(x.created_at) FROM sessions x WHERE COALESCE(x.thread_id, x.id) = t.thread) AS touched
         FROM (SELECT DISTINCT COALESCE(thread_id, id) AS thread FROM sessions WHERE workspace_id = ?1) t
         JOIN sessions r ON r.id = t.thread
         ORDER BY touched DESC LIMIT 50",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await?;
    let first_line = |payload: Option<String>| -> String {
        payload
            .and_then(|p| serde_json::from_str::<serde_json::Value>(&p).ok())
            .and_then(|v| {
                v.pointer("/text")
                    .or_else(|| v.pointer("/kind/text"))
                    .and_then(|t| t.as_str())
                    .map(|t| t.trim().chars().take(60).collect::<String>())
            })
            .unwrap_or_default()
    };
    Ok(Json(
        rows.iter()
            .map(|r| {
                let stored = r
                    .try_get::<Option<String>, _>("title")
                    .unwrap_or(None)
                    .filter(|t| !t.is_empty());
                serde_json::json!({
                    "id": r.try_get::<String, _>("thread").unwrap_or_default(),
                    "latest": r.try_get::<Option<String>, _>("latest").unwrap_or(None),
                    "runtime": r.try_get::<String, _>("runtime_kind").unwrap_or_default(),
                    "status": r.try_get::<Option<String>, _>("status").unwrap_or(None),
                    "profile": r.try_get::<Option<String>, _>("agent_profile").unwrap_or(None),
                    "created_at": r.try_get::<String, _>("created_at").unwrap_or_default(),
                    "last_at": r.try_get::<Option<String>, _>("last_at").unwrap_or(None),
                    "events": r.try_get::<i64, _>("events").unwrap_or(0),

                    "titled": stored.is_some(),
                    "title": stored.unwrap_or_else(|| first_line(r.try_get::<Option<String>, _>("first_msg").unwrap_or(None))),
                })
            })
            .collect::<Vec<_>>(),
    ))
}
