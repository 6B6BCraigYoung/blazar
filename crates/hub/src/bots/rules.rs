use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use blazar_core_types::WorkspaceId;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use tokio::sync::broadcast::error::RecvError;

use super::{Bot, opt, s};
use crate::api::Shared;
use crate::state::ServerEvent;

pub const EVERY: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(2);
const PROBE_EVERY: Duration = Duration::from_secs(120);

pub struct Spec {
    pub key: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    pub unit: &'static str,
    pub default: Option<i64>,
    pub min: i64,
    pub max: i64,
}

pub const SPECS: &[Spec] = &[
    Spec {
        key: "fact_stale",
        label: "事实失联",
        about: "事实超过心跳间隔 5 倍没更新：进收件箱，并写信给负责它的 bot。",
        unit: "",
        default: None,
        min: 0,
        max: 0,
    },
    Spec {
        key: "claim_expired",
        label: "认领过期",
        about: "认领过期时写信给认领人，过期超过这么久自动释放。",
        unit: "小时",
        default: Some(24),
        min: 1,
        max: 720,
    },
    Spec {
        key: "bot_stuck",
        label: "bot 卡住",
        about: "bot 在跑却这么久没有新事件：进收件箱；到两倍时再写信给总控 bot。",
        unit: "分钟",
        default: Some(20),
        min: 1,
        max: 1440,
    },
    Spec {
        key: "approval_waiting",
        label: "审批等待",
        about: "审批等了这么久还没人处理：进收件箱提醒一次。",
        unit: "分钟",
        default: Some(15),
        min: 1,
        max: 1440,
    },
    Spec {
        key: "job_orphan",
        label: "后台任务落单",
        about: "后台任务 waits_for 里的机器、事实、文件或任务不在了：进收件箱，并写信给发起它的 bot。",
        unit: "",
        default: None,
        min: 0,
        max: 0,
    },
    Spec {
        key: "node_offline",
        label: "机器离线",
        about: "机器离线这么久：进收件箱并写信给总控 bot，列出上面还在跑的任务和相关的 bot。打开时每 2 分钟探测一次机器。",
        unit: "分钟",
        default: Some(10),
        min: 1,
        max: 1440,
    },
    Spec {
        key: "quota_high",
        label: "额度快用完",
        about: "账号某个额度窗口的用量超过这个比例：每个窗口提醒一次。",
        unit: "%",
        default: Some(90),
        min: 50,
        max: 100,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    pub enabled: bool,
    pub threshold: i64,
}

fn read(spec: &Spec, saved: &Value) -> Setting {
    let v = &saved[spec.key];
    Setting {
        enabled: v["enabled"].as_bool().unwrap_or(true),
        threshold: v["threshold"]
            .as_i64()
            .filter(|n| spec.default.is_some() && (spec.min..=spec.max).contains(n))
            .or(spec.default)
            .unwrap_or(0),
    }
}

#[must_use]
pub fn settings_of(saved: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for spec in SPECS {
        let c = read(spec, saved);
        let mut v = json!({ "enabled": c.enabled });
        if spec.default.is_some() {
            v["threshold"] = json!(c.threshold);
        }
        out.insert(spec.key.into(), v);
    }
    Value::Object(out)
}

pub fn merge(cur: &mut Value, incoming: &Value) -> Result<(), String> {
    let map = incoming.as_object().ok_or("rules 要是对象")?;
    let mut next = cur.clone();
    for (key, v) in map {
        let spec = SPECS
            .iter()
            .find(|s| s.key == key)
            .ok_or_else(|| format!("没有这条提醒规则：{key}"))?;
        if let Some(on) = v.get("enabled") {
            let on = on
                .as_bool()
                .ok_or_else(|| format!("{} 的开关要是 true 或 false", spec.label))?;
            next[key]["enabled"] = json!(on);
        }
        if let Some(t) = v.get("threshold") {
            if spec.default.is_none() {
                return Err(format!("{} 没有阈值", spec.label));
            }
            let n = t
                .as_i64()
                .filter(|n| (spec.min..=spec.max).contains(n))
                .ok_or_else(|| {
                    format!(
                        "{} 的阈值要在 {} 到 {} {} 之间",
                        spec.label, spec.min, spec.max, spec.unit
                    )
                })?;
            next[key]["threshold"] = json!(n);
        }
    }
    *cur = next;
    Ok(())
}

pub async fn settings(st: &Shared) -> HashMap<&'static str, Setting> {
    let saved = crate::office::kv_get(st, "prefs").await;
    SPECS
        .iter()
        .map(|spec| (spec.key, read(spec, &saved["rules"])))
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct Cond {
    pub subject: String,
    pub since: Option<DateTime<Utc>>,
    pub after_secs: i64,
    pub escalate_after_secs: Option<i64>,
    pub title: String,
    pub body: String,
    pub workspace_id: Option<String>,
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub notify_bot: Option<String>,
    pub about_bot: Option<String>,
}

struct Alert {
    subject: String,
    since: String,
    raised_at: Option<String>,
    escalated_at: Option<String>,
    resolved_at: Option<String>,
    inbox_id: Option<String>,
}

fn parse_time(t: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn local(t: DateTime<Utc>) -> String {
    t.with_timezone(&chrono::Local)
        .format("%m-%d %H:%M")
        .to_string()
}

async fn alerts_of(st: &Shared, rule: &str) -> HashMap<String, Alert> {
    sqlx::query("SELECT * FROM rule_alerts WHERE rule = ?1")
        .bind(rule)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| Alert {
            subject: s(r, "subject"),
            since: s(r, "since"),
            raised_at: opt(r, "raised_at"),
            escalated_at: opt(r, "escalated_at"),
            resolved_at: opt(r, "resolved_at"),
            inbox_id: opt(r, "inbox_id"),
        })
        .map(|a| (a.subject.clone(), a))
        .collect()
}

async fn stamp(st: &Shared, rule: &str, subject: &str, column: &str, inbox: Option<&str>) {
    let _ = sqlx::query(&format!(
        "UPDATE rule_alerts SET {column} = ?3, inbox_id = COALESCE(?4, inbox_id)
         WHERE rule = ?1 AND subject = ?2"
    ))
    .bind(rule)
    .bind(subject)
    .bind(Utc::now().to_rfc3339())
    .bind(inbox)
    .execute(st.db.pool())
    .await;
}

async fn resolve(st: &Shared, rule: &str, a: &Alert) {
    if a.raised_at.is_none() {
        let _ = sqlx::query("DELETE FROM rule_alerts WHERE rule = ?1 AND subject = ?2")
            .bind(rule)
            .bind(&a.subject)
            .execute(st.db.pool())
            .await;
        return;
    }
    stamp(st, rule, &a.subject, "resolved_at", None).await;
    if let Some(id) = &a.inbox_id {
        crate::inbox::mark_resolved(st, id).await;
    }
}

pub async fn lead(st: &Shared) -> Option<Bot> {
    super::all(st).await.into_iter().find(Bot::is_lead)
}

async fn escalate(st: &Shared, rule: &'static str, c: &Cond, age_secs: i64) {
    let Some(lead) = lead(st).await else {
        return;
    };
    if c.about_bot.as_deref() == Some(lead.id.as_str()) {
        return;
    }
    let held = crate::bots::jobs::took(
        &(Utc::now() - chrono::Duration::seconds(age_secs)).to_rfc3339(),
        None,
    );
    let sent = super::send(
        st,
        &lead.id,
        super::NewMail {
            body: format!(
                "{}\n{}\n已经持续 {held}。用户的收件箱里也有一条。",
                c.title, c.body
            ),
            subject: Some(c.title.clone()),
            from_kind: "rule".into(),
            from_ref: rule.into(),
            ..super::NewMail::default()
        },
    )
    .await;
    if let Err(e) = sent {
        tracing::warn!(target: "blazar::rules", "给总控 bot 发 {rule} 提醒失败: {e}");
    }
}

async fn notify(st: &Shared, rule: &'static str, bot: &str, c: &Cond) {
    let sent = super::send(
        st,
        bot,
        super::NewMail {
            body: format!("{}\n{}", c.title, c.body),
            subject: Some(c.title.clone()),
            from_kind: "rule".into(),
            from_ref: rule.into(),
            ..super::NewMail::default()
        },
    )
    .await;
    if let Err(e) = sent {
        tracing::warn!(target: "blazar::rules", "给 bot 发 {rule} 提醒失败: {e}");
    }
}

pub async fn apply(st: &Shared, rule: &'static str, conds: Vec<Cond>) {
    let now = Utc::now();
    let rows = alerts_of(st, rule).await;
    let mut seen = HashSet::new();
    for c in conds {
        if !seen.insert(c.subject.clone()) {
            continue;
        }
        let open = rows.get(&c.subject).filter(|a| a.resolved_at.is_none());
        let since = c
            .since
            .or_else(|| open.and_then(|a| parse_time(&a.since)))
            .unwrap_or(now);
        if open.is_none() {
            let _ = sqlx::query(
                "INSERT INTO rule_alerts (rule, subject, since) VALUES (?1, ?2, ?3)
                 ON CONFLICT (rule, subject) DO UPDATE SET since = excluded.since,
                     raised_at = NULL, escalated_at = NULL, resolved_at = NULL, inbox_id = NULL",
            )
            .bind(rule)
            .bind(&c.subject)
            .bind(since.to_rfc3339())
            .execute(st.db.pool())
            .await;
        } else if c.since.is_some() {
            let _ = sqlx::query(
                "UPDATE rule_alerts SET since = ?3 WHERE rule = ?1 AND subject = ?2 AND since != ?3",
            )
            .bind(rule)
            .bind(&c.subject)
            .bind(since.to_rfc3339())
            .execute(st.db.pool())
            .await;
        }
        let age = (now - since).num_seconds();
        if age < c.after_secs {
            if let Some(a) = open.filter(|a| a.raised_at.is_some()) {
                resolve(st, rule, a).await;
            }
            continue;
        }
        if open.and_then(|a| a.raised_at.as_ref()).is_none() {
            let id = crate::inbox::push_id(
                st,
                crate::inbox::Item {
                    kind: "alert",
                    title: c.title.clone(),
                    body: c.body.clone(),
                    workspace_id: c.workspace_id.clone(),
                    thread_id: c.thread_id.clone(),
                    session_id: c.session_id.clone(),
                    ref_id: Some(format!("rule:{rule}:{}", c.subject)),
                },
            )
            .await;
            stamp(st, rule, &c.subject, "raised_at", id.as_deref()).await;
            if let Some(bot) = &c.notify_bot {
                notify(st, rule, bot, &c).await;
            }
        }
        if let Some(after) = c.escalate_after_secs
            && age >= after
            && open.and_then(|a| a.escalated_at.as_ref()).is_none()
        {
            stamp(st, rule, &c.subject, "escalated_at", None).await;
            escalate(st, rule, &c, age).await;
        }
    }
    for a in rows
        .values()
        .filter(|a| a.resolved_at.is_none() && !seen.contains(&a.subject))
    {
        resolve(st, rule, a).await;
    }
}

async fn fact_stale(st: &Shared) -> Vec<Cond> {
    sqlx::query(
        "SELECT f.key, f.ttl_secs, f.updated_at, b.handle AS owner_handle
         FROM facts f LEFT JOIN bots b ON b.id = f.owner_bot WHERE f.status = 'lost'",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| {
        let key = s(r, "key");
        let ttl: i64 = r.try_get("ttl_secs").ok().flatten().unwrap_or_default();
        let last = parse_time(&s(r, "updated_at")).map_or_else(String::new, local);
        Cond {
            subject: key.clone(),
            title: format!("事实 {key} 失联"),
            body: format!(
                "超过心跳间隔（{ttl} 秒）的 5 倍没有更新，最后一次在 {last}{}。",
                opt(r, "owner_handle")
                    .map(|h| format!("，负责人 bot:{h}"))
                    .unwrap_or_default()
            ),
            ..Cond::default()
        }
    })
    .collect()
}

async fn session_thread(st: &Shared, sid: &str) -> Option<String> {
    sqlx::query_scalar("SELECT COALESCE(thread_id, id) FROM sessions WHERE id = ?1")
        .bind(sid)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
}

async fn bot_stuck(st: &Shared, mins: i64) -> Vec<Cond> {
    let now = Utc::now();
    let mut out = Vec::new();
    for bot in super::all(st).await {
        if bot.status != "running" {
            continue;
        }
        let Ok(ws) = bot.run_workspace().parse::<uuid::Uuid>() else {
            continue;
        };
        let Some(live) = super::status::live_in(st, WorkspaceId(ws)).await else {
            continue;
        };
        if live.idle {
            continue;
        }
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM approvals WHERE session_id = ?1 AND decision IS NULL",
        )
        .bind(&live.session)
        .fetch_one(st.db.pool())
        .await
        .unwrap_or(0);
        if waiting > 0 {
            continue;
        }
        let last: Option<String> = sqlx::query_scalar(
            "SELECT COALESCE((SELECT MAX(ts) FROM events WHERE session_id = ?1), created_at)
             FROM sessions WHERE id = ?1",
        )
        .bind(&live.session)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
        let Some(since) = last.as_deref().and_then(parse_time) else {
            continue;
        };
        if (now - since).num_seconds() < mins * 60 {
            continue;
        }
        out.push(Cond {
            subject: bot.id.clone(),
            since: Some(since),
            after_secs: mins * 60,
            escalate_after_secs: Some(mins * 120),
            title: format!("bot:{} 超过 {mins} 分钟没有动静", bot.handle),
            body: format!(
                "它的会话还在跑，最后一个事件在 {}。可能卡在某个命令或网络上，到 bot 页面看看，必要时打断它。",
                local(since)
            ),
            workspace_id: Some(bot.run_workspace().to_owned()),
            thread_id: session_thread(st, &live.session).await,
            session_id: Some(live.session.clone()),
            about_bot: Some(bot.id.clone()),
            ..Cond::default()
        });
    }
    out
}

async fn approval_waiting(st: &Shared, mins: i64) -> Vec<Cond> {
    let before = (Utc::now() - chrono::Duration::minutes(mins)).to_rfc3339();
    sqlx::query(
        "SELECT a.id, a.request, a.created_at, s.id AS sid, s.workspace_id,
                COALESCE(s.thread_id, s.id) AS thread, w.name AS ws
         FROM approvals a JOIN sessions s ON s.id = a.session_id
         LEFT JOIN workspaces w ON w.id = s.workspace_id
         WHERE a.decision IS NULL AND s.status = 'running' AND a.created_at <= ?1",
    )
    .bind(before)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| {
        let request: Value = serde_json::from_str(&s(r, "request")).unwrap_or_default();
        let tool = request["tool_name"]
            .as_str()
            .or(request["tool"].as_str())
            .unwrap_or("一个操作");
        Cond {
            subject: s(r, "id"),
            since: parse_time(&s(r, "created_at")),
            after_secs: mins * 60,
            title: format!("有个审批等了 {mins} 分钟还没处理"),
            body: format!(
                "工作区 {} 里的 {tool} 在等你批准，会话停在这里。",
                opt(r, "ws").unwrap_or_else(|| "（已删除）".into())
            ),
            workspace_id: opt(r, "workspace_id"),
            thread_id: opt(r, "thread"),
            session_id: opt(r, "sid"),
            ..Cond::default()
        }
    })
    .collect()
}

async fn gone_target(st: &Shared, job: &super::jobs::Job, kind: &str, v: &str) -> Option<String> {
    match kind {
        "node" => {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM nodes WHERE name = ?1")
                    .bind(v)
                    .fetch_optional(st.db.pool())
                    .await
                    .ok()
                    .flatten();
            match status.as_deref() {
                None => Some(format!("机器 {v} 已经删掉了")),
                Some("offline") => Some(format!("机器 {v} 离线")),
                Some(_) => None,
            }
        }
        "fact" => match super::facts::get(st, v).await {
            None => Some(format!("事实 {v} 不存在")),
            Some(f) if f.status == "lost" => Some(format!("事实 {v} 失联")),
            Some(_) => None,
        },
        "path" => super::jobs::missing_paths(st, &job.id)
            .iter()
            .any(|p| p == v)
            .then(|| format!("{} 上的 {v} 不见了", job.node)),
        "job" => match super::jobs::get(st, v).await {
            None => Some(format!("后台任务 {v} 不存在")),
            Some(t) if t.status != "running" => {
                Some(format!("后台任务「{}」已经是 {}", t.label, t.status))
            }
            Some(_) => None,
        },
        _ => None,
    }
}

async fn job_orphan(st: &Shared) -> Vec<Cond> {
    let filter = super::jobs::Filter {
        status: Some("running".into()),
        ..super::jobs::Filter::default()
    };
    let mut out = Vec::new();
    for job in super::jobs::list(st, &filter, None)
        .await
        .unwrap_or_default()
    {
        let mut gone = Vec::new();
        for (kind, v) in job.waits() {
            if let Some(why) = gone_target(st, &job, &kind, &v).await {
                gone.push(why);
            }
        }
        if gone.is_empty() {
            continue;
        }
        out.push(Cond {
            subject: job.id.clone(),
            title: format!("后台任务「{}」等的东西不在了", job.label),
            body: format!(
                "{}。任务还在 {} 上跑，发起方 {}。不需要了就到「后台任务」里结束它。",
                gone.join("；"),
                job.node,
                job.owner_label()
            ),
            workspace_id: job.workspace_id.clone(),
            notify_bot: job.bot_id.clone().filter(|_| job.owner_kind == "bot"),
            ..Cond::default()
        });
    }
    out
}

async fn node_offline(st: &Shared, mins: i64) -> Vec<Cond> {
    let nodes: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, name FROM nodes WHERE status = 'offline' AND transport != 'local'",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut out = Vec::new();
    for (id, name) in nodes {
        let jobs: Vec<String> = sqlx::query_scalar(
            "SELECT label FROM jobs WHERE node_id = ?1 AND status = 'running' ORDER BY created_at",
        )
        .bind(&id)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
        let bots: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT b.handle FROM bots b
             WHERE b.archived_at IS NULL AND (
                 EXISTS (SELECT 1 FROM workspaces w WHERE w.node_id = ?1
                         AND (w.id = b.home_ws_id OR w.id = b.brain_ws_id))
                 OR EXISTS (SELECT 1 FROM claims c WHERE c.node_id = ?1 AND c.bot_id = b.id))
             ORDER BY b.handle",
        )
        .bind(&id)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
        let jobs = if jobs.is_empty() {
            "上面没有还在跑的后台任务。".to_owned()
        } else {
            let labels: Vec<String> = jobs.iter().map(|l| format!("「{l}」")).collect();
            format!("还在跑的后台任务：{}。", labels.join("、"))
        };
        let bots = if bots.is_empty() {
            "没有家在这台机器上的 bot。".to_owned()
        } else {
            let handles: Vec<String> = bots.iter().map(|h| format!("bot:{h}")).collect();
            format!(
                "相关的 bot：{}，离线期间不给它们派活，恢复在线后留着的邮件照常处理。",
                handles.join("、")
            )
        };
        out.push(Cond {
            subject: name.clone(),
            after_secs: mins * 60,
            escalate_after_secs: Some(mins * 60),
            title: format!("机器 {name} 离线超过 {mins} 分钟"),
            body: format!("{jobs}{bots}恢复在线后这条会自动标为已解决。"),
            ..Cond::default()
        });
    }
    out
}

fn window_label(name: &str) -> String {
    match name {
        "five_hour" => "5 小时窗口".into(),
        "seven_day" => "7 天窗口".into(),
        "seven_day_opus" => "7 天 Opus 窗口".into(),
        "blocked" => "额度（已被限流）".into(),
        other => other.to_owned(),
    }
}

async fn quota_high(st: &Shared, pct: i64) -> Vec<Cond> {
    let now = Utc::now();
    sqlx::query(
        "SELECT r.account_id, r.window_name, r.utilization, r.resets_at, a.display_name
         FROM rate_limit_snapshots r JOIN accounts a ON a.id = r.account_id
         ORDER BY a.display_name, r.window_name",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .filter_map(|r| {
        let used: f64 = r.try_get("utilization").unwrap_or(0.0);
        let resets = opt(r, "resets_at").as_deref().and_then(parse_time);
        let over = used * 100.0 >= i32::try_from(pct).map_or(f64::MAX, f64::from);
        if !over || resets.is_some_and(|t| t <= now) {
            return None;
        }
        let window = s(r, "window_name");
        Some(Cond {
            subject: format!("{}:{window}", s(r, "account_id")),
            title: format!(
                "账号「{}」{}已用 {:.0}%",
                s(r, "display_name"),
                window_label(&window),
                used * 100.0
            ),
            body: resets.map_or_else(
                || "还不知道什么时候重置。".into(),
                |t| format!("{} 重置。", local(t)),
            ),
            ..Cond::default()
        })
    })
    .collect()
}

pub async fn offline_node_of(st: &Shared, bot: &Bot) -> Option<String> {
    sqlx::query_scalar(
        "SELECT n.name FROM workspaces w JOIN nodes n ON n.id = w.node_id
         WHERE w.id IN (?1, ?2) AND n.status = 'offline' LIMIT 1",
    )
    .bind(&bot.home_ws_id)
    .bind(bot.run_workspace())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
}

pub async fn run_once(st: &Shared) {
    let cfg = settings(st).await;
    let on = |k: &str| cfg.get(k).copied().filter(|c| c.enabled);
    crate::events::prune(
        &st.db,
        Utc::now(),
        crate::events::KEEP_SECS,
        crate::events::KEEP_ROWS,
    )
    .await;
    super::facts::sweep(st, on("fact_stale").is_some()).await;
    if let Some(c) = on("claim_expired") {
        super::claims::sweep(st, c.threshold).await;
    }
    for spec in SPECS {
        let conds = match on(spec.key) {
            None => Vec::new(),
            Some(c) => match spec.key {
                "fact_stale" => fact_stale(st).await,
                "bot_stuck" => bot_stuck(st, c.threshold).await,
                "approval_waiting" => approval_waiting(st, c.threshold).await,
                "job_orphan" => job_orphan(st).await,
                "node_offline" => node_offline(st, c.threshold).await,
                "quota_high" => quota_high(st, c.threshold).await,
                _ => continue,
            },
        };
        apply(st, spec.key, conds).await;
    }
}

fn triggers(ev: &ServerEvent) -> bool {
    matches!(
        ev,
        ServerEvent::NodesChanged
            | ServerEvent::AccountsChanged
            | ServerEvent::JobsChanged
            | ServerEvent::FactsChanged
            | ServerEvent::BotsChanged
    )
}

async fn probe_nodes(st: &Shared) {
    if st.mesh_ctx.topology_label(&st.mesh_via).is_some() {
        let _ = crate::api::refresh_mesh(State(st.clone())).await;
    } else {
        crate::api::probe_ssh_nodes(st).await;
    }
}

pub async fn probe_due(st: &Shared, last: Option<Instant>) -> bool {
    settings(st)
        .await
        .get("node_offline")
        .is_some_and(|c| c.enabled)
        && last.is_none_or(|t| t.elapsed() >= PROBE_EVERY)
}

pub async fn watch(st: Shared) {
    let mut rx = st.bus.subscribe();
    let mut probed: Option<Instant> = None;
    loop {
        if probe_due(&st, probed).await {
            probed = Some(Instant::now());
            let probe = st.clone();
            tokio::spawn(async move { probe_nodes(&probe).await });
        }
        run_once(&st).await;
        let sleep = tokio::time::sleep(EVERY);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
                ev = rx.recv() => match ev {
                    Ok(ev) if triggers(&ev.event) => {
                        tokio::time::sleep(SETTLE).await;
                        while rx.try_recv().is_ok() {}
                        break;
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => {
                        (&mut sleep).await;
                        break;
                    }
                },
            }
        }
    }
}

pub async fn list(State(st): State<Shared>) -> Response {
    let saved = crate::office::kv_get(&st, "prefs").await;
    let open: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
        "SELECT rule, COUNT(*) FROM rule_alerts
         WHERE raised_at IS NOT NULL AND resolved_at IS NULL GROUP BY rule",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .into_iter()
    .collect();
    let rules: Vec<Value> = SPECS
        .iter()
        .map(|spec| {
            let c = read(spec, &saved["rules"]);
            json!({
                "key": spec.key,
                "label": spec.label,
                "about": spec.about,
                "unit": spec.unit,
                "min": spec.min,
                "max": spec.max,
                "default": spec.default,
                "enabled": c.enabled,
                "threshold": spec.default.map(|_| c.threshold),
                "open": open.get(spec.key).copied().unwrap_or(0),
            })
        })
        .collect();
    Json(json!({ "rules": rules })).into_response()
}

#[cfg(test)]
#[path = "rules_tests.rs"]
mod tests;
