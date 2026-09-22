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
