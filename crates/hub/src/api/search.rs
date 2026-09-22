use super::*;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,

    #[serde(default)]
    pub workspaces: String,
}

#[derive(Debug, Serialize)]
pub struct SearchGroup {
    pub workspace_id: String,
    pub workspace: String,
    pub node: String,
    pub project: Option<String>,
    pub hits: Vec<blazar_vfs::SearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub(crate) const SEARCH_TIMEOUT_SECS: u64 = 14;

pub async fn search(
    State(st): State<Shared>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<Vec<SearchGroup>>> {
    if q.q.trim().is_empty() {
        return Ok(Json(Vec::new()));
    }
    let filter: Vec<&str> = q
        .workspaces
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let rows = sqlx::query(
        "SELECT w.id, w.name, w.path, n.name AS node, p.name AS project
         FROM workspaces w JOIN nodes n ON n.id = w.node_id
         LEFT JOIN projects p ON p.id = w.project_id",
    )
    .fetch_all(st.db.pool())
    .await?;

    let mut tasks = Vec::new();
    for r in rows {
        let id: String = r.try_get("id")?;
        if !filter.is_empty() && !filter.contains(&id.as_str()) {
            continue;
        }
        let name: String = r.try_get("name")?;
        let path: String = r.try_get("path")?;
        let node: String = r.try_get("node")?;
        let project: Option<String> = r.try_get("project")?;
        let vfs = vfs_for(&st, &node, &path);
        let query = q.q.clone();

        tasks.push(tokio::spawn(async move {
            let result = match tokio::time::timeout(
                std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS),
                vfs.search(&query, 80),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => {
                    return SearchGroup {
                        workspace_id: id,
                        workspace: name,
                        node,
                        project,
                        hits: Vec::new(),
                        error: Some(format!("搜索超时（>{SEARCH_TIMEOUT_SECS}s）")),
                    };
                }
            };
            match result {
                Ok(hits) => SearchGroup {
                    workspace_id: id,
                    workspace: name,
                    node,
                    project,
                    hits,
                    error: None,
                },
                Err(e) => SearchGroup {
                    workspace_id: id,
                    workspace: name,
                    node,
                    project,
                    hits: Vec::new(),
                    error: Some(e.to_string()),
                },
            }
        }));
    }

    let mut groups = Vec::new();
    for t in tasks {
        if let Ok(g) = t.await
            && (!g.hits.is_empty() || g.error.is_some())
        {
            groups.push(g);
        }
    }
    groups.sort_by(|a, b| b.hits.len().cmp(&a.hits.len()));
    Ok(Json(groups))
}
