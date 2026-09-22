use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;

const MAX_BODY: usize = 20_000;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn check(name: &str, body: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 40 {
        return Err("名字要 1–40 个字".into());
    }
    if n.chars().any(|c| c.is_whitespace() || c.is_control()) || n.starts_with(['/', '@']) {
        return Err("名字里不能有空白，也不能以 / 或 @ 开头".into());
    }
    if body.trim().is_empty() {
        return Err("内容不能为空".into());
    }
    if body.len() > MAX_BODY {
        return Err("内容太长了（上限 20000 字节）".into());
    }
    Ok(())
}

fn view(r: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": r.try_get::<String, _>("id").unwrap_or_default(),
        "name": r.try_get::<String, _>("name").unwrap_or_default(),
        "body": r.try_get::<String, _>("body").unwrap_or_default(),
        "updated_at": r.try_get::<String, _>("updated_at").unwrap_or_default(),
    })
}

pub async fn list(State(st): State<Shared>) -> Response {
    let rows = sqlx::query("SELECT id, name, body, updated_at FROM snippets ORDER BY name")
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
    Json(rows.iter().map(view).collect::<Vec<_>>()).into_response()
}

#[derive(Deserialize)]
pub struct SnippetBody {
    pub name: String,
    pub body: String,
}

fn conflict_or(e: &sqlx::Error) -> Response {
    if e.to_string().contains("UNIQUE") {
        fail(StatusCode::CONFLICT, "已经有一个同名的片段了")
    } else {
        fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

pub async fn create(State(st): State<Shared>, Json(b): Json<SnippetBody>) -> Response {
    if let Err(e) = check(&b.name, &b.body) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    let id = uuid::Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    match sqlx::query(
        "INSERT INTO snippets (id, name, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
    )
    .bind(&id)
    .bind(b.name.trim())
    .bind(&b.body)
    .bind(&now)
    .execute(st.db.pool())
    .await
    {
        Ok(_) => Json(json!({ "id": id })).into_response(),
        Err(e) => conflict_or(&e),
    }
}

pub async fn update(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<SnippetBody>,
) -> Response {
    if let Err(e) = check(&b.name, &b.body) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    match sqlx::query("UPDATE snippets SET name = ?2, body = ?3, updated_at = ?4 WHERE id = ?1")
        .bind(&id)
        .bind(b.name.trim())
        .bind(&b.body)
        .bind(Utc::now().to_rfc3339())
        .execute(st.db.pool())
        .await
    {
        Ok(r) if r.rows_affected() == 0 => fail(StatusCode::NOT_FOUND, "没有这个片段"),
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) => conflict_or(&e),
    }
}

pub async fn delete(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let _ = sqlx::query("DELETE FROM snippets WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await;
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_must_be_typeable_and_bodies_non_empty() {
        assert!(check("验收清单", "- 跑测试").is_ok());
        assert!(check("pr-checklist", "x").is_ok());
        for bad in ["", "  ", "a b", "/cmd", "@x", &"长".repeat(41)] {
            assert!(check(bad, "x").is_err(), "{bad}");
        }
        assert!(check("ok", "   ").is_err());
        assert!(check("ok", &"x".repeat(MAX_BODY + 1)).is_err());
    }
}
