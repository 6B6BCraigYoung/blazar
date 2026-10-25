use super::*;

pub const KIND: &str = "assistant";
pub const NAME: &str = "对话";

pub(crate) fn dir(st: &AppState) -> std::path::PathBuf {
    st.mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
        .join("assistant")
}

pub(crate) async fn find(st: &AppState) -> anyhow::Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM workspaces WHERE kind = ?1 ORDER BY created_at LIMIT 1")
            .bind(KIND)
            .fetch_optional(st.db.pool())
            .await?,
    )
}

pub(crate) async fn is_assistant(st: &AppState, id: &str) -> bool {
    sqlx::query_scalar::<_, String>("SELECT kind FROM workspaces WHERE id = ?1")
        .bind(id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .is_some_and(|k| k == KIND)
}

pub(crate) async fn ensure(st: &AppState) -> ApiResult<String> {
    if let Some(id) = find(st).await? {
        return Ok(id);
    }
    let dir = dir(st);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError(anyhow::anyhow!("创建对话目录失败: {e}")))?;
    let now = Utc::now().to_rfc3339();
    let node_id = super::workspaces::ensure_node(st, "local", &now).await?;
    sqlx::query(
        "INSERT INTO workspaces (id, node_id, name, path, kind, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (node_id, path) DO UPDATE SET kind = excluded.kind, name = excluded.name",
    )
    .bind(WorkspaceId::new().to_string())
    .bind(&node_id)
    .bind(NAME)
    .bind(dir.display().to_string())
    .bind(KIND)
    .bind(&now)
    .execute(st.db.pool())
    .await?;
    let id = find(st)
        .await?
        .ok_or_else(|| ApiError(anyhow::anyhow!("对话工作区没有建起来")))?;
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(id)
}

pub async fn get_assistant(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    match find(&st).await? {
        Some(id) => Ok(Json(describe(&st, &id).await?)),
        None => Ok(Json(serde_json::json!({ "id": null, "threads": [] }))),
    }
}

pub async fn open_assistant(State(st): State<Shared>) -> ApiResult<Json<serde_json::Value>> {
    let id = ensure(&st).await?;
    Ok(Json(describe(&st, &id).await?))
}

async fn describe(st: &Shared, id: &str) -> ApiResult<serde_json::Value> {
    let (activity, path): (String, String) =
        sqlx::query_as("SELECT activity, path FROM workspaces WHERE id = ?1")
            .bind(id)
            .fetch_one(st.db.pool())
            .await?;
    let mut threads = super::workspace_sessions(State(st.clone()), Path(id.to_owned()))
        .await?
        .0;
    for t in &mut threads {
        let thread = t["id"].as_str().unwrap_or_default().to_owned();
        t["summary"] = serde_json::Value::String(summary(st, &thread).await);
        if t["status"].as_str() == Some("running") && activity == "awaiting_approval" {
            t["status"] = serde_json::Value::String("awaiting_approval".into());
        }
    }
    Ok(serde_json::json!({ "id": id, "path": path, "threads": threads }))
}

async fn summary(st: &AppState, thread: &str) -> String {
    let payload: Option<String> = sqlx::query_scalar(
        "SELECT e.payload FROM events e JOIN sessions x ON x.id = e.session_id
         WHERE COALESCE(x.thread_id, x.id) = ?1 AND e.parent_tool_id IS NULL
           AND (e.payload LIKE '{\"type\":\"assistant_message\"%'
             OR e.payload LIKE '{\"type\":\"finished\"%'
             OR e.payload LIKE '{\"type\":\"user_message\"%')
         ORDER BY x.created_at DESC, e.seq DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    payload
        .as_deref()
        .and_then(super::view::step_of)
        .map(|s| s.text)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok<T>(result: ApiResult<T>) -> T {
        result.unwrap_or_else(|error| panic!("{}", error.message()))
    }

    async fn state() -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().join("mesh-staging")),
            crate::services::Services::Isolated,
        );
        (dir, st)
    }

    #[tokio::test]
    async fn the_assistant_workspace_is_created_once_under_the_data_dir() {
        let (dir, st) = state().await;
        let first = ok(ensure(&st).await);
        let second = ok(ensure(&st).await);
        assert_eq!(first, second);
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT name, path, kind FROM workspaces")
                .fetch_all(st.db.pool())
                .await
                .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, NAME);
        assert_eq!(
            rows[0].1,
            dir.path().join("assistant").display().to_string()
        );
        assert_eq!(rows[0].2, KIND);
        assert!(dir.path().join("assistant").is_dir());
        assert!(is_assistant(&st, &first).await);
    }

    #[tokio::test]
    async fn lists_keep_the_assistant_apart() {
        let (dir, st) = state().await;
        let id = ok(ensure(&st).await);
        let now = Utc::now().to_rfc3339();
        let node = ok(super::super::workspaces::ensure_node(&st, "local", &now).await);
        sqlx::query(
            "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('p1', ?1, 'repo', ?2, ?3)",
        )
        .bind(&node)
        .bind(dir.path().join("repo").display().to_string())
        .bind(&now)
        .execute(st.db.pool())
        .await
        .unwrap();
        let snap = super::super::view::snapshot(&st).await.unwrap();
        assert_eq!(
            snap.workspaces
                .iter()
                .map(|w| w.id.as_str())
                .collect::<Vec<_>>(),
            ["p1"]
        );
        assert_eq!(
            snap.assistant.as_ref().map(|w| w.id.as_str()),
            Some(id.as_str())
        );
        assert_eq!(
            snap.nodes
                .iter()
                .find(|n| n.name == "local")
                .unwrap()
                .workspace_count,
            1
        );
        let listed = ok(super::super::list_workspaces(State(st.clone())).await).0;
        assert!(listed.iter().all(|w| w.id != id));
        let described = ok(get_assistant(State(st.clone())).await).0;
        assert_eq!(described["id"], id);
        assert!(!is_assistant(&st, "p1").await);
    }

    #[tokio::test]
    async fn deleting_a_chat_removes_its_sessions_and_settles_the_activity() {
        let (_dir, st) = state().await;
        let id = ok(ensure(&st).await);
        for (sid, thread, status, at) in [
            ("s1", "s1", "done", "1"),
            ("s2", "s1", "failed", "2"),
            ("s3", "s3", "failed", "3"),
        ] {
            sqlx::query(
                "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at, thread_id)
                 VALUES (?1, ?2, 'claude', ?3, ?4, ?5)",
            )
            .bind(sid)
            .bind(&id)
            .bind(status)
            .bind(at)
            .bind(thread)
            .execute(st.db.pool())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, 1, ?2, '{\"type\":\"user_message\",\"text\":\"hi\"}')",
            )
            .bind(sid)
            .bind(at)
            .execute(st.db.pool())
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO inbox (id, kind, title, workspace_id, thread_id, created_at) VALUES ('i1', 'run_failed', 't', ?1, 's3', '3')",
        )
        .bind(&id)
        .execute(st.db.pool())
        .await
        .unwrap();
        sqlx::query("UPDATE workspaces SET activity = 'errored' WHERE id = ?1")
            .bind(&id)
            .execute(st.db.pool())
            .await
            .unwrap();
        let code = ok(super::super::delete_thread(
            State(st.clone()),
            Path((id.clone(), "s3".into())),
        )
        .await);
        assert_eq!(code, StatusCode::NO_CONTENT);
        let left: Vec<String> = sqlx::query_scalar("SELECT id FROM sessions ORDER BY id")
            .fetch_all(st.db.pool())
            .await
            .unwrap();
        assert_eq!(left, ["s1", "s2"]);
        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(st.db.pool())
            .await
            .unwrap();
        assert_eq!(events, 2);
        let inbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox")
            .fetch_one(st.db.pool())
            .await
            .unwrap();
        assert_eq!(inbox, 0);
        let activity: String = sqlx::query_scalar("SELECT activity FROM workspaces WHERE id = ?1")
            .bind(&id)
            .fetch_one(st.db.pool())
            .await
            .unwrap();
        assert_eq!(activity, "errored");
        ok(super::super::delete_thread(State(st.clone()), Path((id.clone(), "s1".into()))).await);
        let activity: String = sqlx::query_scalar("SELECT activity FROM workspaces WHERE id = ?1")
            .bind(&id)
            .fetch_one(st.db.pool())
            .await
            .unwrap();
        assert_eq!(activity, "idle");
        let gone = super::super::delete_thread(State(st.clone()), Path((id.clone(), "s1".into())))
            .await
            .err()
            .map(|e| e.message())
            .unwrap_or_default();
        assert!(gone.contains("不存在"), "{gone}");
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES ('s9', ?1, 'claude', 'running', '9')",
        )
        .bind(&id)
        .execute(st.db.pool())
        .await
        .unwrap();
        let busy = super::super::delete_thread(State(st.clone()), Path((id, "s9".into())))
            .await
            .err()
            .map(|e| e.message())
            .unwrap_or_default();
        assert!(busy.contains("还在运行"), "{busy}");
    }

    #[tokio::test]
    async fn without_an_assistant_the_api_answers_empty() {
        let (_dir, st) = state().await;
        let described = ok(get_assistant(State(st.clone())).await).0;
        assert!(described["id"].is_null());
        assert_eq!(described["threads"], serde_json::json!([]));
    }
}
