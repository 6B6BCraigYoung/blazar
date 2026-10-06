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
    let workspace = WorkspaceId(id.parse()?);
    let _guard = st.quiesce(workspace).map_err(ApiError::bad_request)?;
    stop_sessions_of(&st, workspace).await?;
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

#[derive(Debug, Deserialize)]
pub struct CopyBody {
    pub to: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub dest: String,
}

const MAX_COPY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_COPY_ITEMS: usize = 64;

fn not_a_folder(error: &blazar_vfs::VfsError) -> bool {
    matches!(
        error,
        blazar_vfs::VfsError::Transport(blazar_transport::TransportError::Command { code: 4, .. })
    )
}

pub async fn workspace_copy(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<CopyBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.paths.is_empty() {
        return Err(ApiError::bad_request("至少要给一个路径"));
    }
    if body.paths.len() > MAX_COPY_ITEMS {
        return Err(ApiError::bad_request(format!(
            "一次最多拷 {MAX_COPY_ITEMS} 项"
        )));
    }
    let dest = body.dest.trim().trim_matches('/').to_owned();
    if !dest.is_empty() {
        blazar_vfs::safe_relative_path(&dest).map_err(|e| ApiError::bad_request(e.to_string()))?;
    }
    let (from_node, from_path) = locate(&st, &id).await?;
    let (to_node, to_path) = locate(&st, &body.to).await?;
    let from = vfs_for(&st, &from_node, &from_path);
    let to = vfs_for(&st, &to_node, &to_path);
    let mut budget = MAX_COPY_BYTES;
    let mut copied = Vec::new();
    for raw in &body.paths {
        let rel = blazar_vfs::safe_relative_path(raw)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_owned();
        let target = if dest.is_empty() {
            name
        } else {
            format!("{dest}/{name}")
        };
        let (payload, kind) = match from.archive_base64(&rel, budget).await {
            Ok(archive) => (archive, "dir"),
            Err(error) if not_a_folder(&error) => (from.read_base64(&rel, budget).await?, "file"),
            Err(error) => return Err(error.into()),
        };
        budget = budget.saturating_sub(payload.len() as u64 * 3 / 4);
        if kind == "dir" {
            to.extract_base64(&target, &payload).await?;
        } else {
            to.write_base64(&target, &payload).await?;
        }
        copied.push(serde_json::json!({ "path": target, "kind": kind }));
    }
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(Json(serde_json::json!({
        "to": body.to,
        "node": to_node,
        "copied": copied,
        "remaining_bytes": budget,
    })))
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

#[derive(Debug, Deserialize)]
pub struct DirQuery {
    #[serde(default)]
    pub path: String,
}

pub async fn workspace_ls(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<DirQuery>,
) -> ApiResult<Json<blazar_vfs::DirItems>> {
    let (node, path) = locate(&st, &id).await?;
    Ok(Json(vfs_for(&st, &node, &path).list_dir(&q.path).await?))
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

const DOWNLOAD_MAX: u64 = 128 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub dir: bool,
}

pub async fn workspace_download(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<DownloadQuery>,
) -> Response {
    use base64::Engine;
    let (node, root) = match locate(&st, &id).await {
        Ok(v) => v,
        Err(e) => return ApiError(e).into_response(),
    };
    let vfs = vfs_for(&st, &node, &root);
    let read = if q.dir {
        vfs.archive_base64(&q.path, DOWNLOAD_MAX).await
    } else {
        vfs.read_base64(&q.path, DOWNLOAD_MAX).await
    };
    let b64 = match read {
        Ok(b) => b,
        Err(blazar_vfs::VfsError::PathEscape(_)) => {
            return (StatusCode::BAD_REQUEST, "路径越界").into_response();
        }
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) else {
        return (StatusCode::BAD_GATEWAY, "文件没读完整").into_response();
    };
    let base = q
        .path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty() && *n != ".")
        .map_or_else(
            || {
                root.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            },
            str::to_owned,
        );
    let base = if base.is_empty() {
        "download".to_owned()
    } else {
        base
    };
    let name = if q.dir {
        format!("{base}.tar.gz")
    } else {
        base
    };
    let ctype = if q.dir {
        "application/gzip"
    } else {
        "application/octet-stream"
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, ctype.to_owned()),
            (axum::http::header::CONTENT_DISPOSITION, attachment(&name)),
            (
                axum::http::header::X_CONTENT_TYPE_OPTIONS,
                "nosniff".to_owned(),
            ),
            (axum::http::header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        bytes,
    )
        .into_response()
}

fn attachment(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && !matches!(c, '"' | '\\')) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

#[derive(Debug, Deserialize)]
pub struct WriteFile {
    pub path: String,
    pub content: String,

    #[serde(default)]
    pub expect_mtime: Option<u64>,

    #[serde(default)]
    pub expect_version: Option<String>,
}

pub async fn workspace_file_write(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<WriteFile>,
) -> ApiResult<Json<blazar_vfs::Written>> {
    let (node, path) = locate(&st, &id).await?;
    let w = vfs_for(&st, &node, &path)
        .write_checked(
            &b.path,
            &b.content,
            b.expect_mtime,
            b.expect_version.as_deref(),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn local_state(root: &std::path::Path) -> Shared {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, root.to_path_buf()),
            crate::services::Services::Isolated,
        )
    }

    async fn local_workspace(st: &Shared, path: &std::path::Path) -> String {
        let created = create_workspace(
            State(st.clone()),
            Json(CreateWorkspace {
                node: "local".into(),
                path: path.to_string_lossy().into_owned(),
                name: None,
                project: None,
            }),
        )
        .await
        .map_err(|e| e.message())
        .unwrap();
        created.0["id"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn copy_moves_files_and_folders_between_workspaces() {
        let root = tempfile::tempdir().unwrap();
        let st = local_state(root.path()).await;
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(a.path().join("pkg/src")).unwrap();
        std::fs::write(a.path().join("pkg/src/lib.rs"), "pub fn x() {}").unwrap();
        std::fs::write(a.path().join("notes.md"), "# notes").unwrap();
        let from = local_workspace(&st, a.path()).await;
        let to = local_workspace(&st, b.path()).await;
        let out = workspace_copy(
            State(st.clone()),
            Path(from.clone()),
            Json(CopyBody {
                to: to.clone(),
                paths: vec!["pkg".into(), "notes.md".into()],
                dest: "incoming".into(),
            }),
        )
        .await
        .map_err(|e| e.message())
        .unwrap()
        .0;
        assert_eq!(out["copied"].as_array().unwrap().len(), 2);
        assert_eq!(out["copied"][0]["kind"], "dir");
        assert_eq!(out["copied"][1]["kind"], "file");
        assert_eq!(
            std::fs::read_to_string(b.path().join("incoming/pkg/src/lib.rs")).unwrap(),
            "pub fn x() {}"
        );
        assert_eq!(
            std::fs::read_to_string(b.path().join("incoming/notes.md")).unwrap(),
            "# notes"
        );
        for (paths, dest) in [
            (vec!["../secret".to_owned()], String::new()),
            (vec!["notes.md".to_owned()], "../out".to_owned()),
            (Vec::new(), String::new()),
            (vec!["missing.txt".to_owned()], String::new()),
        ] {
            let bad = workspace_copy(
                State(st.clone()),
                Path(from.clone()),
                Json(CopyBody {
                    to: to.clone(),
                    paths,
                    dest,
                }),
            )
            .await;
            assert!(bad.is_err());
        }
        assert!(!b.path().join("secret").exists());
        assert!(!root.path().join("out").exists());
    }

    #[test]
    fn download_names_survive_quotes_and_unicode() {
        assert_eq!(
            attachment("报告 \"v2\".pdf"),
            "attachment; filename=\"__ _v2_.pdf\"; filename*=UTF-8''%E6%8A%A5%E5%91%8A%20%22v2%22.pdf"
        );
    }

    #[test]
    fn file_write_requests_keep_legacy_mtime_and_accept_content_versions() {
        let legacy: WriteFile = serde_json::from_value(serde_json::json!({
            "path": "note.txt", "content": "note", "expect_mtime": 12
        }))
        .unwrap();
        assert_eq!(legacy.expect_mtime, Some(12));
        assert_eq!(legacy.expect_version, None);
        let version = "a".repeat(64);
        let current: WriteFile = serde_json::from_value(serde_json::json!({
            "path": "note.txt", "content": "note", "expect_version": version
        }))
        .unwrap();
        assert_eq!(current.expect_mtime, None);
        assert_eq!(current.expect_version.as_deref(), Some(version.as_str()));
    }
}
