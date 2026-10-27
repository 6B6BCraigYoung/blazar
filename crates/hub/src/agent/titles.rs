use std::process::Stdio;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use blazar_core_types::{SessionId, WorkspaceId};
use serde::Deserialize;

use crate::api::{ApiResult, Shared};
use crate::state::ServerEvent;

const MAX_CHARS: usize = 40;

const ERROR_MARKS: &[&str] = &[
    "not logged in",
    "please run /login",
    "api error",
    "invalid api key",
    "unauthorized",
    "authentication_error",
    "authentication failed",
    "rate limit",
    "usage limit",
    "未登录",
    "登录已失效",
];

#[must_use]
pub fn looks_like_error(text: &str) -> bool {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ERROR_MARKS.iter().any(|m| first.contains(m))
}

const ECHO_CUES: [&str; 8] = [
    "做什么",
    "标题",
    "摘要",
    "概括",
    "总结",
    "一句话",
    "summary",
    "title",
];

#[must_use]
pub fn strip_echo(line: &str) -> &str {
    let line = line.trim_start();
    let Some(i) = line.find(['：', ':']) else {
        return line;
    };
    let (head, rest) = line.split_at(i);
    let rest = rest.trim_start_matches(['：', ':']).trim_start();
    let head = head.trim_end().to_lowercase();
    let echoed = head.chars().count() <= 14 && ECHO_CUES.iter().any(|c| head.ends_with(c));
    if echoed && !rest.is_empty() {
        rest
    } else {
        line
    }
}

const OPENERS: [char; 7] = ['（', '(', '「', '『', '【', '《', '['];
const CLOSERS: [char; 7] = ['）', ')', '」', '』', '】', '》', ']'];
const BREAKS: &str = "。！？；，、,;!?: ：";

fn unmatched_opener(chars: &[char]) -> Option<usize> {
    let mut stack: Vec<usize> = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        if OPENERS.contains(c) {
            stack.push(i);
        } else if CLOSERS.contains(c) {
            stack.pop();
        }
    }
    stack.first().copied()
}

#[must_use]
pub fn fit(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_owned();
    }
    let room = max.saturating_sub(1);
    let floor = room / 2;
    let at_break = |i: usize| {
        i > 0
            && i <= chars.len()
            && (BREAKS.contains(chars[i - 1])
                || chars
                    .get(i)
                    .is_some_and(|c| BREAKS.contains(*c) || OPENERS.contains(c)))
    };
    let mut cut = (floor..=room).rev().find(|&i| at_break(i)).unwrap_or(room);
    if cut == room {
        let mut back = room;
        while back > floor
            && chars[back - 1].is_ascii_alphanumeric()
            && chars.get(back).is_some_and(char::is_ascii_alphanumeric)
        {
            back -= 1;
        }
        if back > floor {
            cut = back;
        }
    }
    if let Some(open) = unmatched_opener(&chars[..cut]) {
        cut = open;
    }
    let mut out: String = chars[..cut].iter().collect();
    let trimmed = out
        .trim_end_matches(|c: char| BREAKS.contains(c) || OPENERS.contains(&c))
        .len();
    out.truncate(trimmed);
    out.push('…');
    out
}

fn clean(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = strip_echo(line)
        .trim_matches(|c: char| "\"'“”「」『』《》。．.".contains(c))
        .trim();
    if line.is_empty() || line.starts_with('/') || looks_like_error(line) {
        return None;
    }
    Some(fit(line, MAX_CHARS))
}

pub struct Brain {
    pub program: String,
    pub env: Vec<(String, String)>,
}

pub(crate) async fn brain(st: &Shared, account: Option<&str>) -> Brain {
    let program = crate::api::local_program(st, "claude").await;
    let mut chosen = None;
    for requested in [account, None] {
        if chosen.is_none() {
            chosen = crate::accounts::resolve(st, "claude", "local", requested, None, None, &[])
                .await
                .ok()
                .flatten();
        }
    }
    let mut env = Vec::new();
    if let Some(c) = chosen {
        if let Some((k, v)) = c.env {
            env.push((k, v));
        }
        if let Some((k, p)) = c.token_file
            && let Ok(token) = tokio::fs::read_to_string(&p).await
            && !token.trim().is_empty()
        {
            env.push((k, token.trim().to_owned()));
        }
    }
    Brain { program, env }
}

async fn account_of(st: &Shared, sid: SessionId) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT account_id FROM sessions WHERE id = ?1")
        .bind(sid.to_string())
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .flatten()
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

async fn summarize(brain: &Brain, user: &str, assistant: &str) -> Option<String> {
    let prompt = format!(
        "给下面这段对话起一个标签页标题：用对话本身的语言（用户用中文就用中文），\
         不超过 12 个字或 6 个英文单词，说清楚在做什么。\
         直接输出标题这几个字，不要加「标题」「这段对话在做什么」之类的前缀，不要引号、句号或解释。\
         \n\n用户：{user}\n\n助手：{assistant}"
    );
    clean(&ask_haiku(brain, &prompt, 45).await?)
}

pub(crate) async fn ask_haiku(brain: &Brain, prompt: &str, secs: u64) -> Option<String> {
    let program = brain.program.as_str();
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
        .envs(brain.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
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
    if text.is_empty() || !out.status.success() {
        let err: String = String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(300)
            .collect();
        tracing::warn!(
            target: "blazar::titles",
            "{program} 没有可用输出（{}）：{} {}",
            out.status,
            text.chars().take(120).collect::<String>(),
            err.trim()
        );
        return None;
    }
    if looks_like_error(&text) {
        tracing::warn!(target: "blazar::titles", "{program} 回了一条报错而不是内容：{}", text.chars().take(160).collect::<String>());
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
    let brain = brain(&st, account_of(&st, sid).await.as_deref()).await;
    let Some(title) = summarize(&brain, &user, &assistant).await else {
        tracing::warn!(target: "blazar::titles", "会话 {sid} 没总结出标题（{} 不可用或没有输出）", brain.program);
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
        assert_eq!(clean("Not logged in · Please run /login").as_deref(), None);
        assert!(looks_like_error("API Error: 401 unauthorized\n"));
        assert!(!looks_like_error("修复登录超时"));
        let long = "很".repeat(100);
        assert_eq!(clean(&long).unwrap().chars().count(), MAX_CHARS);
    }

    #[test]
    fn echoed_prompt_prefixes_are_dropped_before_storing() {
        assert_eq!(
            strip_echo("这段对话在做什么：在 lab-3.invalid 上为真机试验做准备"),
            "在 lab-3.invalid 上为真机试验做准备"
        );
        assert_eq!(strip_echo("标题: 修复登录超时"), "修复登录超时");
        assert_eq!(strip_echo("Summary: paginate users"), "paginate users");
        assert_eq!(strip_echo("标题改好了：测试通过"), "标题改好了：测试通过");
        assert_eq!(strip_echo("摘要："), "摘要：");
        assert_eq!(
            clean("这段对话在做什么：在 lab-3.invalid 上为真机试验做准备").as_deref(),
            Some("在 lab-3.invalid 上为真机试验做准备")
        );
    }

    #[test]
    fn long_titles_end_on_a_whole_word_with_brackets_paired() {
        let text = "在 lab-3.invalid 上为真机试验做准备（方案丙，A/B/C 三组对照）再汇总结果";
        assert_eq!(fit(text, 100), text);
        let cut = fit(text, 30);
        assert_eq!(cut, "在 lab-3.invalid 上为真机试验做准备…");
        assert!(fit(text, 40).chars().count() <= 40);
        assert_eq!(
            fit("请求在 lab-3.invalid 上跑 benchmark-suite 然后汇总", 24),
            "请求在 lab-3.invalid 上跑…"
        );
        assert_eq!(
            fit("改好了分页，测试通过，没有推送", 12),
            "改好了分页，测试通过…"
        );
        let cjk = "很".repeat(100);
        assert_eq!(fit(&cjk, MAX_CHARS).chars().count(), MAX_CHARS);
        for s in [fit(text, 30), fit(text, 36), fit(text, 44)] {
            assert_eq!(
                s.chars().filter(|c| OPENERS.contains(c)).count(),
                s.chars().filter(|c| CLOSERS.contains(c)).count(),
                "{s}"
            );
        }
    }
}
