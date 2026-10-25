use super::*;
use blazar_core_types::api::{StateSnapshot, WorkspaceStep};

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
    let (activity, path, current): (String, String, Option<String>) = sqlx::query_as(
        "SELECT w.activity, w.path,
                (SELECT s.id FROM sessions s WHERE s.workspace_id = w.id
                 ORDER BY s.created_at DESC LIMIT 1)
         FROM workspaces w WHERE w.id = ?1",
    )
    .bind(id)
    .fetch_one(st.db.pool())
    .await?;
    let mut threads = super::workspace_sessions(State(st.clone()), Path(id.to_owned()))
        .await?
        .0;
    for t in &mut threads {
        let thread = t["id"].as_str().unwrap_or_default().to_owned();
        t["summary"] = serde_json::Value::String(summary(st, &thread).await);
        if matches!(activity.as_str(), "running" | "awaiting_approval")
            && t["latest"].as_str() == current.as_deref()
        {
            t["status"] = serde_json::Value::String(activity.clone());
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
    payload.as_deref().map(last_sentence).unwrap_or_default()
}

const SUMMARY_MAX: usize = 160;

#[must_use]
pub fn last_sentence(payload: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
    let text = match v["type"].as_str() {
        Some("finished") if v["status"] == "failed" => v["message"].as_str(),
        _ => v["text"].as_str(),
    }
    .unwrap_or_default();
    let prose = |line: &str| {
        let l = line.trim();
        !l.is_empty()
            && !l.starts_with('|')
            && !l.starts_with('#')
            && !l.starts_with("```")
            && !l.starts_with("- ")
            && !l.starts_with("* ")
            && !l.starts_with(char::is_numeric)
    };
    let line = text
        .lines()
        .rev()
        .find(|l| prose(l))
        .or_else(|| text.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or_default()
        .trim();
    if line.chars().count() > SUMMARY_MAX {
        let mut cut: String = line.chars().take(SUMMARY_MAX - 1).collect();
        cut.push('…');
        cut
    } else {
        line.to_owned()
    }
}

pub(crate) async fn briefing(st: &AppState) -> String {
    match super::view::snapshot(st).await {
        Ok(snap) => brief(&snap),
        Err(_) => String::new(),
    }
}

#[must_use]
pub fn brief(snap: &StateSnapshot) -> String {
    let dir = snap
        .assistant
        .as_ref()
        .map(|w| w.path.clone())
        .unwrap_or_default();
    let mut out = format!(
        "你是用户的助手，不属于任何项目。这个目录（{dir}）只是你的工作目录，用户的项目都在下面列出的工作区里。\n\
         用 blazar-fleet 的工具看和管它们：list_workspaces、get_history、get_diff、read_file 看情况，probe_node 看机器；\
         send_prompt 给某个工作区派活（立即返回），wait_workspace 等它停下并拿到最后一句回复，copy_files 在工作区之间搬文件。\n\
         动别的工作区之前，先用一两句话说清楚要对哪个工作区做什么，再动手。\n"
    );
    if snap.workspaces.is_empty() {
        out.push_str("现在还没有工作区。\n");
    } else {
        out.push_str("现在的工作区：\n");
        for w in &snap.workspaces {
            let step = w
                .step
                .as_ref()
                .map(step_line)
                .filter(|s| !s.is_empty())
                .map(|s| format!("，{s}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- {}：机器 {}，{}{step}；工作区 id {}\n",
                w.name,
                w.node,
                state_label(&w.activity),
                w.id
            ));
        }
    }
    out.push_str("机器：\n");
    for n in &snap.nodes {
        let online = n.name == "local" || n.status == "online";
        out.push_str(&format!(
            "- {}：{}\n",
            n.name,
            if online { "在线" } else { "离线" }
        ));
    }
    out
}

fn state_label(activity: &str) -> &'static str {
    match activity {
        "running" | "starting" => "运行中",
        "awaiting_approval" => "等用户批准",
        "errored" => "上一轮失败了",
        "completed" => "刚跑完",
        _ => "空闲",
    }
}

fn step_line(step: &WorkspaceStep) -> String {
    match step.kind.as_str() {
        "tool" => {
            let arg = ["command", "file_path", "description", "pattern"]
                .iter()
                .find_map(|k| step.input.get(*k).and_then(serde_json::Value::as_str))
                .unwrap_or_default();
            if arg.is_empty() {
                format!("正在用 {}", step.tool)
            } else {
                format!("正在用 {} {arg}", step.tool)
            }
        }
        "retry" => "正在重试".into(),
        "error" => format!("失败：{}", step.text),
        _ => step.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blazar_core_types::api::{NodeView, WorkspaceView};

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

    #[test]
    fn a_summary_is_the_last_plain_sentence() {
        let table = serde_json::json!({
            "type": "finished", "status": "success",
            "text": "三个都看了：\n\n| 工作区 | 状态 |\n| --- | --- |\n| atlas-api | 已完成 |\n\n只有 quasar-ml 在等你。"
        });
        assert_eq!(last_sentence(&table.to_string()), "只有 quasar-ml 在等你。");
        let failed = serde_json::json!({
            "type": "finished", "status": "failed", "message": "API Error: 529 overloaded\nretrying"
        });
        assert_eq!(last_sentence(&failed.to_string()), "retrying");
        let said = serde_json::json!({ "type": "assistant_message", "text": "- 第一点\n- 第二点" });
        assert_eq!(last_sentence(&said.to_string()), "- 第一点");
        let long = serde_json::json!({ "type": "user_message", "text": "好".repeat(200) });
        let cut = last_sentence(&long.to_string());
        assert_eq!(cut.chars().count(), SUMMARY_MAX);
        assert!(cut.ends_with('…'));
        assert_eq!(last_sentence("not json"), "");
    }

    #[tokio::test]
    async fn without_an_assistant_the_api_answers_empty() {
        let (_dir, st) = state().await;
        let described = ok(get_assistant(State(st.clone())).await).0;
        assert!(described["id"].is_null());
        assert_eq!(described["threads"], serde_json::json!([]));
    }

    #[test]
    fn the_briefing_names_every_workspace_and_machine() {
        let node = |name: &str, status: &str| NodeView {
            name: name.into(),
            transport: "ssh".into(),
            ipv4: None,
            status: status.into(),
            latency_ms: None,
            cost: None,
            workspace_count: 1,
            network: None,
            role: None,
            last_seen_at: None,
        };
        let ws =
            |id: &str, node: &str, activity: &str, step: Option<WorkspaceStep>| WorkspaceView {
                id: id.into(),
                name: id.into(),
                node: node.into(),
                path: format!("/srv/{id}"),
                project: None,
                activity: activity.into(),
                last_active_at: None,
                session_id: None,
                diff: None,
                diff_at: None,
                parent: None,
                step,
                failure: None,
            };
        let snap = StateSnapshot {
            nodes: vec![node("local", "offline"), node("gpu-a", "offline")],
            workspaces: vec![
                ws(
                    "atlas-api",
                    "local",
                    "running",
                    Some(WorkspaceStep {
                        kind: "tool".into(),
                        tool: "Bash".into(),
                        input: serde_json::json!({ "command": "pytest -q" }),
                        text: String::new(),
                    }),
                ),
                ws("quasar-train", "gpu-a", "awaiting_approval", None),
            ],
            assistant: Some(ws("helper", "local", "idle", None)),
        };
        let text = brief(&snap);
        assert!(text.contains("/srv/helper"), "{text}");
        assert!(
            text.contains("atlas-api")
                && text.contains("运行中")
                && text.contains("Bash pytest -q")
        );
        assert!(text.contains("quasar-train") && text.contains("等用户批准"));
        assert!(text.contains("- local：在线") && text.contains("- gpu-a：离线"));
        assert!(text.contains("send_prompt") && text.contains("wait_workspace"));
        assert!(!text.contains("helper：机器"));
        let empty = brief(&StateSnapshot::default());
        assert!(empty.contains("还没有工作区"));
    }
}
