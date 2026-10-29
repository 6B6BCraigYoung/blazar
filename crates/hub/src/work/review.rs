//! 差异视图里的行级审阅意见：按工作区存在 hub 上，换设备、清缓存都还在，随下一条消息发出后清空。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::api::Shared;

const MAX_TEXT: usize = 20_000;
const MAX_BATCH: usize = 500;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub file: String,
    pub side: String,
    pub line: u32,
    #[serde(default)]
    pub code: String,
    pub text: String,
}

fn check(c: &Comment) -> Result<(), &'static str> {
    if c.id.trim().is_empty() || c.id.len() > 64 {
        return Err("意见的 id 不对");
    }
    if c.file.trim().is_empty() || !matches!(c.side.as_str(), "old" | "new") {
        return Err("意见没指明是哪个文件的哪一行");
    }
    if c.text.trim().is_empty() {
        return Err("意见不能为空");
    }
    if c.text.len() + c.code.len() > MAX_TEXT {
        return Err("意见太长了（上限 20000 字节）");
    }
    Ok(())
}

async fn exists(st: &Shared, ws: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workspaces WHERE id = ?1")
        .bind(ws)
        .fetch_one(st.db.pool())
        .await
        .unwrap_or(0)
        > 0
}

pub async fn list(State(st): State<Shared>, Path(ws): Path<String>) -> Response {
    let rows = match sqlx::query(
        "SELECT id, file, side, line, code, text FROM review_comments
         WHERE workspace_id = ?1 ORDER BY created_at, rowid",
    )
    .bind(&ws)
    .fetch_all(st.db.pool())
    .await
    {
        Ok(r) => r,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let list: Vec<Comment> = rows
        .iter()
        .map(|r| Comment {
            id: r.try_get("id").unwrap_or_default(),
            file: r.try_get("file").unwrap_or_default(),
            side: r.try_get("side").unwrap_or_default(),
            line: r
                .try_get::<i64, _>("line")
                .ok()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            code: r.try_get("code").unwrap_or_default(),
            text: r.try_get("text").unwrap_or_default(),
        })
        .collect();
    Json(list).into_response()
}

#[derive(Debug, Deserialize)]
pub struct SaveBody {
    pub comments: Vec<Comment>,
}

/// 按 id 新建或改写；网页第一次打开时把浏览器里旧的意见一次性搬上来也走这里。
pub async fn save(
    State(st): State<Shared>,
    Path(ws): Path<String>,
    Json(b): Json<SaveBody>,
) -> Response {
    if b.comments.len() > MAX_BATCH {
        return fail(StatusCode::BAD_REQUEST, "一次存的意见太多了");
    }
    if let Some(e) = b.comments.iter().find_map(|c| check(c).err()) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    if !exists(&st, &ws).await {
        return fail(StatusCode::NOT_FOUND, "没有这个工作区");
    }
    let now = Utc::now().to_rfc3339();
    let mut tx = match st.db.pool().begin().await {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    for c in &b.comments {
        let r = sqlx::query(
            "INSERT INTO review_comments (id, workspace_id, file, side, line, code, text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (workspace_id, id) DO UPDATE SET
               file = excluded.file, side = excluded.side, line = excluded.line,
               code = excluded.code, text = excluded.text",
        )
        .bind(&c.id)
        .bind(&ws)
        .bind(&c.file)
        .bind(&c.side)
        .bind(i64::from(c.line))
        .bind(&c.code)
        .bind(&c.text)
        .bind(&now)
        .execute(&mut *tx)
        .await;
        if let Err(e) = r {
            return fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    match tx.commit().await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn delete_one(
    State(st): State<Shared>,
    Path((ws, id)): Path<(String, String)>,
) -> Response {
    let _ = sqlx::query("DELETE FROM review_comments WHERE workspace_id = ?1 AND id = ?2")
        .bind(&ws)
        .bind(&id)
        .execute(st.db.pool())
        .await;
    StatusCode::NO_CONTENT.into_response()
}

/// 意见随消息发出去之后整批清掉。
pub async fn clear(State(st): State<Shared>, Path(ws): Path<String>) -> Response {
    let _ = sqlx::query("DELETE FROM review_comments WHERE workspace_id = ?1")
        .bind(&ws)
        .execute(st.db.pool())
        .await;
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> Shared {
        let st = crate::state::AppState::with_services(
            blazar_db::Db::open_in_memory().await.unwrap(),
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, std::env::temp_dir()),
            crate::services::Services::Isolated,
        );
        sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
            .execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('ws', 'node', 'repo', '/tmp/x', '0')")
            .execute(st.db.pool()).await.unwrap();
        st
    }

    fn comment(id: &str, text: &str) -> Comment {
        Comment {
            id: id.into(),
            file: "src/a.rs".into(),
            side: "new".into(),
            line: 3,
            code: "let x = 1;".into(),
            text: text.into(),
        }
    }

    async fn listed(st: &Shared) -> Vec<Comment> {
        let r = list(State(st.clone()), Path("ws".into())).await;
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn comments_are_upserted_listed_and_deleted_per_workspace() {
        let st = fixture().await;
        let r = save(
            State(st.clone()),
            Path("ws".into()),
            Json(SaveBody {
                comments: vec![comment("a", "rename this"), comment("b", "missing test")],
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        save(
            State(st.clone()),
            Path("ws".into()),
            Json(SaveBody {
                comments: vec![comment("a", "rename this to total")],
            }),
        )
        .await;
        let got = listed(&st).await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].text, "rename this to total");
        assert_eq!(got[1], comment("b", "missing test"));

        delete_one(State(st.clone()), Path(("ws".into(), "a".into()))).await;
        assert_eq!(listed(&st).await, vec![comment("b", "missing test")]);
        clear(State(st.clone()), Path("ws".into())).await;
        assert!(listed(&st).await.is_empty());
    }

    #[tokio::test]
    async fn bad_comments_and_unknown_workspaces_are_refused() {
        let st = fixture().await;
        let r = save(
            State(st.clone()),
            Path("ws".into()),
            Json(SaveBody {
                comments: vec![comment("a", "  ")],
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = save(
            State(st.clone()),
            Path("nope".into()),
            Json(SaveBody {
                comments: vec![comment("a", "x")],
            }),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
}
