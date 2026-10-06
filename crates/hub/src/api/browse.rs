use super::*;

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    pub path: String,
}

pub async fn browse_node(
    State(st): State<Shared>,
    Path(name): Path<String>,
    Query(q): Query<BrowseQuery>,
) -> ApiResult<Json<blazar_vfs::DirListing>> {
    let vfs = Vfs::new(st.transport(&name), "");
    Ok(Json(vfs.browse(&q.path).await?))
}

#[derive(Debug, Deserialize)]
pub struct MkdirBody {
    #[serde(default)]
    pub path: String,
    pub name: String,
}

pub async fn mkdir_node(
    State(st): State<Shared>,
    Path(name): Path<String>,
    Json(body): Json<MkdirBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let vfs = Vfs::new(st.transport(&name), "");
    let created = vfs
        .mkdir(&body.path, &body.name)
        .await
        .map_err(|error| match error {
            blazar_vfs::VfsError::Folder(message) => ApiError::bad_request(message),
            other => other.into(),
        })?;
    Ok(Json(serde_json::json!({ "path": created })))
}
