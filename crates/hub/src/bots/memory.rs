use std::collections::HashSet;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{Caller, opt, s};
use crate::api::Shared;

pub const TYPES: &[&str] = &["user", "feedback", "project", "reference"];
pub const SIMILAR: f64 = 0.6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub scope: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    pub version: i64,
    pub updated_by: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

#[derive(Debug)]
pub enum WriteError {
    Invalid(String),
    Conflict {
        reason: String,
        existing: Box<Memory>,
    },
}

fn memory_of(r: &SqliteRow, with_body: bool) -> Memory {
    Memory {
        id: s(r, "id"),
        scope: s(r, "scope"),
        name: s(r, "name"),
        description: s(r, "description"),
        kind: s(r, "type"),
        body: if with_body {
            s(r, "body")
        } else {
            String::new()
        },
        version: r.try_get("version").unwrap_or(1),
        updated_by: s(r, "updated_by"),
        updated_at: s(r, "updated_at"),
        archived_at: opt(r, "archived_at"),
    }
}

#[must_use]
pub fn tokens(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut word = String::new();
    let mut prev_cjk: Option<char> = None;
    let flush = |word: &mut String, out: &mut HashSet<String>| {
        if word.chars().count() >= 2 {
            out.insert(word.clone());
        }
        word.clear();
    };
    for c in text.chars() {
        let cjk = ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{f900}'..='\u{faff}').contains(&c);
        if cjk {
            flush(&mut word, &mut out);
            out.insert(c.to_string());
            if let Some(p) = prev_cjk {
                out.insert(format!("{p}{c}"));
            }
            prev_cjk = Some(c);
        } else if c.is_alphanumeric() {
            word.push(c.to_ascii_lowercase());
            prev_cjk = None;
        } else {
            flush(&mut word, &mut out);
            prev_cjk = None;
        }
    }
    flush(&mut word, &mut out);
    out
}

#[must_use]
pub fn similarity(a: &str, b: &str) -> f64 {
    let (ta, tb) = (tokens(a), tokens(b));
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    inter / union
}

#[must_use]
pub fn valid_name(n: &str) -> bool {
    (1..=80).contains(&n.len())
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !n.starts_with('-')
        && !n.ends_with('-')
}

fn scope_allowed(scope: &str, by: Option<&Caller>) -> Result<(), String> {
    if scope == "shared" {
        return Ok(());
    }
    let Some(id) = scope.strip_prefix("bot:") else {
        return Err("Scope must be shared or bot:<id>".into());
    };
    match by {
        Some(c) if c.bot != id && !c.is_lead() => {
            Err("You can only write your own private memory".into())
        }
        _ => Ok(()),
    }
}

pub async fn list(st: &Shared, scope: Option<&str>, by: Option<&Caller>) -> Vec<Memory> {
    let scopes: Vec<String> = match (scope, by) {
        (Some(sc), _) => vec![sc.to_owned()],
        (None, Some(c)) => vec!["shared".into(), format!("bot:{}", c.bot)],
        (None, None) => Vec::new(),
    };
    let rows = if scopes.is_empty() {
        sqlx::query("SELECT * FROM memories WHERE archived_at IS NULL ORDER BY scope, name")
            .fetch_all(st.db.pool())
            .await
    } else {
        sqlx::query(
            "SELECT * FROM memories WHERE archived_at IS NULL AND scope IN (?1, ?2) ORDER BY scope, name",
        )
        .bind(&scopes[0])
        .bind(scopes.get(1).cloned().unwrap_or_default())
        .fetch_all(st.db.pool())
        .await
    };
    rows.unwrap_or_default()
        .iter()
        .map(|r| memory_of(r, false))
        .collect()
}

pub async fn read(st: &Shared, id_or_name: &str, by: Option<&Caller>) -> Option<Memory> {
    let m = sqlx::query(
        "SELECT * FROM memories WHERE id = ?1 OR (scope = 'shared' AND name = ?1) LIMIT 1",
    )
    .bind(id_or_name)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|r| memory_of(&r, true));
    let m = match (m, by) {
        (Some(m), _) => Some(m),
        (None, Some(c)) => sqlx::query("SELECT * FROM memories WHERE scope = ?1 AND name = ?2")
            .bind(format!("bot:{}", c.bot))
            .bind(id_or_name)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .map(|r| memory_of(&r, true)),
        (None, None) => None,
    }?;
    match by {
        Some(c) if m.scope != "shared" && m.scope != format!("bot:{}", c.bot) && !c.is_lead() => {
            None
        }
        _ => Some(m),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Write {
    #[serde(default)]
    pub scope: Option<String>,
    pub name: String,
    pub description: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub body: String,
    #[serde(default)]
    pub version: Option<i64>,
}

pub async fn write(st: &Shared, w: Write, by: Option<&Caller>) -> Result<Memory, WriteError> {
    let name = w.name.trim().to_owned();
    if !valid_name(&name) {
        return Err(WriteError::Invalid(
            "Name must be kebab-case: lowercase letters, digits, hyphens, up to 80 characters"
                .into(),
        ));
    }
    let description = w.description.trim().to_owned();
    if description.is_empty() || description.chars().count() > 300 {
        return Err(WriteError::Invalid(
            "Description must be one sentence, 1–300 characters".into(),
        ));
    }
    if w.body.trim().is_empty() || w.body.chars().count() > 20_000 {
        return Err(WriteError::Invalid(
            "Body must be 1–20000 characters".into(),
        ));
    }
    let kind = w.kind.unwrap_or_else(|| "project".into());
    if !TYPES.contains(&kind.as_str()) {
        return Err(WriteError::Invalid(format!(
            "Type must be one of {}",
            TYPES.join(" / ")
        )));
    }
    let scope = w.scope.unwrap_or_else(|| "shared".into());
    scope_allowed(&scope, by).map_err(WriteError::Invalid)?;
    let updated_by = by.map_or_else(|| "user".to_owned(), |c| format!("bot:{}", c.handle));
    let now = Utc::now().to_rfc3339();
    let existing = sqlx::query("SELECT * FROM memories WHERE scope = ?1 AND name = ?2")
        .bind(&scope)
        .bind(&name)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten()
        .map(|r| memory_of(&r, true));
    if let Some(e) = existing {
        if w.version != Some(e.version) {
            return Err(WriteError::Conflict {
                reason: format!(
                    "{scope}/{name} already exists (version {}). Pass version={} to update it, or pick another name for a new entry",
                    e.version, e.version
                ),
                existing: Box::new(e),
            });
        }
        sqlx::query(
            "UPDATE memories SET description = ?3, type = ?4, body = ?5, version = version + 1,
                    updated_by = ?6, updated_at = ?7, archived_at = NULL
             WHERE scope = ?1 AND name = ?2 AND version = ?8",
        )
        .bind(&scope)
        .bind(&name)
        .bind(&description)
        .bind(&kind)
        .bind(&w.body)
        .bind(&updated_by)
        .bind(&now)
        .bind(e.version)
        .execute(st.db.pool())
        .await
        .map_err(|err| WriteError::Invalid(err.to_string()))?;
        super::changed(st);
        return read(st, &e.id, None)
            .await
            .ok_or_else(|| WriteError::Invalid("Couldn't read the memory back".into()));
    }
    let candidates = list(st, None, None).await;
    for c in candidates {
        let visible = c.scope == "shared" || c.scope == scope;
        if !visible {
            continue;
        }
        let same_name = c.name == name;
        let close = similarity(&c.description, &description) >= SIMILAR;
        if same_name || close {
            let full = read(st, &c.id, None).await.unwrap_or(c);
            return Err(WriteError::Conflict {
                reason: format!(
                    "A very similar memory already exists: {}/{} ({}). Update it (with version={}) instead of adding another",
                    full.scope, full.name, full.description, full.version
                ),
                existing: Box::new(full),
            });
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO memories (id, scope, name, description, type, body, version, updated_by, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)",
    )
    .bind(&id)
    .bind(&scope)
    .bind(&name)
    .bind(&description)
    .bind(&kind)
    .bind(&w.body)
    .bind(&updated_by)
    .bind(&now)
    .execute(st.db.pool())
    .await
    .map_err(|err| WriteError::Invalid(err.to_string()))?;
    super::changed(st);
    read(st, &id, None)
        .await
        .ok_or_else(|| WriteError::Invalid("Couldn't read the memory back".into()))
}

pub async fn archive(st: &Shared, id: &str, by: Option<&Caller>) -> Result<(), String> {
    let m = read(st, id, by).await.ok_or("No such memory")?;
    scope_allowed(&m.scope, by)?;
    sqlx::query("UPDATE memories SET archived_at = ?2 WHERE id = ?1")
        .bind(&m.id)
        .bind(Utc::now().to_rfc3339())
        .execute(st.db.pool())
        .await
        .map_err(|e| e.to_string())?;
    super::changed(st);
    Ok(())
}

pub async fn index(st: &Shared, bot_id: &str) -> String {
    let mut out = String::new();
    for m in sqlx::query(
        "SELECT * FROM memories WHERE archived_at IS NULL AND scope IN ('shared', ?1) ORDER BY scope, name LIMIT 200",
    )
    .bind(format!("bot:{bot_id}"))
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| memory_of(r, false))
    {
        out.push_str(&format!("- {}/{} — {}\n", m.scope, m.name, m.description));
    }
    out
}

#[must_use]
pub fn conflict_json(reason: &str, existing: &Memory) -> Value {
    json!({ "error": reason, "duplicate": true, "existing": existing })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_and_latin_tokens_compare_sensibly() {
        assert!(similarity("训练队列 双节点", "双节点训练队列") > 0.6);
        assert!(
            similarity(
                "gpu keepalive script",
                "GPU keepalive script for the cluster"
            ) >= 0.5
        );
        assert!(similarity("真机评测只靠模型", "仿真数据生成调度器") < 0.2);
        assert_eq!(similarity("", "x"), 0.0);
    }

    #[test]
    fn names_are_kebab_case() {
        assert!(valid_name("train-queue"));
        assert!(!valid_name("Train"));
        assert!(!valid_name("-x"));
        assert!(!valid_name(""));
    }
}
