use std::collections::BTreeSet;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;

use super::{Caller, hands_name, opt, s};
use crate::api::Shared;

pub const MODES: &[&str] = &["exclusive", "shared", "read"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub bot_id: String,
    pub bot_handle: String,
    pub node_id: String,
    pub node: String,
    pub workspace_id: Option<String>,
    pub workspace_path: Option<String>,
    pub scope: String,
    pub mode: String,
    pub note: String,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub hands: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Gpu(BTreeSet<u32>),
    Dir(String),
    Other(String),
}

pub fn parse_scope(raw: &str) -> Result<Scope, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().count() > 300 || raw.chars().any(char::is_control) {
        return Err("范围写法：gpu:0,1 · dir:/data/queue · repo · port:8766 · device:can1".into());
    }
    if let Some(list) = raw.strip_prefix("gpu:") {
        let mut set = BTreeSet::new();
        for part in list.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((a, b)) = part.split_once('-') {
                let (a, b) = (
                    a.trim()
                        .parse::<u32>()
                        .map_err(|_| format!("GPU 编号 {part} 看不懂"))?,
                    b.trim()
                        .parse::<u32>()
                        .map_err(|_| format!("GPU 编号 {part} 看不懂"))?,
                );
                if a > b || b - a > 64 {
                    return Err(format!("GPU 区间 {part} 不合理"));
                }
                set.extend(a..=b);
            } else {
                set.insert(
                    part.parse()
                        .map_err(|_| format!("GPU 编号 {part} 看不懂"))?,
                );
            }
        }
        if set.is_empty() {
            return Err("gpu: 后面要跟编号，比如 gpu:0,1".into());
        }
        return Ok(Scope::Gpu(set));
    }
    if let Some(dir) = raw.strip_prefix("dir:") {
        let dir = dir.trim().trim_end_matches('/');
        if !dir.starts_with('/') {
            return Err("dir: 后面要跟绝对路径".into());
        }
        return Ok(Scope::Dir(dir.to_owned()));
    }
    Ok(Scope::Other(raw.to_owned()))
}

#[must_use]
pub fn canonical(scope: &Scope) -> String {
    match scope {
        Scope::Gpu(set) => format!(
            "gpu:{}",
            set.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Scope::Dir(d) => format!("dir:{d}"),
        Scope::Other(o) => o.clone(),
    }
}

#[must_use]
pub fn overlaps(a: &Scope, b: &Scope) -> bool {
    match (a, b) {
        (Scope::Gpu(x), Scope::Gpu(y)) => x.intersection(y).next().is_some(),
        (Scope::Dir(x), Scope::Dir(y)) => {
            x == y || x.starts_with(&format!("{y}/")) || y.starts_with(&format!("{x}/"))
        }
        (Scope::Other(x), Scope::Other(y)) => x == y,
        _ => false,
    }
}

#[must_use]
pub fn conflicts(a_mode: &str, a: &Scope, b_mode: &str, b: &Scope) -> bool {
    if a_mode == "read" || b_mode == "read" {
        return false;
    }
    if a_mode == "shared" && b_mode == "shared" {
        return false;
    }
    overlaps(a, b)
}

fn claim_of(r: &SqliteRow) -> Claim {
    let node = s(r, "node");
    let workspace_path = opt(r, "workspace_path");
    Claim {
        id: s(r, "id"),
        bot_id: s(r, "bot_id"),
        bot_handle: s(r, "bot_handle"),
        node_id: s(r, "node_id"),
        hands: workspace_path.as_ref().map(|_| hands_name(&node)),
        node,
        workspace_id: opt(r, "workspace_id"),
        workspace_path,
        scope: s(r, "scope"),
        mode: s(r, "mode"),
        note: s(r, "note"),
        expires_at: opt(r, "expires_at"),
        created_at: s(r, "created_at"),
    }
}

const SELECT: &str = "SELECT c.*, n.name AS node, b.handle AS bot_handle, w.path AS workspace_path
     FROM claims c JOIN nodes n ON n.id = c.node_id JOIN bots b ON b.id = c.bot_id
     LEFT JOIN workspaces w ON w.id = c.workspace_id";

pub async fn all(st: &Shared) -> Vec<Claim> {
    sqlx::query(&format!("{SELECT} ORDER BY c.created_at"))
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default()
        .iter()
        .map(claim_of)
        .collect()
}

pub async fn of_bot(st: &Shared, bot_id: &str) -> Vec<Claim> {
    sqlx::query(&format!(
        "{SELECT} WHERE c.bot_id = ?1 ORDER BY c.created_at"
    ))
    .bind(bot_id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(claim_of)
    .collect()
}

pub async fn get(st: &Shared, id: &str) -> Option<Claim> {
    sqlx::query(&format!("{SELECT} WHERE c.id = ?1"))
        .bind(id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| claim_of(&r))
}

pub async fn workspaces_of(st: &Shared, bot_id: &str) -> BTreeSet<String> {
    of_bot(st, bot_id)
        .await
        .into_iter()
        .filter_map(|c| c.workspace_id)
        .collect()
}

#[derive(Debug, Default, Deserialize)]
pub struct NewClaim {
    #[serde(default)]
    pub bot: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    pub scope: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub expires_in_secs: Option<i64>,
}

pub async fn create(st: &Shared, req: NewClaim, by: Option<&Caller>) -> Result<Claim, String> {
    let bot_ref = match (by, req.bot.as_deref()) {
        (Some(c), _) => c.bot.clone(),
        (None, Some(b)) => b.to_owned(),
        (None, None) => return Err("要说明是哪个 bot 在认领".into()),
    };
    let bot = super::get(st, &bot_ref).await.ok_or("没有这个 bot")?;
    let mode = req.mode.unwrap_or_else(|| "exclusive".into());
    if !MODES.contains(&mode.as_str()) {
        return Err(format!("未知模式 {mode}，可用 exclusive / shared / read"));
    }
    let scope = parse_scope(&req.scope)?;
    let (node_id, node_name, workspace_id) = match req
        .workspace_id
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        Some(ws) => {
            let row = sqlx::query(
                "SELECT n.id, n.name FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
            )
            .bind(ws)
            .fetch_optional(st.db.pool())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("工作区不存在")?;
            (s(&row, "id"), s(&row, "name"), Some(ws.to_owned()))
        }
        None => {
            let name = req.node.as_deref().map(str::trim).unwrap_or("local");
            let id: String = sqlx::query_scalar("SELECT id FROM nodes WHERE name = ?1")
                .bind(name)
                .fetch_optional(st.db.pool())
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("没有叫 {name} 的机器"))?;
            (id, name.to_owned(), None)
        }
    };
    let now = Utc::now();
    let others: Vec<Claim> = sqlx::query(&format!(
        "{SELECT} WHERE c.node_id = ?1 AND c.bot_id != ?2 AND (c.expires_at IS NULL OR c.expires_at >= ?3)"
    ))
    .bind(&node_id)
    .bind(&bot.id)
    .bind(now.to_rfc3339())
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(claim_of)
    .collect();
    for o in &others {
        let Ok(theirs) = parse_scope(&o.scope) else {
            continue;
        };
        if conflicts(&mode, &scope, &o.mode, &theirs) {
            return Err(format!(
                "和 bot:{} 在 {} 上的认领 {}（{}）冲突，先用 send_message 找它商量",
                o.bot_handle, node_name, o.scope, o.mode
            ));
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    let expires = req
        .expires_in_secs
        .filter(|x| *x > 0)
        .map(|x| (now + chrono::Duration::seconds(x)).to_rfc3339());
    sqlx::query(
        "INSERT INTO claims (id, bot_id, node_id, workspace_id, scope, mode, note, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&id)
    .bind(&bot.id)
    .bind(&node_id)
    .bind(workspace_id)
    .bind(canonical(&scope))
    .bind(&mode)
    .bind(req.note.unwrap_or_default().chars().take(200).collect::<String>())
    .bind(expires)
    .bind(now.to_rfc3339())
    .execute(st.db.pool())
    .await
    .map_err(|e| e.to_string())?;
    super::changed(st);
    get(st, &id).await.ok_or_else(|| "认领读不回来".into())
}

pub async fn release(st: &Shared, id: &str, by: Option<&Caller>) -> Result<(), String> {
    let c = get(st, id).await.ok_or("没有这条认领")?;
    if let Some(caller) = by
        && caller.bot != c.bot_id
        && !caller.is_lead()
    {
        return Err(format!(
            "这是 bot:{} 的认领，只有它自己或总控能释放",
            c.bot_handle
        ));
    }
    sqlx::query("DELETE FROM claims WHERE id = ?1")
        .bind(&c.id)
        .execute(st.db.pool())
        .await
        .map_err(|e| e.to_string())?;
    super::changed(st);
    Ok(())
}

pub async fn sweep(st: &Shared) {
    let now = Utc::now();
    let expired: Vec<Claim> = sqlx::query(&format!(
        "{SELECT} WHERE c.expires_at IS NOT NULL AND c.expires_at < ?1"
    ))
    .bind(now.to_rfc3339())
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(claim_of)
    .collect();
    for c in expired {
        let notified: Option<String> =
            sqlx::query_scalar("SELECT notified_at FROM claims WHERE id = ?1")
                .bind(&c.id)
                .fetch_optional(st.db.pool())
                .await
                .ok()
                .flatten()
                .flatten();
        match notified {
            None => {
                let _ = sqlx::query("UPDATE claims SET notified_at = ?2 WHERE id = ?1")
                    .bind(&c.id)
                    .bind(now.to_rfc3339())
                    .execute(st.db.pool())
                    .await;
                let _ = super::send(
                    st,
                    &c.bot_id,
                    super::NewMail {
                        body: format!(
                            "你在 {} 上认领的 {}（{}）已经过期。还需要就重新 claim，24 小时后会自动释放。",
                            c.node, c.scope, c.mode
                        ),
                        subject: Some("认领过期".into()),
                        from_kind: "rule".into(),
                        from_ref: "claim_expired".into(),
                        ..super::NewMail::default()
                    },
                )
                .await;
            }
            Some(at) if at < (now - chrono::Duration::hours(24)).to_rfc3339() => {
                let _ = sqlx::query("DELETE FROM claims WHERE id = ?1")
                    .bind(&c.id)
                    .execute(st.db.pool())
                    .await;
                super::changed(st);
            }
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(s: &str) -> Scope {
        parse_scope(s).unwrap()
    }

    #[test]
    fn scopes_parse_and_canonicalize() {
        assert_eq!(canonical(&sc("gpu:1, 0,3-4")), "gpu:0,1,3,4");
        assert_eq!(canonical(&sc("dir:/data/queue/")), "dir:/data/queue");
        assert_eq!(canonical(&sc("port:8766")), "port:8766");
        assert!(parse_scope("gpu:").is_err());
        assert!(parse_scope("gpu:a").is_err());
        assert!(parse_scope("dir:relative").is_err());
        assert!(parse_scope("").is_err());
    }

    #[test]
    fn gpu_sets_overlap_only_when_they_share_a_card() {
        assert!(overlaps(&sc("gpu:0,1"), &sc("gpu:1,2")));
        assert!(!overlaps(&sc("gpu:0,1"), &sc("gpu:2,3")));
        assert!(!overlaps(&sc("gpu:0"), &sc("dir:/gpu")));
    }

    #[test]
    fn directories_overlap_by_prefix() {
        assert!(overlaps(&sc("dir:/data"), &sc("dir:/data/queue")));
        assert!(overlaps(&sc("dir:/data/queue"), &sc("dir:/data")));
        assert!(!overlaps(&sc("dir:/data2"), &sc("dir:/data")));
        assert!(overlaps(&sc("repo"), &sc("repo")));
        assert!(!overlaps(&sc("port:1"), &sc("port:2")));
    }

    #[test]
    fn exclusive_conflicts_shared_does_not_read_never() {
        let (a, b) = (sc("gpu:0,1"), sc("gpu:1"));
        assert!(conflicts("exclusive", &a, "exclusive", &b));
        assert!(conflicts("exclusive", &a, "shared", &b));
        assert!(conflicts("shared", &a, "exclusive", &b));
        assert!(!conflicts("shared", &a, "shared", &b));
        assert!(!conflicts("exclusive", &a, "read", &b));
        assert!(!conflicts("read", &a, "exclusive", &b));
        assert!(!conflicts(
            "exclusive",
            &sc("gpu:0"),
            "exclusive",
            &sc("gpu:1")
        ));
    }
}
