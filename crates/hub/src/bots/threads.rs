use chrono::Utc;
use sqlx::SqliteConnection;

use crate::api::Shared;

use super::{Caller, THREAD_KINDS, Thread, changed, thread_of};

pub async fn threads(st: &Shared, bot_id: &str) -> Vec<Thread> {
    sqlx::query("SELECT * FROM bot_threads WHERE bot_id = ?1 ORDER BY created_at")
        .bind(bot_id)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(thread_of)
        .collect()
}

pub async fn thread(st: &Shared, id: &str) -> Option<Thread> {
    sqlx::query("SELECT * FROM bot_threads WHERE id = ?1")
        .bind(id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| thread_of(&r))
}

pub async fn thread_by_session_thread(st: &Shared, thread_id: &str) -> Option<Thread> {
    sqlx::query("SELECT * FROM bot_threads WHERE thread_id = ?1 AND closed_at IS NULL LIMIT 1")
        .bind(thread_id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| thread_of(&r))
}

pub async fn main_thread(st: &Shared, bot_id: &str) -> Result<Thread, String> {
    let found = sqlx::query(
        "SELECT * FROM bot_threads WHERE bot_id = ?1 AND kind = 'main' AND closed_at IS NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(bot_id)
    .fetch_optional(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    match found {
        Some(r) => Ok(thread_of(&r)),
        None => open_thread(st, bot_id, "main", "").await,
    }
}

pub async fn open_thread(
    st: &Shared,
    bot_id: &str,
    kind: &str,
    title: &str,
) -> Result<Thread, String> {
    if !THREAD_KINDS.contains(&kind) {
        return Err(format!("未知线程类型 {kind}"));
    }
    let id = {
        let mut conn = st.db.pool().acquire().await.map_err(|e| e.to_string())?;
        insert_thread(&mut conn, bot_id, kind, title).await?
    };
    thread(st, &id).await.ok_or_else(|| "线程读不回来".into())
}

pub(super) async fn insert_thread(
    conn: &mut SqliteConnection,
    bot_id: &str,
    kind: &str,
    title: &str,
) -> Result<String, String> {
    let id = uuid::Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO bot_threads (id, bot_id, kind, title, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
    )
    .bind(&id)
    .bind(bot_id)
    .bind(kind)
    .bind(title.chars().take(120).collect::<String>())
    .bind(&now)
    .execute(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;
    Ok(id)
}

pub async fn bind_thread(st: &Shared, id: &str, thread_id: &str, runtime: &str) {
    let _ = sqlx::query(
        "UPDATE bot_threads SET thread_id = ?2, runtime = ?3, updated_at = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(thread_id)
    .bind(runtime)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
}

pub async fn close_thread(st: &Shared, id: &str, summary: &str) -> Result<(), String> {
    let t = thread(st, id).await.ok_or("没有这个线程")?;
    if t.kind == "main" {
        let next = open_thread(st, &t.bot_id, "main", "").await?;
        let _ = sqlx::query(
            "UPDATE bot_mail SET thread_hint = ?2 WHERE thread_hint = ?1 AND status = 'queued'",
        )
        .bind(&t.id)
        .bind(&next.id)
        .execute(st.db.pool())
        .await;
    }
    sqlx::query(
        "UPDATE bot_threads SET closed_at = ?2, summary = ?3, updated_at = ?2 WHERE id = ?1",
    )
    .bind(&t.id)
    .bind(Utc::now().to_rfc3339())
    .bind(summary.chars().take(8000).collect::<String>())
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    changed(st);
    Ok(())
}

pub async fn caller_thread(st: &Shared, c: &Caller) -> Option<String> {
    let sid = c.session.as_deref()?;
    let thread: String =
        sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
            .bind(sid)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()?;
    thread_by_session_thread(st, &thread).await.map(|t| t.id)
}
