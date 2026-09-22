use super::*;

pub async fn get_context(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = sqlx::query("SELECT kind, payload FROM workspace_context WHERE workspace_id = ?1")
        .bind(&id)
        .fetch_all(st.db.pool())
        .await?;
    let mut map = serde_json::Map::new();
    for r in rows {
        let kind: String = r.try_get("kind")?;
        let payload: String = r.try_get("payload")?;
        map.insert(
            kind,
            serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
        );
    }
    Ok(Json(serde_json::Value::Object(map)))
}

pub async fn put_context(
    State(st): State<Shared>,
    Path((id, kind)): Path<(String, String)>,
    Json(payload): Json<serde_json::Value>,
) -> ApiResult<StatusCode> {
    sqlx::query(
        "INSERT INTO workspace_context (workspace_id, kind, payload, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (workspace_id, kind) DO UPDATE SET
            payload = excluded.payload, updated_at = excluded.updated_at",
    )
    .bind(&id)
    .bind(&kind)
    .bind(payload.to_string())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
