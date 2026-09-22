use super::*;

#[derive(Debug, Deserialize)]
pub struct CreateWorkspace {
    pub node: String,
    pub path: String,
    pub name: Option<String>,

    pub project: Option<String>,
}

pub async fn create_workspace(
    State(st): State<Shared>,
    Json(req): Json<CreateWorkspace>,
) -> ApiResult<Json<serde_json::Value>> {
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
        Some(name) if !name.trim().is_empty() => {
            sqlx::query(
                "INSERT INTO projects (id, name, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (name) DO NOTHING",
            )
            .bind(blazar_core_types::ProjectId::new().to_string())
            .bind(name)
            .bind(&now)
            .execute(pool)
            .await?;
            sqlx::query_scalar("SELECT id FROM projects WHERE name = ?1")
                .bind(name)
                .fetch_optional(pool)
                .await?
        }
        _ => None,
    };

    let name = req.name.clone().unwrap_or_else(|| {
        req.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&req.path)
            .to_owned()
    });

    sqlx::query(
        "INSERT INTO workspaces (id, project_id, node_id, name, path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (node_id, path) DO UPDATE SET
            name = excluded.name, project_id = excluded.project_id",
    )
    .bind(WorkspaceId::new().to_string())
    .bind(&project_id)
    .bind(&node_id)
    .bind(&name)
    .bind(&req.path)
    .bind(&now)
    .execute(pool)
    .await?;

    let id: String = sqlx::query_scalar("SELECT id FROM workspaces WHERE node_id=?1 AND path=?2")
        .bind(&node_id)
        .bind(&req.path)
        .fetch_one(pool)
        .await?;

    st.emit(ServerEvent::WorkspacesChanged);
    Ok(Json(serde_json::json!({ "id": id })))
}

pub async fn delete_workspace(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM workspaces WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await?;
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn locate(st: &AppState, id: &str) -> anyhow::Result<(String, String)> {
    let row = sqlx::query(
        "SELECT n.name AS node, w.path FROM workspaces w
         JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(id)
    .fetch_one(st.db.pool())
    .await?;
    Ok((row.try_get("node")?, row.try_get("path")?))
}

pub(crate) fn vfs_for(st: &AppState, node: &str, path: &str) -> Vfs {
    Vfs::new(st.transport(node), path)
}

pub async fn workspace_tree(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (node, path) = locate(&st, &id).await?;
    let (entries, stat) = vfs_for(&st, &node, &path).tree_with_stat(None).await?;

    let truncated = entries.len() >= blazar_vfs::MAX_TREE_ENTRIES;

    if let Some(d) = stat {
        let _ = sqlx::query(
            "UPDATE workspaces SET diff_added = ?1, diff_removed = ?2, diff_files = ?3,
                    diff_at = ?4 WHERE id = ?5",
        )
        .bind(i64::try_from(d.added).unwrap_or(i64::MAX))
        .bind(i64::try_from(d.removed).unwrap_or(i64::MAX))
        .bind(i64::try_from(d.files).unwrap_or(i64::MAX))
        .bind(Utc::now().to_rfc3339())
        .bind(&id)
        .execute(st.db.pool())
        .await;
    }
    Ok(Json(serde_json::json!({
        "entries": entries,
        "truncated": truncated,
        "stat": stat,
    })))
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub path: String,
}

pub async fn workspace_file(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<blazar_vfs::FileContent>> {
    let (node, path) = locate(&st, &id).await?;
    Ok(Json(vfs_for(&st, &node, &path).read(&q.path).await?))
}

pub(crate) fn image_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => return None,
    })
}

pub async fn workspace_raw(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> Response {
    use base64::Engine;
    let Some(ctype) = image_type(&q.path) else {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "只提供图片").into_response();
    };
    let (node, root) = match locate(&st, &id).await {
        Ok(v) => v,
        Err(e) => return ApiError(e).into_response(),
    };
    let b64 = match vfs_for(&st, &node, &root)
        .read_base64(&q.path, 8 * 1024 * 1024)
        .await
    {
        Ok(b) => b,
        Err(blazar_vfs::VfsError::PathEscape(_)) => {
            return (StatusCode::BAD_REQUEST, "路径越界").into_response();
        }
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) else {
        return (StatusCode::BAD_GATEWAY, "图片没读完整").into_response();
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, ctype),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; img-src data:; sandbox",
            ),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (axum::http::header::CACHE_CONTROL, "private, max-age=30"),
        ],
        bytes,
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct WriteFile {
    pub path: String,
    pub content: String,

    #[serde(default)]
    pub expect_mtime: Option<u64>,
}

pub async fn workspace_file_write(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<WriteFile>,
) -> ApiResult<Json<blazar_vfs::Written>> {
    let (node, path) = locate(&st, &id).await?;
    let w = vfs_for(&st, &node, &path)
        .write(&b.path, &b.content, b.expect_mtime)
        .await?;
    if w.saved {
        st.emit(ServerEvent::WorkspacesChanged);
    }
    Ok(Json(w))
}

#[derive(Debug, Default, Deserialize)]
pub struct DiffQuery {
    #[serde(default)]
    pub base: Option<String>,

    #[serde(default)]
    pub w: Option<String>,
}

pub(crate) const MAX_DIFF_BYTES: usize = 3 * 1024 * 1024;

pub async fn workspace_diff(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let (node, path) = locate(&st, &id).await?;
    let vfs = vfs_for(&st, &node, &path);

    let branch = q.base.as_deref().map(str::trim).filter(|b| {
        !b.is_empty() && *b != "head" && !b.starts_with('-') && !b.contains(char::is_whitespace)
    });
    let base = match branch {
        Some(b) => b.to_owned(),
        None => vfs.head_commit().await?.unwrap_or_else(|| "HEAD".into()),
    };
    let opts = blazar_vfs::DiffOpts {
        merge_base: branch.is_some(),
        ignore_ws: q.w.as_deref().is_some_and(|w| w == "1" || w == "true"),
    };

    match vfs.diff_opts(&base, opts).await {
        Ok(mut diff) => {
            let truncated = diff.len() > MAX_DIFF_BYTES;
            if truncated {
                let mut cut = MAX_DIFF_BYTES;
                while !diff.is_char_boundary(cut) {
                    cut -= 1;
                }
                diff.truncate(cut);
            }
            Ok(Json(serde_json::json!({
                "base": base, "diff": diff, "truncated": truncated,
            })))
        }
        Err(blazar_vfs::VfsError::NotARepo(path)) => Ok(Json(serde_json::json!({
            "base": base,
            "diff": "",
            "reason": format!("{path} 不是 git 仓库，没有可比较的基线"),
        }))),
        Err(e) => Err(e.into()),
    }
}
