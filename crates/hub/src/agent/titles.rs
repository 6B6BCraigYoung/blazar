use std::process::Stdio;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use blazar_core_types::{SessionId, WorkspaceId};
use serde::Deserialize;

use crate::api::{ApiResult, Shared};
use crate::state::ServerEvent;

const MAX_CHARS: usize = 40;

fn clean(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .trim_matches(|c: char| "\"'“”「」『』《》。．.".contains(c))
        .trim();
    if line.is_empty() || line.starts_with('/') {
        return None;
    }
    Some(line.chars().take(MAX_CHARS).collect())
}

async fn opening(st: &Shared, sid: SessionId) -> Option<(String, String)> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_id = ?1 ORDER BY seq LIMIT 80",
    )
    .bind(sid.to_string())
    .fetch_all(st.db.pool())
    .await
    .ok()?;
    let mut user = None;
    let mut assistant = None;
    for p in rows {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&p) else {
            continue;
        };

        let text = v
            .pointer("/text")
            .and_then(|t| t.as_str())
            .map(|t| t.chars().take(1200).collect::<String>());
        match v.pointer("/type").and_then(|t| t.as_str()) {
            Some("user_message") if user.is_none() => user = text,
            Some("assistant_message" | "finished") if assistant.is_none() => assistant = text,
            _ => {}
        }
        if user.is_some() && assistant.is_some() {
            break;
        }
    }
    let user = user.filter(|u| !u.trim().is_empty() && !u.trim_start().starts_with('/'))?;
    Some((user, assistant.unwrap_or_default()))
}

async fn summarize(program: &str, user: &str, assistant: &str) -> Option<String> {
    let prompt = format!(
        "用对话本身的语言（用户用中文就用中文），不超过 12 个字或 6 个英文单词，概括下面这段对话在做什么，\
         作为标签页标题。只输出标题本身，不要引号、句号或解释。\n\n用户：{user}\n\n助手：{assistant}"
    );
    clean(&ask_haiku(program, &prompt, 45).await?)
}

pub(crate) async fn ask_haiku(program: &str, prompt: &str, secs: u64) -> Option<String> {
    let dir = std::env::temp_dir();
    let mut child = tokio::process::Command::new(program)
        .args([
            "-p",
            "--model",
            "haiku",
            "--output-format",
            "text",
            "--tools",
            "",
        ])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    let mut stdin = child.stdin.take()?;
    let text = prompt.to_owned();
    tokio::spawn(async move {
        let _ = tokio::io::AsyncWriteExt::write_all(&mut stdin, text.as_bytes()).await;
        let _ = tokio::io::AsyncWriteExt::shutdown(&mut stdin).await;
    });
    let out = tokio::time::timeout(Duration::from_secs(secs), child.wait_with_output()).await;
    let out = match out {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            tracing::warn!(target: "blazar::titles", "{program} 没跑起来：{e}");
            return None;
        }
        Err(_) => {
            tracing::warn!(target: "blazar::titles", "{program} {secs}s 内没答完，放弃");
            return None;
        }
    };
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if text.is_empty() {
        let err: String = String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(300)
            .collect();
        tracing::warn!(target: "blazar::titles", "{program} 没有输出（{}）：{}", out.status, err.trim());
        return None;
    }
    Some(text)
}

async fn root_of(st: &Shared, sid: SessionId) -> Option<SessionId> {
    let root: String =
        sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()??;
    root.parse().ok().map(SessionId)
}

pub async fn maybe_title(st: Shared, sid: SessionId, ws: WorkspaceId) {
    let Some(sid) = root_of(&st, sid).await else {
        return;
    };

    let existing: Option<Option<String>> =
        sqlx::query_scalar::<_, Option<String>>("SELECT title FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten();
    match existing {
        None => return,
        Some(Some(ref t)) if !t.is_empty() => return,
        _ => {}
    }
    let Some((user, assistant)) = opening(&st, sid).await else {
        return;
    };
    let program = crate::api::local_program(&st, "claude").await;
    let Some(title) = summarize(&program, &user, &assistant).await else {
        tracing::warn!(target: "blazar::titles", "会话 {sid} 没总结出标题（{program} 不可用或没有输出）");
        return;
    };
    set_title(&st, sid, ws, &title).await;
}

async fn set_title(st: &Shared, sid: SessionId, ws: WorkspaceId, title: &str) {
    let _ = sqlx::query("UPDATE sessions SET title = ?2 WHERE id = ?1")
        .bind(sid.to_string())
        .bind(title)
        .execute(st.db.pool())
        .await;
    st.emit(ServerEvent::SessionTitled {
        workspace_id: ws,
        session_id: sid,
        title: title.to_owned(),
    });
}

#[derive(Deserialize)]
pub struct TitleBody {
    pub title: String,
}

pub async fn rename(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<TitleBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let sid = SessionId(id.parse()?);
    let sid = root_of(&st, sid).await.unwrap_or(sid);
    let Some(title) = clean(&b.title) else {
        return Ok(Json(
            serde_json::json!({ "ok": false, "reason": "标题不能为空" }),
        ));
    };
    let ws: Option<String> = sqlx::query_scalar("SELECT workspace_id FROM sessions WHERE id = ?1")
        .bind(sid.to_string())
        .fetch_optional(st.db.pool())
        .await?;
    let Some(ws) = ws.and_then(|w| w.parse().ok()).map(WorkspaceId) else {
        return Ok(Json(
            serde_json::json!({ "ok": false, "reason": "没有这个会话" }),
        ));
    };
    set_title(&st, sid, ws, &title).await;
    Ok(Json(serde_json::json!({ "ok": true, "title": title })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_noise_and_limits_length() {
        assert_eq!(
            clean("  “修复登录超时”。\n多余的解释").as_deref(),
            Some("修复登录超时")
        );
        assert_eq!(clean("\n\n").as_deref(), None);
        assert_eq!(clean("/context").as_deref(), None);
        let long = "很".repeat(100);
        assert_eq!(clean(&long).unwrap().chars().count(), MAX_CHARS);
    }
}
