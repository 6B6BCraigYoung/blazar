use super::*;
use axum::http::HeaderMap;
use blazar_core_types::EntryKind;
use blazar_core_types::api::Ask;
use sqlx::Row;

const QUESTION_MAX: usize = 200;
const OPTIONS_MAX: usize = 6;
const OPTION_MAX: usize = 60;
const CONTEXT_MAX: usize = 600;

#[derive(Debug, Default, Deserialize)]
pub struct AskRequest {
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub context: String,
}

#[derive(Debug, Deserialize)]
pub struct AnswerRequest {
    pub text: String,
}

pub(crate) fn normalize(req: &AskRequest) -> Result<Ask, String> {
    let question = req.question.trim();
    if question.is_empty() {
        return Err("问题不能是空的".into());
    }
    let mut options: Vec<String> = Vec::new();
    for option in &req.options {
        let option: String = option.trim().chars().take(OPTION_MAX).collect();
        if option.is_empty() || options.contains(&option) {
            continue;
        }
        options.push(option);
        if options.len() == OPTIONS_MAX {
            break;
        }
    }
    Ok(Ask {
        question: question.chars().take(QUESTION_MAX).collect(),
        options,
    })
}

pub async fn create_ask(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<AskRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = headers
        .get(crate::bots::jobs::SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("只有会话能发这张卡片"))?;
    let ask = normalize(&req).map_err(ApiError::bad_request)?;
    let context: String = req.context.trim().chars().take(CONTEXT_MAX).collect();
    let id = post(&st, sid, &ask, &context).await?;
    Ok(Json(serde_json::json!({
        "id": id,
        "question": ask.question,
        "options": ask.options,
        "hint": "卡片已放进「需要你处理」。现在结束这一轮等用户回答，回答会以新的用户消息出现在这条对话里。",
    })))
}

pub(crate) async fn post(st: &Shared, sid: &str, ask: &Ask, context: &str) -> ApiResult<String> {
    let row = sqlx::query(
        "SELECT workspace_id, COALESCE(thread_id, id) AS thread FROM sessions WHERE id = ?1",
    )
    .bind(sid)
    .fetch_optional(st.db.pool())
    .await?;
    let Some(row) = row else {
        return Err(ApiError(anyhow::Error::new(NotFound("会话不存在".into()))));
    };
    let workspace_id: String = row.try_get("workspace_id")?;
    let thread: String = row.try_get("thread")?;
    crate::inbox::resolve_asks(st, &thread).await;
    sqlx::query("UPDATE sessions SET ask = ?2 WHERE id = ?1")
        .bind(sid)
        .bind(serde_json::to_string(ask).unwrap_or_default())
        .execute(st.db.pool())
        .await?;
    let id = crate::inbox::push_id(
        st,
        crate::inbox::Item {
            kind: "question",
            title: format!("待你决定：{}", ask.question),
            body: context.to_owned(),
            workspace_id: Some(workspace_id),
            thread_id: Some(thread),
            session_id: Some(sid.to_owned()),
            ref_id: Some(format!("ask:{sid}")),
        },
    )
    .await
    .ok_or_else(|| ApiError(anyhow::anyhow!("卡片没有存进收件箱")))?;
    st.emit(ServerEvent::WorkspacesChanged);
    Ok(id)
}

async fn thread_permission_mode(st: &Shared, thread: &str) -> Option<String> {
    let sid: String = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE COALESCE(thread_id, id) = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()?;
    let sid = SessionId(sid.parse().ok()?);
    let events = st.db.events_since(sid, 0).await.ok()?;
    events.iter().find_map(|e| match &e.kind {
        EntryKind::SessionStarted {
            permission_mode, ..
        } => permission_mode.clone(),
        _ => None,
    })
}

pub async fn answer_ask(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(req): Json<AnswerRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let text = req.text.trim().to_owned();
    if text.is_empty() {
        return Err(ApiError::bad_request("回复不能是空的"));
    }
    let row = sqlx::query(
        "SELECT workspace_id, thread_id, kind, ref_id FROM inbox WHERE id = ?1 AND archived_at IS NULL",
    )
    .bind(&id)
    .fetch_optional(st.db.pool())
    .await?;
    let Some(row) = row else {
        return Err(ApiError(anyhow::Error::new(NotFound(
            "这张卡片不存在，或者已经处理过了".into(),
        ))));
    };
    let kind: String = row.try_get("kind")?;
    let ref_id: Option<String> = row.try_get("ref_id")?;
    if kind != "question" || !ref_id.as_deref().is_some_and(|r| r.starts_with("ask:")) {
        return Err(ApiError::bad_request("这张卡片不是在问你的问题"));
    }
    let workspace_id: Option<String> = row.try_get("workspace_id")?;
    let thread: Option<String> = row.try_get("thread_id")?;
    let (Some(workspace_id), Some(thread)) = (workspace_id, thread) else {
        return Err(ApiError::bad_request("这张卡片没有对应的对话"));
    };
    let permission_mode = thread_permission_mode(&st, &thread).await;
    let prompt = PromptRequest {
        text,
        resume: true,
        resume_session: Some(thread.clone()),
        permission_mode,
        interject: true,
        ..Default::default()
    };
    let out = super::sessions::prompt_for_bot(st.clone(), workspace_id, prompt)
        .await
        .map_err(ApiError::bad_request)?;
    if out["admitted"] == serde_json::json!(false) {
        return Ok(Json(serde_json::json!({
            "delivered": false,
            "reason": out["reason"].as_str().unwrap_or("这个对话正在忙，稍后再回"),
        })));
    }
    crate::inbox::resolve_asks(&st, &thread).await;
    crate::inbox::mark_resolved(&st, &id).await;
    Ok(Json(serde_json::json!({
        "delivered": true,
        "session_id": out["session_id"],
        "thread_id": thread,
        "interjected": out["interjected"] == serde_json::json!(true),
    })))
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

    async fn seed_session(st: &Shared, sid: &str) {
        let now = Utc::now().to_rfc3339();
        let node = ok(super::super::workspaces::ensure_node(st, "local", &now).await);
        sqlx::query(
            "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('w1', ?1, 'w', '/tmp/w', ?2)",
        )
        .bind(&node)
        .bind(&now)
        .execute(st.db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES (?1, 'w1', 'claude', 'done', ?2)",
        )
        .bind(sid)
        .bind(&now)
        .execute(st.db.pool())
        .await
        .unwrap();
    }

    #[test]
    fn normalize_trims_dedupes_and_caps_options() {
        let req = AskRequest {
            question: "  推不推？ ".into(),
            options: vec![" 推 ".into(), "推".into(), String::new(), "不推".into()],
            context: String::new(),
        };
        let ask = normalize(&req).unwrap();
        assert_eq!(ask.question, "推不推？");
        assert_eq!(ask.options, vec!["推", "不推"]);
        let many = AskRequest {
            question: "q".into(),
            options: (0..9).map(|i| i.to_string()).collect(),
            context: String::new(),
        };
        assert_eq!(normalize(&many).unwrap().options.len(), OPTIONS_MAX);
        assert!(normalize(&AskRequest::default()).is_err());
    }

    #[tokio::test]
    async fn posting_twice_keeps_one_open_card_and_stores_the_options() {
        let (_dir, st) = state().await;
        let sid = SessionId::new().to_string();
        seed_session(&st, &sid).await;
        let first = normalize(&AskRequest {
            question: "先推还是先装？".into(),
            options: vec!["先推".into(), "先装".into()],
            context: String::new(),
        })
        .unwrap();
        let a = ok(post(&st, &sid, &first, "两批一起装").await);
        let second = normalize(&AskRequest {
            question: "改主意了，装吗？".into(),
            options: vec![],
            context: String::new(),
        })
        .unwrap();
        let b = ok(post(&st, &sid, &second, "").await);
        assert_ne!(a, b);
        let open: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, title, body FROM inbox WHERE kind = 'question' AND archived_at IS NULL",
        )
        .fetch_all(st.db.pool())
        .await
        .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].0, b);
        assert_eq!(open[0].1, "待你决定：改主意了，装吗？");
        let stored: Option<String> = sqlx::query_scalar("SELECT ask FROM sessions WHERE id = ?1")
            .bind(&sid)
            .fetch_one(st.db.pool())
            .await
            .unwrap();
        let ask: Ask = serde_json::from_str(&stored.unwrap()).unwrap();
        assert_eq!(ask.question, "改主意了，装吗？");
        assert!(crate::inbox::has_open_ask(&st, &sid).await);
        assert!(!crate::inbox::has_open_ask(&st, "nope").await);
    }

    #[tokio::test]
    async fn posting_needs_an_existing_session() {
        let (_dir, st) = state().await;
        let ask = Ask {
            question: "q".into(),
            options: vec![],
        };
        assert!(post(&st, "missing", &ask, "").await.is_err());
    }
}
