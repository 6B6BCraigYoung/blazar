use super::*;

pub(crate) async fn task_env_of(
    st: &AppState,
    id: &str,
) -> anyhow::Result<Option<blazar_worktree::TaskEnv>> {
    let row = sqlx::query("SELECT path, branch, base_commit FROM workspaces WHERE id = ?1")
        .bind(id)
        .fetch_one(st.db.pool())
        .await?;
    let branch: Option<String> = row.try_get("branch")?;
    let base: Option<String> = row.try_get("base_commit")?;
    let path: String = row.try_get("path")?;
    Ok(branch.zip(base).map(|(branch, base_commit)| {
        let leaf = path.rsplit('/').next().unwrap_or_default().to_owned();
        let root = path
            .rsplit_once("/worktrees/")
            .map_or_else(|| path.clone(), |(r, _)| r.to_owned());
        blazar_worktree::TaskEnv {
            worktree: path.clone(),
            branch,
            base_commit,
            tmpdir: format!("{root}/tmp/{leaf}"),
            config_root: format!("{root}/config/{leaf}"),
        }
    }))
}

pub(crate) async fn worktree_mgr(
    st: &AppState,
    id: &str,
) -> anyhow::Result<(blazar_worktree::WorktreeManager, blazar_worktree::TaskEnv)> {
    let (node, _) = locate(st, id).await?;
    let env = task_env_of(st, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("该工作区不是隔离工作区，没有可管理的 worktree"))?;

    let repo: Option<String> = sqlx::query_scalar("SELECT repo_root FROM workspaces WHERE id = ?1")
        .bind(id)
        .fetch_one(st.db.pool())
        .await?;
    let repo = repo.ok_or_else(|| {
        anyhow::anyhow!("该工作区没有记录主仓库路径（可能建于旧版本），无法管理 worktree")
    })?;
    Ok((
        blazar_worktree::WorktreeManager::new(st.transport(&node), repo),
        env,
    ))
}

#[derive(Debug, Deserialize)]
pub struct BranchQuery {
    pub repo: String,

    #[serde(default = "yes")]
    pub fetch: bool,
}

pub async fn list_branches(
    State(st): State<Shared>,
    Path(name): Path<String>,
    Query(q): Query<BranchQuery>,
) -> ApiResult<Json<Vec<blazar_worktree::BranchInfo>>> {
    let mgr = blazar_worktree::WorktreeManager::new(st.transport(&name), &q.repo);
    Ok(Json(mgr.list_branches(q.fetch).await?))
}

#[derive(Debug, Deserialize)]
pub struct CreateIsolated {
    pub node: String,

    pub repo: String,

    pub from_branch: Option<String>,

    pub name: Option<String>,
    pub project: Option<String>,
}

pub async fn create_isolated(
    State(st): State<Shared>,
    Json(req): Json<CreateIsolated>,
) -> ApiResult<Json<serde_json::Value>> {
    let prefix = crate::office::prefs(&st).await["git"]["branch_prefix"]
        .as_str()
        .unwrap_or("blazar/")
        .to_owned();
    let mgr = blazar_worktree::WorktreeManager::new(st.transport(&req.node), &req.repo)
        .with_branch_prefix(prefix);
    let env = match &req.from_branch {
        Some(b) if !b.trim().is_empty() => mgr.create_from_branch(b.trim()).await?,
        _ => {
            let n = req.name.as_deref().unwrap_or("task");
            mgr.create(n).await?
        }
    };

    let now = Utc::now().to_rfc3339();
    let pool = st.db.pool();
    sqlx::query(
        "INSERT INTO nodes (id, name, transport, status, created_at)
         VALUES (?1, ?2, ?3, 'online', ?4) ON CONFLICT (name) DO NOTHING",
    )
    .bind(blazar_core_types::NodeId::new().to_string())
    .bind(&req.node)
    .bind(if req.node == "local" { "local" } else { "ssh" })
    .bind(&now)
    .execute(pool)
    .await?;
    let node_id: String = sqlx::query_scalar("SELECT id FROM nodes WHERE name = ?1")
        .bind(&req.node)
        .fetch_one(pool)
        .await?;
    let project_id: Option<String> = match &req.project {
        Some(p) if !p.trim().is_empty() => {
            sqlx::query(
                "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (name) DO NOTHING",
            )
            .bind(blazar_core_types::ProjectId::new().to_string())
            .bind(p)
            .bind(&now)
            .execute(pool)
            .await?;
            sqlx::query_scalar("SELECT id FROM projects WHERE name = ?1")
                .bind(p)
                .fetch_optional(pool)
                .await?
        }
        _ => None,
    };
    let display = req
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| env.branch.clone());

    let ws_id = WorkspaceId::new().to_string();
    sqlx::query(
        "INSERT INTO workspaces (id, project_id, node_id, name, path, branch, base_commit,
                                 origin, created_at, repo_root)
         VALUES (?1,?2,?3,?4,?5,?6,?7,'manual',?8,?9)",
    )
    .bind(&ws_id)
    .bind(&project_id)
    .bind(&node_id)
    .bind(&display)
    .bind(&env.worktree)
    .bind(&env.branch)
    .bind(&env.base_commit)
    .bind(&now)
    .bind(&req.repo)
    .execute(pool)
    .await?;

    tokio::spawn(crate::scripts::on_workspace_created(
        st.clone(),
        ws_id.clone(),
        req.repo.clone(),
    ));

    st.emit(ServerEvent::WorkspacesChanged);
    Ok(Json(serde_json::json!({
        "id": ws_id,
        "branch": env.branch,
        "worktree": env.worktree,
        "base_commit": env.base_commit,
    })))
}

pub(crate) const BLAZAR_ROOT: &str = "$HOME/.blazar";

pub async fn node_leftovers(
    State(st): State<Shared>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let t = st.transport(&name);
    let found = blazar_worktree::survey(t.as_ref(), BLAZAR_ROOT).await?;
    let owned = owned_leaves(&st, &name).await?;
    let (mine, orphans): (Vec<_>, Vec<_>) =
        found.into_iter().partition(|l| owned.contains(&l.leaf));
    let total_kb: u64 = orphans.iter().map(|l| l.size_kb).sum();
    Ok(Json(serde_json::json!({
        "orphans": orphans,
        "owned_count": mine.len(),
        "orphan_kb": total_kb,
    })))
}

pub(crate) async fn owned_leaves(
    st: &AppState,
    node: &str,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT w.path FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE n.name = ?1",
    )
    .bind(node)
    .fetch_all(st.db.pool())
    .await?;
    Ok(paths
        .iter()
        .filter_map(|p| {
            p.rsplit_once("/worktrees/")
                .map(|(_, leaf)| leaf.to_owned())
        })
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct SweepRequest {
    pub leaves: Vec<String>,

    #[serde(default)]
    pub force: bool,
}

pub async fn sweep_node(
    State(st): State<Shared>,
    Path(name): Path<String>,
    Json(req): Json<SweepRequest>,
) -> ApiResult<Json<blazar_worktree::Swept>> {
    let owned = owned_leaves(&st, &name).await?;
    let (allowed, refused): (Vec<String>, Vec<String>) =
        req.leaves.into_iter().partition(|l| !owned.contains(l));
    let t = st.transport(&name);
    let mut r = blazar_worktree::sweep(t.as_ref(), BLAZAR_ROOT, &allowed, req.force).await?;
    for l in refused {
        r.skipped
            .push((l, "库里还记着这个工作区，请走「销毁」而不是清扫".into()));
    }
    Ok(Json(r))
}

pub async fn push_workspace(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<blazar_worktree::Pushed>> {
    let (mgr, env) = worktree_mgr(&st, &id).await?;
    Ok(Json(mgr.push(&env).await?))
}

pub async fn pause_workspace(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let stopped = stop_sessions_of(&st, WorkspaceId(id.parse()?)).await;
    let (mgr, env) = worktree_mgr(&st, &id).await?;
    let paused = mgr.pause(&env).await?;
    sqlx::query("UPDATE workspaces SET status = 'paused' WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await?;
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(Json(serde_json::json!({
        "paused": true,
        "commit": paused.commit,
        "branch": env.branch,
        "stopped_sessions": stopped,

        "worktree_was_missing": paused.worktree_was_missing,
    })))
}

pub async fn resume_workspace(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (mgr, env) = worktree_mgr(&st, &id).await?;
    let r = mgr.resume(&env).await?;
    sqlx::query("UPDATE workspaces SET status = 'active' WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await?;
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(Json(serde_json::json!({
        "resumed": true,

        "already_present": r.already_present,

        "moved_aside": r.moved_aside,
    })))
}

#[derive(Debug, Deserialize)]
pub struct CommitRequest {
    pub message: String,
}

pub async fn commit_workspace(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<CommitRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let (mgr, env) = worktree_mgr(&st, &id).await?;
    let commit = mgr.commit(&env, &req.message).await?;
    Ok(Json(serde_json::json!({
        "commit": commit,
        "changed": commit.is_some(),
    })))
}

pub async fn destroy_workspace(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let stopped = stop_sessions_of(&st, WorkspaceId(id.parse()?)).await;

    let mut branch_kept = None;
    if let Ok((mgr, env)) = worktree_mgr(&st, &id).await {
        branch_kept = mgr.destroy(&env).await?.branch_kept;
    }
    sqlx::query("DELETE FROM workspaces WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await?;
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(Json(serde_json::json!({
        "destroyed": true,
        "stopped_sessions": stopped,

        "branch_kept": branch_kept,
    })))
}
