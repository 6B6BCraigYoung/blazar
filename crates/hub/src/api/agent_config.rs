use super::*;

#[derive(Debug, Serialize)]
pub struct AgentConfigView {
    pub id: String,
    pub node: Option<String>,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub program_path: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub custom_args: Vec<String>,
    pub custom_env: std::collections::BTreeMap<String, String>,
    pub max_concurrent: i64,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct AgentConfigInput {
    #[serde(default)]
    pub node: Option<String>,
    pub agent_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub program_path: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub custom_args: Vec<String>,
    #[serde(default)]
    pub custom_env: std::collections::BTreeMap<String, String>,
    #[serde(default = "two")]
    pub max_concurrent: i64,
    #[serde(default = "tru")]
    pub enabled: bool,
}

pub(crate) const fn two() -> i64 {
    2
}

pub(crate) fn row_to_agent_config(r: &sqlx::sqlite::SqliteRow) -> AgentConfigView {
    AgentConfigView {
        id: r.try_get("id").unwrap_or_default(),
        node: r.try_get("node").unwrap_or(None),
        agent_id: r.try_get("agent_id").unwrap_or_default(),
        display_name: r.try_get("display_name").unwrap_or(None),
        program_path: r.try_get("program_path").unwrap_or(None),
        model: r.try_get("model").unwrap_or(None),
        permission_mode: r.try_get("permission_mode").unwrap_or(None),
        custom_args: serde_json::from_str(
            &r.try_get::<String, _>("custom_args").unwrap_or_default(),
        )
        .unwrap_or_default(),
        custom_env: serde_json::from_str(&r.try_get::<String, _>("custom_env").unwrap_or_default())
            .unwrap_or_default(),
        max_concurrent: r.try_get("max_concurrent").unwrap_or(2),
        enabled: r.try_get::<i64, _>("enabled").unwrap_or(1) != 0,
    }
}

pub async fn list_agent_configs(State(st): State<Shared>) -> ApiResult<Json<Vec<AgentConfigView>>> {
    let rows = sqlx::query(
        "SELECT c.*, n.name AS node FROM agent_configs c
         LEFT JOIN nodes n ON n.id = c.node_id
         ORDER BY n.name IS NULL DESC, n.name, c.agent_id",
    )
    .fetch_all(st.db.pool())
    .await?;
    Ok(Json(rows.iter().map(row_to_agent_config).collect()))
}

pub async fn upsert_agent_config(
    State(st): State<Shared>,
    Json(c): Json<AgentConfigInput>,
) -> ApiResult<Json<serde_json::Value>> {
    if blazar_runtime::spec::find(&c.agent_id).is_none() {
        return Err(ApiError(anyhow::anyhow!("未知 agent: {}", c.agent_id)));
    }
    let node_id: Option<String> = match &c.node {
        Some(n) if !n.is_empty() => {
            sqlx::query_scalar("SELECT id FROM nodes WHERE name = ?1")
                .bind(n)
                .fetch_optional(st.db.pool())
                .await?
        }
        _ => None,
    };
    let now = Utc::now().to_rfc3339();
    let id = uuid::Uuid::now_v7().to_string();

    let conflict = if node_id.is_none() {
        "ON CONFLICT (agent_id) WHERE node_id IS NULL"
    } else {
        "ON CONFLICT (node_id, agent_id)"
    };
    let sql = format!(
        "INSERT INTO agent_configs (id, node_id, agent_id, display_name, program_path, model,
             permission_mode, custom_args, custom_env, max_concurrent, enabled,
             created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)
         {conflict} DO UPDATE SET
             display_name = excluded.display_name,
             program_path = excluded.program_path,
             model = excluded.model,
             permission_mode = excluded.permission_mode,
             custom_args = excluded.custom_args,
             custom_env = excluded.custom_env,
             max_concurrent = excluded.max_concurrent,
             enabled = excluded.enabled,
             updated_at = excluded.updated_at"
    );
    sqlx::query(&sql)
        .bind(&id)
        .bind(&node_id)
        .bind(&c.agent_id)
        .bind(&c.display_name)
        .bind(&c.program_path)
        .bind(&c.model)
        .bind(&c.permission_mode)
        .bind(serde_json::to_string(&c.custom_args).unwrap_or_else(|_| "[]".into()))
        .bind(serde_json::to_string(&c.custom_env).unwrap_or_else(|_| "{}".into()))
        .bind(c.max_concurrent)
        .bind(i64::from(c.enabled))
        .bind(&now)
        .execute(st.db.pool())
        .await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn delete_agent_config(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM agent_configs WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn effective_agent_config(
    st: &AppState,
    node: &str,
    agent_id: &str,
) -> Option<AgentConfigView> {
    let rows = sqlx::query(
        "SELECT c.*, n.name AS node FROM agent_configs c
         LEFT JOIN nodes n ON n.id = c.node_id
         WHERE c.agent_id = ?1 AND c.enabled = 1
           AND (n.name = ?2 OR c.node_id IS NULL)
         ORDER BY c.node_id IS NOT NULL, c.updated_at",
    )
    .bind(agent_id)
    .bind(node)
    .fetch_all(st.db.pool())
    .await
    .ok()?;

    rows.iter()
        .map(row_to_agent_config)
        .reduce(merge_agent_config)
}

pub(crate) fn merge_agent_config(
    mut base: AgentConfigView,
    over: AgentConfigView,
) -> AgentConfigView {
    base.id = over.id;
    base.node = over.node.or(base.node);
    base.display_name = over.display_name.or(base.display_name);
    base.program_path = over.program_path.or(base.program_path);
    base.model = over.model.or(base.model);
    base.permission_mode = over.permission_mode.or(base.permission_mode);
    base.custom_env.extend(over.custom_env);
    base.custom_args.extend(over.custom_args);
    base.max_concurrent = over.max_concurrent;
    base.enabled = over.enabled;
    base
}
