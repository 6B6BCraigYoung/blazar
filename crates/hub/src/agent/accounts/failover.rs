use axum::Json;
use axum::extract::{Path, State};
use chrono::Utc;
use serde_json::{Value, json};

use crate::api::Shared;
use crate::state::ServerEvent;

use super::get;
use super::probe::check;
use super::quota::refresh_quota;
use super::selection::{resolve, windows_of};

async fn account_of_session(st: &Shared, sid: blazar_core_types::SessionId) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT account_id FROM sessions WHERE id = ?1")
        .bind(sid.to_string())
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .flatten()
}

pub(super) async fn upsert_window(
    st: &Shared,
    account: &str,
    name: &str,
    util: f64,
    resets_at: Option<String>,
) {
    let _ = sqlx::query(
        "INSERT INTO rate_limit_snapshots (account_id, window_name, utilization, resets_at, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (account_id, window_name) DO UPDATE SET
           utilization = excluded.utilization, resets_at = excluded.resets_at,
           observed_at = excluded.observed_at",
    )
    .bind(account)
    .bind(name)
    .bind(util.clamp(0.0, 1.0))
    .bind(resets_at)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
}

pub async fn note_rate_limit(
    st: &Shared,
    sid: blazar_core_types::SessionId,
    rl: &blazar_core_types::RateLimit,
) {
    let Some(account) = account_of_session(st, sid).await else {
        return;
    };
    for w in &rl.windows {
        upsert_window(
            st,
            &account,
            &w.name,
            w.utilization,
            w.resets_at.map(|t| t.to_rfc3339()),
        )
        .await;
    }
    if rl.allowed {
        let _ = sqlx::query(
            "DELETE FROM rate_limit_snapshots WHERE account_id = ?1 AND window_name = 'blocked'",
        )
        .bind(&account)
        .execute(st.db.pool())
        .await;
    } else if !rl.windows.is_empty() && rl.windows.iter().all(|w| w.utilization < 1.0) {
        upsert_window(st, &account, "blocked", 1.0, None).await;
    }
    st.emit(ServerEvent::AccountsChanged);
}

async fn session_model(st: &Shared, sid: blazar_core_types::SessionId) -> Option<String> {
    let started: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_id = ?1
           AND payload LIKE '{\"type\":\"session_started\"%' ORDER BY seq DESC LIMIT 1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let request: Option<String> =
        sqlx::query_scalar::<_, Option<String>>("SELECT request FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .flatten();
    [started, request]
        .into_iter()
        .flatten()
        .filter_map(|p| serde_json::from_str::<Value>(&p).ok())
        .find_map(|v| v["model"].as_str().map(str::to_owned))
        .filter(|m| !m.trim().is_empty())
}

async fn note_model_block(
    st: &Shared,
    sid: blazar_core_types::SessionId,
    account: &str,
    why: &str,
) {
    let Some(model) = session_model(st, sid).await else {
        return;
    };
    let model = model
        .split('[')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let _ = sqlx::query(
        "INSERT INTO account_model_blocks (account_id, model, reason, observed_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (account_id, model) DO UPDATE SET reason = excluded.reason, observed_at = excluded.observed_at",
    )
    .bind(account)
    .bind(&model)
    .bind(why.chars().take(300).collect::<String>())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    st.emit(ServerEvent::AccountsChanged);
}

pub async fn on_run_finished(st: Shared, sid: blazar_core_types::SessionId, status: &'static str) {
    if let Ok(Some((Some(acc), rt))) = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT account_id, runtime_kind FROM sessions WHERE id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
        && rt == "codex"
        && let Some(a) = get(&st, &acc).await
    {
        let st = st.clone();
        tokio::spawn(async move {
            if let Err(e) = refresh_quota(&st, &a).await {
                tracing::info!(target: "blazar::accounts", "更新 Codex 额度失败：{e}");
            }
        });
    }
    if status != "failed" {
        return;
    }
    type Row = (
        String,
        Option<String>,
        i64,
        Option<String>,
        String,
        String,
        String,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT workspace_id, account_id, account_auto, request, COALESCE(thread_id, id), runtime_kind,
                COALESCE(node, 'local')
         FROM sessions WHERE id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let Some((ws, Some(account), 1, Some(raw), thread, runtime, node)) = row else {
        return;
    };
    let worked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE session_id = ?1 AND (
           payload LIKE '{\"type\":\"assistant_message\"%' OR payload LIKE '{\"type\":\"tool_use\"%'
           OR payload LIKE '{\"type\":\"thinking\"%')",
    )
    .bind(sid.to_string())
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(1);
    let midway = worked > 0;
    let why: String = sqlx::query_scalar::<_, String>(
        "SELECT payload FROM events WHERE session_id = ?1 AND (
           payload LIKE '{\"type\":\"finished\"%' OR payload LIKE '{\"type\":\"error\"%')
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| v["message"].as_str().map(str::to_owned))
    .unwrap_or_default();
    let class = blazar_core_types::FailureClass::classify(&why);
    if !model_only_limit(&why) && !class.should_switch_account() {
        return;
    }
    let Ok(req) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    let mut exclude: Vec<String> =
        serde_json::from_value(req["exclude_accounts"].clone()).unwrap_or_default();
    exclude.push(account.clone());
    let model = session_model(&st, sid).await;
    match resolve(
        &st,
        &runtime,
        &node,
        Some("auto"),
        None,
        model.as_deref(),
        &exclude,
    )
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return,
        Err(e) => {
            tracing::info!(target: "blazar::accounts", "{sid} 不自动换号重发：{e}");
            return;
        }
    }

    if !midway {
        let _ = sqlx::query("UPDATE sessions SET provider_session_id = NULL WHERE id = ?1")
            .bind(sid.to_string())
            .execute(st.db.pool())
            .await;
    }
    let req = retry_request(req, midway, &exclude, &thread);
    let Ok(parsed) = serde_json::from_value::<crate::api::PromptRequest>(req) else {
        return;
    };
    let from = get(&st, &account).await.map(|a| a.label).unwrap_or(account);
    match send_again(&st, &ws, parsed).await {
        Ok(Json(v)) if v["admitted"] != json!(false) => {
            let to = match v["account"].as_str() {
                Some(id) => get(&st, id).await.map(|a| a.label).unwrap_or_default(),
                None => String::new(),
            };
            let ws_name = sqlx::query_as::<_, (String, String)>(
                "SELECT w.name, n.name FROM workspaces w JOIN nodes n ON n.id = w.node_id
                 WHERE w.id = ?1",
            )
            .bind(&ws)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .map(|(name, node)| blazar_core_types::api::workspace_label(&name, &node))
            .unwrap_or_default();
            crate::inbox::push(
                &st,
                crate::inbox::Item {
                    kind: "rate_limit",
                    title: if midway {
                        format!("{ws_name} · 「{from}」中途停了，已换到「{to}」接着做")
                    } else {
                        format!("{ws_name} · 已从「{from}」换到「{to}」重发")
                    },
                    body: why.chars().take(200).collect(),
                    workspace_id: Some(ws),
                    thread_id: Some(thread),
                    session_id: v["session_id"].as_str().map(str::to_owned),
                    ref_id: None,
                },
            )
            .await;
        }
        Ok(Json(v)) => tracing::info!(target: "blazar::accounts", "换号重发没被接受：{v}"),
        Err(e) => tracing::warn!(target: "blazar::accounts", "换号重发失败：{}", e.message()),
    }
}

#[must_use]
pub fn retry_request(mut req: Value, midway: bool, exclude: &[String], thread: &str) -> Value {
    if midway {
        req["text"] = json!(CONTINUE_TEXT);
        req["images"] = json!([]);
        req["context_file"] = Value::Null;
        req["resume"] = json!(true);
        req["resume_at_last"] = json!(false);
    } else if req["resume"].as_bool() == Some(true) {
        req["resume_at_last"] = json!(true);
    }
    req["exclude_accounts"] = json!(exclude);
    req["account"] = json!("auto");
    req["retry_thread"] = json!(thread);
    req["resume_session"] = json!(thread);
    req["wait_secs"] = json!(0);
    req
}

pub const CONTINUE_TEXT: &str = "(The previous account hit its usage limit, so this continues on another account.) Pick up where you left off and finish the remaining work.";

fn send_again<'a>(
    st: &'a Shared,
    ws: &'a str,
    req: crate::api::PromptRequest,
) -> futures::future::BoxFuture<'a, Result<Json<Value>, crate::api::ApiError>> {
    Box::pin(crate::api::prompt(
        State(st.clone()),
        Path(ws.to_owned()),
        Json(req),
    ))
}

#[must_use]
pub fn model_only_limit(message: &str) -> bool {
    let t = message.to_ascii_lowercase();
    [
        "switch to another model",
        "requires usage credits",
        "requires extra usage",
    ]
    .iter()
    .any(|p| t.contains(p))
}

pub async fn note_failure(st: &Shared, sid: blazar_core_types::SessionId, message: &str) {
    use blazar_core_types::FailureClass;
    if model_only_limit(message) {
        if let Some(account) = account_of_session(st, sid).await {
            note_model_block(st, sid, &account, message).await;
        }
        return;
    }
    let class = FailureClass::classify(message);
    if !class.should_switch_account() {
        return;
    }
    let Some(account) = account_of_session(st, sid).await else {
        return;
    };
    if class == FailureClass::QuotaLimit {
        let now = Utc::now();
        let full = windows_of(st).await.get(&account).is_some_and(|ws| {
            ws.iter()
                .any(|w| w.live(now) && w.resets_at.is_some() && w.utilization >= 1.0)
        });
        if !full {
            upsert_window(st, &account, "blocked", 1.0, None).await;
        }
        st.emit(ServerEvent::AccountsChanged);
        return;
    }
    let Some(a) = get(st, &account).await else {
        return;
    };
    if a.kind == "token" {
        let _ =
            sqlx::query("UPDATE accounts SET status = 'logged_out', checked_at = ?2 WHERE id = ?1")
                .bind(&a.id)
                .bind(Utc::now().to_rfc3339())
                .execute(st.db.pool())
                .await;
        st.emit(ServerEvent::AccountsChanged);
    } else {
        check(st, &a).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_midway_resumes_the_same_conversation() {
        let orig = json!({ "text": "重构登录模块", "images": [{ "media_type": "image/png", "data": "AA==" }],
                           "resume": false, "model": "claude-fable-5-1", "context_file": "src/a.rs" });
        let r = retry_request(orig.clone(), true, &["a".into()], "t1");
        assert_eq!(r["text"], CONTINUE_TEXT);
        assert_eq!(r["images"], json!([]), "图片已经在上下文里了，不再发");
        assert_eq!(r["resume"], true, "接着同一个会话，上下文不变");
        assert_eq!(r["resume_at_last"], false, "从中断处接着，不回退到上一轮");
        assert_eq!(r["resume_session"], "t1");
        assert_eq!(r["model"], "claude-fable-5-1", "模型不变");
        assert_eq!(r["account"], "auto");
        assert_eq!(r["exclude_accounts"], json!(["a"]));

        let r = retry_request(orig, false, &["a".into(), "b".into()], "t1");
        assert_eq!(r["text"], "重构登录模块", "没开始干就原样重发");
        assert_eq!(r["images"].as_array().map(Vec::len), Some(1));
        assert_eq!(r["retry_thread"], "t1");
        assert_eq!(r["exclude_accounts"], json!(["a", "b"]));
    }

    #[test]
    fn model_specific_refusals_do_not_block_the_account() {
        assert!(model_only_limit(
            "Fable 5.1 requires usage credits. Switch to another model, or manage usage credits"
        ));
        assert!(!model_only_limit(
            "This request would exceed your account's rate limit. Please try again later."
        ));
    }
}
