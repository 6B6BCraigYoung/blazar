use super::*;

#[derive(Debug, Serialize)]
pub struct NodeView {
    pub name: String,
    pub transport: String,
    pub ipv4: Option<String>,
    pub status: String,
    pub latency_ms: Option<f64>,
    pub cost: Option<String>,
    pub workspace_count: i64,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceView {
    pub id: String,
    pub name: String,
    pub node: String,
    pub path: String,
    pub project: Option<String>,
    pub activity: String,
    pub last_active_at: Option<String>,
    pub session_id: Option<String>,

    pub diff: Option<blazar_vfs::DiffStat>,

    pub diff_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StateSnapshot {
    pub nodes: Vec<NodeView>,
    pub workspaces: Vec<WorkspaceView>,
}

pub async fn get_state(State(st): State<Shared>) -> ApiResult<Json<StateSnapshot>> {
    Ok(Json(snapshot(&st).await?))
}

pub(crate) async fn snapshot(st: &AppState) -> anyhow::Result<StateSnapshot> {
    let nodes = sqlx::query(
        "SELECT n.name, n.transport, n.endpoint, n.status, n.labels,
                (SELECT COUNT(*) FROM workspaces w WHERE w.node_id = n.id) AS wc
         FROM nodes n ORDER BY n.name",
    )
    .fetch_all(st.db.pool())
    .await?
    .into_iter()
    .map(|r| {
        let labels: String = r.try_get("labels").unwrap_or_default();
        let meta: serde_json::Value = serde_json::from_str(&labels).unwrap_or_default();
        NodeView {
            name: r.try_get("name").unwrap_or_default(),
            transport: r.try_get("transport").unwrap_or_default(),
            ipv4: r.try_get::<Option<String>, _>("endpoint").unwrap_or(None),
            status: r.try_get("status").unwrap_or_default(),
            latency_ms: meta.get("latency_ms").and_then(serde_json::Value::as_f64),
            cost: meta
                .get("cost")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            workspace_count: r.try_get("wc").unwrap_or(0),
        }
    })
    .collect();

    let workspaces = sqlx::query(
        "SELECT w.id, w.name, w.path, w.activity, w.last_active_at,
                w.diff_added, w.diff_removed, w.diff_files, w.diff_at,
                n.name AS node, p.name AS project,
                (SELECT s.id FROM sessions s WHERE s.workspace_id = w.id
                 ORDER BY s.created_at DESC LIMIT 1) AS session_id
         FROM workspaces w
         JOIN nodes n ON n.id = w.node_id
         LEFT JOIN projects p ON p.id = w.project_id
         ORDER BY p.name IS NULL, p.name, n.name, w.name",
    )
    .fetch_all(st.db.pool())
    .await?
    .into_iter()
    .map(|r| WorkspaceView {
        id: r.try_get("id").unwrap_or_default(),
        name: r.try_get("name").unwrap_or_default(),
        node: r.try_get("node").unwrap_or_default(),
        path: r.try_get("path").unwrap_or_default(),
        project: r.try_get("project").unwrap_or(None),
        activity: r.try_get("activity").unwrap_or_else(|_| "idle".into()),
        last_active_at: r.try_get("last_active_at").unwrap_or(None),
        session_id: r.try_get("session_id").unwrap_or(None),
        diff: match (
            r.try_get::<Option<i64>, _>("diff_added").unwrap_or(None),
            r.try_get::<Option<i64>, _>("diff_removed").unwrap_or(None),
            r.try_get::<Option<i64>, _>("diff_files").unwrap_or(None),
        ) {
            (Some(a), Some(d), Some(f)) => Some(blazar_vfs::DiffStat {
                added: u64::try_from(a).unwrap_or(0),
                removed: u64::try_from(d).unwrap_or(0),
                files: u64::try_from(f).unwrap_or(0),
            }),
            _ => None,
        },
        diff_at: r.try_get("diff_at").unwrap_or(None),
    })
    .collect();

    Ok(StateSnapshot { nodes, workspaces })
}
