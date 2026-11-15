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
    #[serde(default)]
    pub replaces: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ReportRequest {
    #[serde(default)]
    pub level: String,
    pub text: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub done: Option<u32>,
    #[serde(default)]
    pub total: Option<u32>,
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Deserialize)]
pub struct AnswerRequest {
    pub text: String,
}

pub(crate) fn normalize(req: &AskRequest) -> Result<Ask, String> {
    let question = req.question.trim();
    if question.is_empty() {
        return Err("Question can't be empty".into());
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
        .ok_or_else(|| ApiError::bad_request("Only a session can post this card"))?;
    let ask = normalize(&req).map_err(ApiError::bad_request)?;
    let context: String = req.context.trim().chars().take(CONTEXT_MAX).collect();
    let replaces = req
        .replaces
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    if let Some(old) = replaces {
        crate::inbox::withdraw_ask(&st, sid, old)
            .await
            .map_err(ApiError::bad_request)?;
    }
    let id = post(&st, sid, &ask, &context).await?;
    Ok(Json(serde_json::json!({
        "id": id,
        "question": ask.question,
        "options": ask.options,
        "replaced": replaces,
        "hint": "The card is in the user's Needs you list. Don't wait for it: carry on with any work that doesn't depend on this answer, and end the turn only when nothing is left. The answer will arrive as a new user message in this conversation, even mid-turn. If the user answers in the chat instead, close this card with close_ask.",
    })))
}

pub async fn close_ask(
    State(st): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = headers
        .get(crate::bots::jobs::SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("Only a session can close this card"))?;
    crate::inbox::withdraw_ask(&st, sid, id.trim())
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(serde_json::json!({ "id": id, "closed": true })))
}

fn caller_session(headers: &HeaderMap) -> ApiResult<&str> {
    headers
        .get(crate::bots::jobs::SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("Only a session can report"))
}

pub async fn create_report(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<ReportRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = caller_session(&headers)?;
    let detail: String = req.detail.trim().chars().take(CONTEXT_MAX).collect();
    match req.level.as_str() {
        "decision" => {
            let ask = normalize(&AskRequest {
                question: req.text.clone(),
                options: req.options.clone(),
                ..Default::default()
            })
            .map_err(ApiError::bad_request)?;
            let id = post(&st, sid, &ask, &detail).await?;
            Ok(Json(serde_json::json!({
                "id": id,
                "level": "decision",
                "hint": "Sent as a card to the user's Needs you list. Carry on with work that doesn't depend on the answer; it arrives as a new user message.",
            })))
        }
        "info" | "" => {
            let text = req.text.trim();
            if text.is_empty() {
                return Err(ApiError::bad_request("Report text can't be empty"));
            }
            let (workspace_id, thread) = session_home(&st, sid).await?;
            let id = crate::inbox::push_id(
                &st,
                crate::inbox::Item {
                    kind: "report",
                    title: text.chars().take(QUESTION_MAX).collect(),
                    body: detail,
                    workspace_id: Some(workspace_id),
                    thread_id: Some(thread),
                    session_id: Some(sid.to_owned()),
                    ref_id: None,
                },
            )
            .await
            .ok_or_else(|| ApiError(anyhow::anyhow!("汇报没有存进收件箱")))?;
            if let Some(total) = req.total.filter(|t| *t > 0) {
                sqlx::query(
                    "UPDATE inbox SET progress_done = ?2, progress_total = ?3, progress_unit = ?4 WHERE id = ?1",
                )
                .bind(&id)
                .bind(i64::from(req.done.unwrap_or(0).min(total)))
                .bind(i64::from(total))
                .bind(req.unit.trim())
                .execute(st.db.pool())
                .await?;
                st.emit(crate::state::ServerEvent::WorkspacesChanged);
            }
            Ok(Json(serde_json::json!({
                "id": id,
                "level": "info",
                "hint": "Added to the console's report feed without disturbing the user.",
            })))
        }
        other => Err(ApiError::bad_request(format!(
            "Unknown level \"{other}\"; use info or decision"
        ))),
    }
}

async fn session_home(st: &Shared, sid: &str) -> ApiResult<(String, String)> {
    let row = sqlx::query(
        "SELECT workspace_id, COALESCE(thread_id, id) AS thread FROM sessions WHERE id = ?1",
    )
    .bind(sid)
    .fetch_optional(st.db.pool())
    .await?;
    let Some(row) = row else {
        return Err(ApiError(anyhow::Error::new(NotFound("会话不存在".into()))));
    };
    Ok((row.try_get("workspace_id")?, row.try_get("thread")?))
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
    sqlx::query("UPDATE sessions SET ask = COALESCE(ask, ?2) WHERE id = ?1")
        .bind(sid)
        .bind(serde_json::to_string(ask).unwrap_or_default())
        .execute(st.db.pool())
        .await?;
    let id = crate::inbox::push_ask(
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
        ask,
    )
    .await
    .ok_or_else(|| ApiError(anyhow::anyhow!("卡片没有存进收件箱")))?;
    if !crate::inbox::locate_ask(st, &id, sid, &ask.question).await {
        crate::inbox::anchor_at_tail(st, &id, sid).await;
        let (st2, card, sid, question) =
            (st.clone(), id.clone(), sid.to_owned(), ask.question.clone());
        st.services.spawn(async move {
            for _ in 0..12 {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if crate::inbox::locate_ask(&st2, &card, &sid, &question).await {
                    break;
                }
            }
        });
    }
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
        "SELECT i.workspace_id, i.thread_id, i.kind, i.ref_id, COALESCE(i.ask, s.ask) AS card_ask,
                (SELECT COUNT(*) FROM inbox o WHERE o.session_id = i.session_id AND o.kind = 'question'
                   AND o.ref_id = i.ref_id) AS cards
         FROM inbox i LEFT JOIN sessions s ON s.id = i.session_id
         WHERE i.id = ?1 AND i.archived_at IS NULL",
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
    let cards: i64 = row.try_get("cards").unwrap_or(0);
    let text = match crate::brief::ask_from_row(&row, "card_ask") {
        Some(ask) if cards > 1 => format!("关于「{}」：{text}", ask.question),
        _ => text,
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
    let queued = serde_json::json!({
        "text": prompt.text,
        "resume": true,
        "resume_session": thread,
        "permission_mode": prompt.permission_mode,
    });
    let out = super::sessions::prompt_for_bot(st.clone(), workspace_id.clone(), prompt)
        .await
        .map_err(ApiError::bad_request)?;
    if out["admitted"] == serde_json::json!(false) {
        let put = crate::agent::chat::put(
            State(st.clone()),
            Path(workspace_id),
            Json(crate::agent::chat::QueueBody {
                thread_id: Some(thread.clone()),
                request: queued,
                append: true,
                expected: None,
            }),
        )
        .await;
        if !put.status().is_success() {
            return Ok(Json(serde_json::json!({
                "delivered": false,
                "reason": out["reason"].as_str().unwrap_or("这个对话正在忙，稍后再回"),
            })));
        }
        crate::inbox::resolve_ask(&st, &id).await;
        return Ok(Json(serde_json::json!({
            "delivered": true,
            "queued": true,
            "thread_id": thread,
        })));
    }
    crate::inbox::resolve_ask(&st, &id).await;
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
            ..Default::default()
        };
        let ask = normalize(&req).unwrap();
        assert_eq!(ask.question, "推不推？");
        assert_eq!(ask.options, vec!["推", "不推"]);
        let many = AskRequest {
            question: "q".into(),
            options: (0..9).map(|i| i.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(normalize(&many).unwrap().options.len(), OPTIONS_MAX);
        assert!(normalize(&AskRequest::default()).is_err());
    }

    fn ask(question: &str) -> Ask {
        normalize(&AskRequest {
            question: question.into(),
            options: vec!["是".into(), "否".into()],
            ..Default::default()
        })
        .unwrap()
    }

    async fn later_turn(st: &Shared, thread: &str) -> String {
        let later = SessionId::new().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, thread_id, runtime_kind, status, created_at)
             VALUES (?1, 'w1', ?2, 'claude', 'done', ?3)",
        )
        .bind(&later)
        .bind(thread)
        .bind(Utc::now().to_rfc3339())
        .execute(st.db.pool())
        .await
        .unwrap();
        later
    }

    async fn open_cards(st: &Shared) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT id FROM inbox WHERE kind = 'question' AND archived_at IS NULL ORDER BY id",
        )
        .fetch_all(st.db.pool())
        .await
        .unwrap()
    }

    fn session_headers(sid: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(crate::bots::jobs::SESSION_HEADER, sid.parse().unwrap());
        h
    }

    #[tokio::test]
    async fn cards_stay_open_across_new_cards_and_later_turns() {
        let (_dir, st) = state().await;
        let sid = SessionId::new().to_string();
        seed_session(&st, &sid).await;
        let a = ok(post(&st, &sid, &ask("先推还是先装？"), "两批一起装").await);
        let b = ok(post(&st, &sid, &ask("装吗？"), "").await);
        assert_ne!(a, b);
        let card: Option<String> = sqlx::query_scalar("SELECT ask FROM inbox WHERE id = ?1")
            .bind(&a)
            .fetch_one(st.db.pool())
            .await
            .unwrap();
        let stored: Ask = serde_json::from_str(&card.unwrap()).unwrap();
        assert_eq!(stored.options, vec!["是", "否"]);

        let later = later_turn(&st, &sid).await;
        let c = ok(post(&st, &later, &ask("换哪台机器？"), "").await);
        assert_eq!(open_cards(&st).await, vec![a.clone(), b.clone(), c.clone()]);
        assert!(crate::inbox::has_open_ask(&st, &sid).await);
        assert!(crate::inbox::has_open_ask(&st, &later).await);
        assert!(!crate::inbox::has_open_ask(&st, "nope").await);
    }

    #[tokio::test]
    async fn answering_dismissing_and_withdrawing_close_only_that_card() {
        let (_dir, st) = state().await;
        let sid = SessionId::new().to_string();
        seed_session(&st, &sid).await;
        let a = ok(post(&st, &sid, &ask("一"), "").await);
        let b = ok(post(&st, &sid, &ask("二"), "").await);
        let c = ok(post(&st, &sid, &ask("三"), "").await);

        crate::inbox::resolve_ask(&st, &a).await;
        assert_eq!(open_cards(&st).await, vec![b.clone(), c.clone()]);

        crate::inbox::resolve_one(State(st.clone()), Path(b.clone())).await;
        assert_eq!(open_cards(&st).await, vec![c.clone()]);
        assert!(crate::inbox::has_open_ask(&st, &sid).await);

        let later = later_turn(&st, &sid).await;
        let _ = ok(close_ask(State(st.clone()), session_headers(&later), Path(c.clone())).await);
        assert!(open_cards(&st).await.is_empty());
        let stored: Option<String> = sqlx::query_scalar("SELECT ask FROM sessions WHERE id = ?1")
            .bind(&sid)
            .fetch_one(st.db.pool())
            .await
            .unwrap();
        assert!(
            stored.is_none(),
            "the session stops asking once its last card closes"
        );
    }

    #[tokio::test]
    async fn only_the_same_conversation_can_withdraw_a_card() {
        let (_dir, st) = state().await;
        let sid = SessionId::new().to_string();
        seed_session(&st, &sid).await;
        let a = ok(post(&st, &sid, &ask("推不推？"), "").await);
        let stranger = SessionId::new().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES (?1, 'w1', 'claude', 'done', '0')",
        )
        .bind(&stranger)
        .execute(st.db.pool())
        .await
        .unwrap();
        assert!(
            close_ask(
                State(st.clone()),
                session_headers(&stranger),
                Path(a.clone())
            )
            .await
            .is_err()
        );
        assert!(
            close_ask(State(st.clone()), HeaderMap::new(), Path(a.clone()))
                .await
                .is_err()
        );
        assert_eq!(open_cards(&st).await, vec![a]);
    }

    #[tokio::test]
    async fn a_replacing_card_closes_the_one_it_replaces() {
        let (_dir, st) = state().await;
        let sid = SessionId::new().to_string();
        seed_session(&st, &sid).await;
        let keep = ok(post(&st, &sid, &ask("另一件事"), "").await);
        let old = ok(post(&st, &sid, &ask("推不推？"), "").await);
        let out = ok(create_ask(
            State(st.clone()),
            session_headers(&sid),
            Json(AskRequest {
                question: "推到哪个分支？".into(),
                options: vec!["main".into(), "dev".into()],
                replaces: Some(old.clone()),
                ..Default::default()
            }),
        )
        .await)
        .0;
        let new = out["id"].as_str().unwrap().to_owned();
        assert_eq!(out["replaced"], old.as_str());
        assert_eq!(open_cards(&st).await, vec![keep, new]);
        assert!(
            create_ask(
                State(st.clone()),
                session_headers(&sid),
                Json(AskRequest {
                    question: "q".into(),
                    replaces: Some("missing".into()),
                    ..Default::default()
                }),
            )
            .await
            .is_err()
        );
    }

    async fn event(st: &Shared, sid: &str, seq: i64, payload: &str) {
        sqlx::query("INSERT INTO events (session_id, seq, ts, payload) VALUES (?1, ?2, '0', ?3)")
            .bind(sid)
            .bind(seq)
            .bind(payload)
            .execute(st.db.pool())
            .await
            .unwrap();
    }

    async fn anchor(st: &Shared, card: &str) -> (Option<i64>, Option<String>) {
        sqlx::query_as("SELECT anchor_seq, anchor_tool FROM inbox WHERE id = ?1")
            .bind(card)
            .fetch_one(st.db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_card_remembers_the_call_that_asked_it() {
        let (_dir, st) = state().await;
        let sid = SessionId::new().to_string();
        seed_session(&st, &sid).await;
        event(&st, &sid, 1, r#"{"type":"user_message","text":"hi"}"#).await;
        event(
            &st,
            &sid,
            4,
            r#"{"type":"tool_use","id":"toolu_a","name":"mcp__blazar-fleet__ask_user","input":{"question":"推不推？","options":["推","不推"]}}"#,
        )
        .await;
        event(
            &st,
            &sid,
            7,
            r#"{"type":"tool_use","id":"toolu_b","name":"mcp__blazar-fleet__report","input":{"level":"decision","text":"装不装？"}}"#,
        )
        .await;
        event(&st, &sid, 9, r#"{"type":"assistant_message","text":"ok"}"#).await;
        let a = ok(post(&st, &sid, &ask("推不推？"), "").await);
        let b = ok(post(&st, &sid, &ask("装不装？"), "").await);
        let c = ok(post(&st, &sid, &ask("没在日志里的问题"), "").await);
        assert_eq!(anchor(&st, &a).await, (Some(4), Some("toolu_a".into())));
        assert_eq!(anchor(&st, &b).await, (Some(7), Some("toolu_b".into())));
        assert_eq!(anchor(&st, &c).await, (Some(9), None));

        let again = ok(post(&st, &sid, &ask("推不推？"), "").await);
        assert_eq!(
            anchor(&st, &again).await,
            (Some(9), None),
            "a call already pointed at by another card isn't reused"
        );

        let listed = crate::inbox::list(State(st.clone()), Query(Default::default())).await;
        let body = axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let item = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|it| it["id"] == a.as_str())
            .unwrap();
        assert_eq!(item["anchor_seq"], 4);
        assert_eq!(item["anchor_tool"], "toolu_a");
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

    #[tokio::test]
    async fn an_answer_to_a_busy_run_that_cannot_take_it_waits_in_the_queue() {
        let (_dir, st) = state().await;
        let now = Utc::now().to_rfc3339();
        let node = ok(super::super::workspaces::ensure_node(&st, "local", &now).await);
        let ws = WorkspaceId::new();
        sqlx::query(
            "INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES (?1, ?2, 'w', '/tmp/w', ?3)",
        )
        .bind(ws.to_string())
        .bind(&node)
        .bind(&now)
        .execute(st.db.pool())
        .await
        .unwrap();
        let sid = SessionId::new();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, runtime_kind, status, created_at) VALUES (?1, ?2, 'claude', 'running', ?3)",
        )
        .bind(sid.to_string())
        .bind(ws.to_string())
        .bind(&now)
        .execute(st.db.pool())
        .await
        .unwrap();
        st.running.write().await.insert(
            sid,
            crate::state::RunningSession {
                workspace_id: ws,
                task: tokio::spawn(std::future::pending()),
                killer: None,
                live: None,
            },
        );
        let ask = normalize(&AskRequest {
            question: "推不推？".into(),
            options: vec!["推".into(), "不推".into()],
            ..Default::default()
        })
        .unwrap();
        let card = ok(post(&st, &sid.to_string(), &ask, "").await);

        let out = ok(answer_ask(
            State(st.clone()),
            Path(card.clone()),
            Json(AnswerRequest { text: "推".into() }),
        )
        .await)
        .0;
        assert_eq!(out["delivered"], true);
        assert_eq!(out["queued"], true);
        let queued: String = sqlx::query_scalar(
            "SELECT request FROM queued_messages WHERE workspace_id = ?1 AND thread_id = ?2",
        )
        .bind(ws.to_string())
        .bind(sid.to_string())
        .fetch_one(st.db.pool())
        .await
        .unwrap();
        let queued: serde_json::Value = serde_json::from_str(&queued).unwrap();
        assert_eq!(queued["text"], "推");
        assert_eq!(queued["resume_session"], sid.to_string());
        assert!(!crate::inbox::has_open_ask(&st, &sid.to_string()).await);
        for (_, run) in st.running.write().await.drain() {
            run.task.abort();
        }
    }
}
