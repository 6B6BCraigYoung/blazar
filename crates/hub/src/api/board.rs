use axum::Json;
use axum::extract::{Query, State};
use blazar_core_types::api::{Board, BoardNote, BoardRow, Progress};
use serde::Deserialize;
use sqlx::Row;

use super::view::{PROGRESS, progress_of};
use super::{ApiResult, Shared};

const RECENT: i64 = 10;

const FAMILY: &str = "WITH RECURSIVE fam(id) AS (
    SELECT w.id FROM workspaces w JOIN projects p ON p.id = w.project_id WHERE p.name = ?1
    UNION
    SELECT c.id FROM workspaces c JOIN fam f ON c.parent_id = f.id
)";

#[derive(Deserialize)]
pub struct BoardQuery {
    pub name: String,
}

fn text(r: &sqlx::sqlite::SqliteRow, k: &str) -> Option<String> {
    r.try_get::<Option<String>, _>(k)
        .ok()
        .flatten()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

pub async fn project_board(
    State(st): State<Shared>,
    Query(q): Query<BoardQuery>,
) -> ApiResult<Json<Board>> {
    let pool = st.db.pool();
    let name = q.name.trim();
    let sql = format!(
        "{FAMILY}
         SELECT w.id, w.name, w.path, w.parent_id, w.activity, w.last_active_at, w.archived_at,
                n.name AS node, {PROGRESS},
                (SELECT r.title FROM sessions s JOIN sessions r ON r.id = COALESCE(s.thread_id, s.id)
                 WHERE s.workspace_id = w.id ORDER BY s.created_at DESC LIMIT 1) AS doing,
                (SELECT s.summary FROM sessions s WHERE s.workspace_id = w.id
                 AND s.summary IS NOT NULL AND s.summary <> '' ORDER BY s.created_at DESC LIMIT 1) AS summary,
                (SELECT i.title FROM inbox i WHERE i.workspace_id = w.id AND i.kind = 'report'
                 ORDER BY i.created_at DESC LIMIT 1) AS report,
                (SELECT i.created_at FROM inbox i WHERE i.workspace_id = w.id AND i.kind = 'report'
                 ORDER BY i.created_at DESC LIMIT 1) AS report_at,
                (SELECT COUNT(*) FROM inbox i WHERE i.workspace_id = w.id
                 AND i.kind IN ('approval', 'question') AND i.archived_at IS NULL) AS waiting
         FROM workspaces w JOIN nodes n ON n.id = w.node_id
         WHERE w.id IN (SELECT id FROM fam)
         ORDER BY w.name"
    );
    let rows = sqlx::query(&sql).bind(name).fetch_all(pool).await?;
    let rows = rows
        .iter()
        .map(|r| {
            let active = text(r, "last_active_at");
            let reported = text(r, "report_at");
            BoardRow {
                id: r.try_get("id").unwrap_or_default(),
                name: r.try_get("name").unwrap_or_default(),
                node: r.try_get("node").unwrap_or_default(),
                path: r.try_get("path").unwrap_or_default(),
                parent: text(r, "parent_id"),
                archived: text(r, "archived_at").is_some(),
                activity: text(r, "activity").unwrap_or_else(|| "idle".into()),
                doing: text(r, "doing"),
                latest: text(r, "report").or_else(|| text(r, "summary")),
                updated_at: active.max(reported),
                waiting: r
                    .try_get::<i64, _>("waiting")
                    .ok()
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
                progress: progress_of(r),
            }
        })
        .collect();
    let notes = sqlx::query(&format!(
        "{FAMILY}
         SELECT i.id, i.workspace_id, i.title, i.body, i.created_at,
                i.progress_done, i.progress_total, i.progress_unit,
                w.name AS workspace, n.name AS node
         FROM inbox i
         JOIN workspaces w ON w.id = i.workspace_id
         JOIN nodes n ON n.id = w.node_id
         WHERE i.kind = 'report' AND i.workspace_id IN (SELECT id FROM fam)
         ORDER BY i.created_at DESC LIMIT ?2"
    ))
    .bind(name)
    .bind(RECENT)
    .fetch_all(pool)
    .await?;
    let recent = notes
        .iter()
        .map(|r| {
            let at: String = r.try_get("created_at").unwrap_or_default();
            let total = r.try_get::<Option<i64>, _>("progress_total").ok().flatten();
            let title: String = r.try_get("title").unwrap_or_default();
            BoardNote {
                id: r.try_get("id").unwrap_or_default(),
                workspace_id: r.try_get("workspace_id").unwrap_or_default(),
                workspace: r.try_get("workspace").unwrap_or_default(),
                node: r.try_get("node").unwrap_or_default(),
                body: r.try_get("body").unwrap_or_default(),
                progress: total
                    .and_then(|t| u32::try_from(t).ok())
                    .map(|total| Progress {
                        done: r
                            .try_get::<Option<i64>, _>("progress_done")
                            .ok()
                            .flatten()
                            .and_then(|d| u32::try_from(d).ok())
                            .unwrap_or(0),
                        total,
                        unit: text(r, "progress_unit").unwrap_or_default(),
                        title: title.clone(),
                        at: at.clone(),
                    }),
                title,
                at,
            }
        })
        .collect();
    Ok(Json(Board { rows, recent }))
}
