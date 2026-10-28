use blazar_core_types::{SessionId, WorkspaceId};
use chrono::{DateTime, Utc};

use crate::api::Shared;

use super::{Job, Owner, SUMMARY_LINES};

#[must_use]
pub fn took(from: &str, to: Option<&str>) -> String {
    let secs = elapsed(from, to);
    match secs {
        0..60 => format!("{secs} 秒"),
        60..3600 => format!("{} 分 {} 秒", secs / 60, secs % 60),
        _ => format!("{} 小时 {} 分", secs / 3600, secs % 3600 / 60),
    }
}

fn took_en(from: &str, to: Option<&str>) -> String {
    let secs = elapsed(from, to);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

fn elapsed(from: &str, to: Option<&str>) -> i64 {
    let parse = |t: &str| {
        DateTime::parse_from_rfc3339(t)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let end = to.and_then(parse).unwrap_or_else(Utc::now);
    parse(from).map_or(0, |start| (end - start).num_seconds().max(0))
}

fn headline(job: &Job) -> String {
    let how = match job.status.as_str() {
        "done" => "跑完了",
        "failed" => "失败了",
        "killed" => "被结束了",
        "lost" => "失联了",
        _ => "还在跑",
    };
    format!("后台任务「{}」{how}", job.label)
}

fn headline_en(job: &Job) -> String {
    let how = match job.status.as_str() {
        "done" => "finished",
        "failed" => "failed",
        "killed" => "was killed",
        "lost" => "was lost",
        _ => "is still running",
    };
    format!("Background job \"{}\" {how}", job.label)
}

#[must_use]
pub fn summary(job: &Job, tail: &str) -> String {
    let code = match (job.exit_code, job.status.as_str()) {
        (Some(c), _) => format!("退出码：{c}"),
        (None, "killed") => "退出码：无（被结束）".into(),
        (None, "lost") => "退出码：无（任务目录或进程不见了）".into(),
        (None, _) => "退出码未知（登记的进程拿不到退出码）".into(),
    };
    let lines: Vec<&str> = tail.lines().collect();
    let tail = lines[lines.len().saturating_sub(SUMMARY_LINES)..].join("\n");
    let mut out = format!(
        "{}。\n任务 id：{}\n机器：{}\n{code}\n耗时：{}\n",
        headline(job),
        job.id,
        job.node,
        took(&job.created_at, job.ended_at.as_deref()),
    );
    if let Some(c) = &job.command {
        out.push_str(&format!("命令：{c}\n"));
    }
    if tail.trim().is_empty() {
        out.push_str("日志最后 20 行：（没有输出）\n");
    } else {
        out.push_str(&format!("日志最后 20 行：\n```\n{tail}\n```\n"));
    }
    out
}

#[must_use]
pub fn summary_en(job: &Job, tail: &str) -> String {
    let code = match (job.exit_code, job.status.as_str()) {
        (Some(c), _) => format!("Exit code: {c}"),
        (None, "killed") => "Exit code: none (killed)".into(),
        (None, "lost") => "Exit code: none (job directory or process is gone)".into(),
        (None, _) => "Exit code: unknown (registered processes don't report one)".into(),
    };
    let lines: Vec<&str> = tail.lines().collect();
    let tail = lines[lines.len().saturating_sub(SUMMARY_LINES)..].join("\n");
    let mut out = format!(
        "{}.\nJob id: {}\nMachine: {}\n{code}\nDuration: {}\n",
        headline_en(job),
        job.id,
        job.node,
        took_en(&job.created_at, job.ended_at.as_deref()),
    );
    if let Some(c) = &job.command {
        out.push_str(&format!("Command: {c}\n"));
    }
    if tail.trim().is_empty() {
        out.push_str("Last 20 log lines: (no output)\n");
    } else {
        out.push_str(&format!("Last 20 log lines:\n```\n{tail}\n```\n"));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Resume,
    Interject,
    Busy,
}

pub async fn route(st: &Shared, ws: WorkspaceId, thread: &str) -> Wake {
    let running: Option<(SessionId, bool)> = st
        .running
        .read()
        .await
        .iter()
        .find(|(_, r)| r.workspace_id == ws)
        .map(|(sid, r)| (*sid, r.live.as_ref().is_some_and(|l| l.interactive)));
    let Some((sid, interactive)) = running else {
        return Wake::Resume;
    };
    let its: Option<String> =
        sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
            .bind(sid.to_string())
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten();
    if interactive && its.as_deref() == Some(thread) {
        Wake::Interject
    } else {
        Wake::Busy
    }
}

async fn wake_session(st: &Shared, thread: &str, text: &str) -> Result<(), String> {
    let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT workspace_id, runtime_kind, agent_profile, node FROM sessions
         WHERE COALESCE(thread_id, id) = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    let (ws, runtime, profile, node) =
        row.ok_or("Couldn't find the session that started the job")?;
    let wsid = WorkspaceId(ws.parse().map_err(|_| "Invalid workspace id")?);
    let how = route(st, wsid, thread).await;
    if how == Wake::Busy {
        return Err("The workspace is running another conversation".into());
    }
    let agent = if runtime == "claude_cli" {
        "claude".to_owned()
    } else {
        runtime
    };
    let req = crate::api::PromptRequest {
        text: format!("(Hub notice) A background job you started has ended.\n\n{text}"),
        resume: true,
        resume_session: Some(thread.to_owned()),
        interject: how == Wake::Interject,
        agent: Some(agent),
        profile,
        brain: Some(if node.as_deref().is_none_or(|n| n == "local") {
            "local".into()
        } else {
            "node".into()
        }),
        ..crate::api::PromptRequest::default()
    };
    let out = crate::api::prompt_for_bot(st.clone(), ws, req).await?;
    if out["admitted"] == false {
        return Err(out["reason"]
            .as_str()
            .unwrap_or("The workspace is busy")
            .to_owned());
    }
    Ok(())
}

async fn to_inbox(st: &Shared, job: &Job, text: String) {
    let thread = job
        .session_id
        .clone()
        .filter(|_| job.owner_kind == "session");
    let home: Option<String> = match &thread {
        Some(t) => sqlx::query_scalar(
            "SELECT workspace_id FROM sessions WHERE COALESCE(thread_id, id) = ?1
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(t)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten(),
        None => None,
    };
    crate::inbox::push(
        st,
        crate::inbox::Item {
            kind: "job",
            title: headline(job),
            body: text,
            workspace_id: home.clone().or_else(|| job.workspace_id.clone()),
            thread_id: thread.filter(|_| home.is_some()),
            session_id: None,
            ref_id: Some(job.id.clone()),
        },
    )
    .await;
}

pub async fn announce(st: &Shared, job: &Job, tail: &str) {
    if job.notify == "none" {
        return;
    }
    let text = summary(job, tail);
    if job.notify == "wake" {
        let en = summary_en(job, tail);
        match job.owner() {
            Owner::Bot { bot, thread } => {
                let hint = match thread {
                    Some(t) => crate::bots::thread_by_session_thread(st, &t)
                        .await
                        .filter(|th| th.bot_id == bot)
                        .map(|th| th.id),
                    None => None,
                };
                let sent = crate::bots::send(
                    st,
                    &bot,
                    crate::bots::NewMail {
                        body: en.clone(),
                        subject: Some(headline_en(job)),
                        thread: hint,
                        from_kind: "system".into(),
                        from_ref: format!("job:{}", job.id),
                        ..crate::bots::NewMail::default()
                    },
                )
                .await;
                match sent {
                    Ok(_) => return,
                    Err(e) => {
                        tracing::warn!(target: "blazar::jobs", "给 bot 发后台任务通知失败，改进收件箱: {e}");
                    }
                }
            }
            Owner::Session { thread, .. } => match wake_session(st, &thread, &en).await {
                Ok(()) => return,
                Err(e) => {
                    tracing::info!(target: "blazar::jobs", "没能唤醒发起任务的会话，改进收件箱: {e}");
                }
            },
            Owner::User => {}
        }
    }
    to_inbox(st, job, text).await;
}
