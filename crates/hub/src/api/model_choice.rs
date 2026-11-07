use std::collections::BTreeMap;

use super::*;

pub(crate) const GLOBAL: &str = "global";

/// A model and reasoning effort. `None` leaves the field to the layer below; an empty string
/// is an explicit choice of the runtime's own default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Choice {
    fn is_empty(&self) -> bool {
        self.model.is_none() && self.effort.is_none()
    }

    #[must_use]
    pub fn or(self, lower: Choice) -> Choice {
        Choice {
            model: self.model.or(lower.model),
            effort: self.effort.or(lower.effort),
        }
    }
}

pub(crate) fn ws_scope(workspace: &str) -> String {
    format!("ws:{workspace}")
}

pub(crate) fn thread_scope(thread: &str) -> String {
    format!("thread:{thread}")
}

pub(crate) fn valid_model(m: &str) -> bool {
    m.len() <= 80
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:[]/".contains(c))
}

fn valid_effort(e: &str) -> bool {
    e.is_empty() || blazar_runtime::cli_runtime::EFFORTS.contains(&e)
}

fn valid_runtime(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 40
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn clean(c: &Choice) -> Result<Choice, String> {
    let model = c.model.as_deref().map(str::trim);
    let effort = c.effort.as_deref().map(str::trim);
    if model.is_some_and(|m| !valid_model(m)) {
        return Err("That model name isn't valid".into());
    }
    if effort.is_some_and(|e| !valid_effort(e)) {
        return Err("Unknown reasoning effort".into());
    }
    Ok(Choice {
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
    })
}

async fn layer(st: &AppState, scope: &str) -> BTreeMap<String, Choice> {
    let rows: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT runtime, model, effort FROM model_choices WHERE scope = ?1")
            .bind(scope)
            .fetch_all(st.db.pool())
            .await
            .unwrap_or_default();
    rows.into_iter()
        .map(|(rt, model, effort)| (rt, Choice { model, effort }))
        .collect()
}

pub(crate) async fn put(
    st: &AppState,
    scope: &str,
    runtime: &str,
    c: &Choice,
) -> Result<(), String> {
    if !valid_runtime(runtime) {
        return Err("Unknown runtime".into());
    }
    let c = clean(c)?;
    let r = if c.is_empty() {
        sqlx::query("DELETE FROM model_choices WHERE scope = ?1 AND runtime = ?2")
            .bind(scope)
            .bind(runtime)
            .execute(st.db.pool())
            .await
    } else {
        sqlx::query(
            "INSERT INTO model_choices (scope, runtime, model, effort, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (scope, runtime) DO UPDATE SET model = excluded.model, effort = excluded.effort,
               updated_at = excluded.updated_at",
        )
        .bind(scope)
        .bind(runtime)
        .bind(&c.model)
        .bind(&c.effort)
        .bind(Utc::now().to_rfc3339())
        .execute(st.db.pool())
        .await
    };
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// The conversation's own layer: what was picked for it, else what its latest turn on each
/// runtime asked for.
pub(crate) async fn thread_layer(st: &AppState, thread: &str) -> BTreeMap<String, Choice> {
    let mut out = layer(st, &thread_scope(thread)).await;
    let turns: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT runtime_kind, request FROM sessions WHERE COALESCE(thread_id, id) = ?1
         ORDER BY created_at DESC LIMIT 50",
    )
    .bind(thread)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    for (rt, request) in turns {
        if out.contains_key(&rt) {
            continue;
        }
        let Some(v) = request.and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
        else {
            continue;
        };
        let pick = |k: &str| v[k].as_str().map(str::to_owned).filter(|s| !s.is_empty());
        let c = Choice {
            model: pick("model"),
            effort: pick("effort"),
        };
        if !c.is_empty() {
            out.insert(rt, c);
        }
    }
    out
}

/// What a turn runs with: the request itself, then the conversation, then the agent profile,
/// then the workspace default, then the global default.
pub(crate) async fn for_turn(
    st: &AppState,
    workspace: &str,
    thread: Option<&str>,
    runtime: &str,
    asked: Choice,
    profile: Choice,
) -> Choice {
    let mut c = asked;
    if let Some(t) = thread {
        c = c.or(thread_layer(st, t)
            .await
            .remove(runtime)
            .unwrap_or_default());
    }
    c = c.or(profile);
    c = c.or(layer(st, &ws_scope(workspace))
        .await
        .remove(runtime)
        .unwrap_or_default());
    c.or(layer(st, GLOBAL).await.remove(runtime).unwrap_or_default())
}

/// Each turn leaves what it ran with on its conversation, so the next turn carries on with it.
pub(crate) async fn remember_turn(st: &AppState, thread: &str, runtime: &str, c: &Choice) {
    if c.is_empty() {
        return;
    }
    if let Err(e) = put(st, &thread_scope(thread), runtime, c).await {
        tracing::warn!(target: "blazar::models", "没能记下这个对话用的模型: {e}");
    }
}

pub(crate) async fn forget_workspace(st: &AppState, workspace: &str) {
    let _ = sqlx::query(
        "DELETE FROM model_choices WHERE scope = ?1 OR scope IN
           (SELECT 'thread:' || COALESCE(thread_id, id) FROM sessions WHERE workspace_id = ?2)",
    )
    .bind(ws_scope(workspace))
    .bind(workspace)
    .execute(st.db.pool())
    .await;
}

#[derive(Debug, Default, Deserialize)]
pub struct ChoicesQuery {
    #[serde(default)]
    pub thread: Option<String>,
}

pub async fn get_model_choices(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<ChoicesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let thread = match q.thread.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => thread_layer(&st, t).await,
        None => BTreeMap::new(),
    };
    Ok(Json(serde_json::json!({
        "global": layer(&st, GLOBAL).await,
        "workspace": layer(&st, &ws_scope(&id)).await,
        "thread": thread,
    })))
}

#[derive(Debug, Deserialize)]
pub struct PutChoice {
    pub scope: String,
    #[serde(default)]
    pub thread: Option<String>,
    pub runtime: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

pub async fn put_model_choice(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<PutChoice>,
) -> ApiResult<Json<serde_json::Value>> {
    let scope = match b.scope.as_str() {
        "global" => GLOBAL.to_owned(),
        "workspace" => ws_scope(&id),
        "thread" => {
            let thread = b
                .thread
                .as_deref()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| ApiError::bad_request("Which conversation?"))?;
            let ours: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sessions WHERE workspace_id = ?1 AND COALESCE(thread_id, id) = ?2",
            )
            .bind(&id)
            .bind(thread)
            .fetch_one(st.db.pool())
            .await?;
            if ours == 0 {
                return Err(ApiError::bad_request(
                    "That conversation isn't in this workspace",
                ));
            }
            thread_scope(thread)
        }
        _ => {
            return Err(ApiError::bad_request(
                "scope must be thread, workspace or global",
            ));
        }
    };
    let c = Choice {
        model: b.model,
        effort: b.effort,
    };
    put(&st, &scope, &b.runtime, &c)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_model_defaults(State(st): State<Shared>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "global": layer(&st, GLOBAL).await }))
}

#[derive(Debug, Deserialize)]
pub struct PutDefault {
    pub runtime: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

pub async fn put_model_default(
    State(st): State<Shared>,
    Json(b): Json<PutDefault>,
) -> ApiResult<Json<serde_json::Value>> {
    let c = Choice {
        model: b.model,
        effort: b.effort,
    };
    put(&st, GLOBAL, &b.runtime, &c)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn state() -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        let st = AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().join("mesh")),
            crate::services::Services::Isolated,
        );
        (dir, st)
    }

    fn c(model: Option<&str>, effort: Option<&str>) -> Choice {
        Choice {
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        }
    }

    async fn seed_turn(st: &Shared, sid: &str, thread: &str, runtime: &str, request: &str) {
        let now = Utc::now().to_rfc3339();
        let node = super::super::workspaces::ensure_node(st, "local", &now)
            .await
            .unwrap_or_else(|e| panic!("{}", e.message()));
        sqlx::query(
            "INSERT OR IGNORE INTO workspaces (id, node_id, name, path, created_at) VALUES ('w1', ?1, 'w', '/tmp/w', ?2)",
        )
        .bind(&node)
        .bind(&now)
        .execute(st.db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, workspace_id, thread_id, runtime_kind, status, created_at, request)
             VALUES (?1, 'w1', ?2, ?3, 'done', ?4, ?5)",
        )
        .bind(sid)
        .bind(thread)
        .bind(runtime)
        .bind(now)
        .bind(request)
        .execute(st.db.pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn each_layer_fills_only_what_the_one_above_leaves_open() {
        let (_dir, st) = state().await;
        let none = Choice::default();
        assert_eq!(
            for_turn(&st, "w1", None, "claude", none.clone(), none.clone()).await,
            none
        );

        put(&st, GLOBAL, "claude", &c(Some("sonnet"), Some("medium")))
            .await
            .unwrap();
        put(&st, GLOBAL, "codex", &c(Some("gpt-x"), None))
            .await
            .unwrap();
        assert_eq!(
            for_turn(&st, "w1", None, "claude", none.clone(), none.clone()).await,
            c(Some("sonnet"), Some("medium"))
        );

        put(&st, &ws_scope("w1"), "claude", &c(Some("opus"), None))
            .await
            .unwrap();
        assert_eq!(
            for_turn(&st, "w1", None, "claude", none.clone(), none.clone()).await,
            c(Some("opus"), Some("medium")),
            "the workspace sets the model, the effort still comes from the global default"
        );
        assert_eq!(
            for_turn(&st, "w2", None, "claude", none.clone(), none.clone()).await,
            c(Some("sonnet"), Some("medium")),
            "another workspace keeps the global default"
        );
        assert_eq!(
            for_turn(&st, "w1", None, "codex", none.clone(), none.clone()).await,
            c(Some("gpt-x"), None),
            "each runtime keeps its own choices"
        );

        put(
            &st,
            &thread_scope("t1"),
            "claude",
            &c(Some("haiku"), Some("high")),
        )
        .await
        .unwrap();
        assert_eq!(
            for_turn(&st, "w1", Some("t1"), "claude", none.clone(), none.clone()).await,
            c(Some("haiku"), Some("high"))
        );
        assert_eq!(
            for_turn(
                &st,
                "w1",
                Some("t1"),
                "claude",
                none.clone(),
                c(Some("profile-model"), None)
            )
            .await,
            c(Some("haiku"), Some("high")),
            "the conversation outranks the agent profile"
        );
        assert_eq!(
            for_turn(
                &st,
                "w1",
                None,
                "claude",
                none.clone(),
                c(Some("profile-model"), None)
            )
            .await,
            c(Some("profile-model"), Some("medium")),
            "the agent profile outranks the workspace default"
        );
        assert_eq!(
            for_turn(
                &st,
                "w1",
                Some("t1"),
                "claude",
                c(Some("asked"), None),
                none.clone()
            )
            .await,
            c(Some("asked"), Some("high")),
            "the request itself wins"
        );
        assert_eq!(
            for_turn(&st, "w1", None, "claude", c(Some(""), None), none.clone()).await,
            c(Some(""), Some("medium")),
            "an explicit runtime default is not filled from below"
        );
    }

    #[tokio::test]
    async fn a_conversation_without_a_pick_carries_on_with_its_latest_turn() {
        let (_dir, st) = state().await;
        seed_turn(
            &st,
            "s1",
            "t1",
            "claude",
            r#"{"text":"a","model":"opus","effort":"high"}"#,
        )
        .await;
        seed_turn(&st, "s2", "t1", "codex", r#"{"text":"b","model":"gpt-x"}"#).await;
        seed_turn(&st, "s3", "t2", "claude", r#"{"text":"c"}"#).await;
        let layer = thread_layer(&st, "t1").await;
        assert_eq!(layer["claude"], c(Some("opus"), Some("high")));
        assert_eq!(layer["codex"], c(Some("gpt-x"), None));
        assert!(thread_layer(&st, "t2").await.is_empty());

        put(&st, &thread_scope("t1"), "claude", &c(Some("sonnet"), None))
            .await
            .unwrap();
        assert_eq!(
            thread_layer(&st, "t1").await["claude"],
            c(Some("sonnet"), None),
            "a pick on the conversation outranks what its old turns asked for"
        );
    }

    #[tokio::test]
    async fn choices_are_checked_and_clearing_removes_them() {
        let (_dir, st) = state().await;
        assert!(
            put(&st, GLOBAL, "claude", &c(Some("opus; rm -rf /"), None))
                .await
                .is_err()
        );
        assert!(
            put(&st, GLOBAL, "claude", &c(None, Some("extreme")))
                .await
                .is_err()
        );
        assert!(put(&st, GLOBAL, "", &c(Some("opus"), None)).await.is_err());
        put(&st, GLOBAL, "claude", &c(Some(" opus "), Some("")))
            .await
            .unwrap();
        assert_eq!(
            layer(&st, GLOBAL).await["claude"],
            c(Some("opus"), Some(""))
        );
        put(&st, GLOBAL, "claude", &Choice::default())
            .await
            .unwrap();
        assert!(layer(&st, GLOBAL).await.is_empty());
    }

    #[tokio::test]
    async fn a_conversation_pick_must_belong_to_the_workspace() {
        let (_dir, st) = state().await;
        seed_turn(&st, "s1", "t1", "claude", "{}").await;
        let put_thread = |ws: &str, thread: &str| {
            put_model_choice(
                State(st.clone()),
                Path(ws.to_owned()),
                Json(PutChoice {
                    scope: "thread".into(),
                    thread: Some(thread.into()),
                    runtime: "claude".into(),
                    model: Some("opus".into()),
                    effort: None,
                }),
            )
        };
        assert!(put_thread("w1", "t1").await.is_ok());
        assert!(put_thread("w2", "t1").await.is_err());
        assert!(put_thread("w1", "nope").await.is_err());
        let got = get_model_choices(
            State(st.clone()),
            Path("w1".into()),
            Query(ChoicesQuery {
                thread: Some("t1".into()),
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.message()))
        .0;
        assert_eq!(got["thread"]["claude"]["model"], "opus");
        let _ = put_model_default(
            State(st.clone()),
            Json(PutDefault {
                runtime: "claude".into(),
                model: Some("sonnet".into()),
                effort: None,
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("{}", e.message()));
        assert_eq!(
            get_model_defaults(State(st.clone())).await.0["global"]["claude"]["model"],
            "sonnet"
        );
        forget_workspace(&st, "w1").await;
        assert!(layer(&st, &thread_scope("t1")).await.is_empty());
    }
}
