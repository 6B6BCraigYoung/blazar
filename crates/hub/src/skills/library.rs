use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_runtime::McpServerSpec;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;

pub(crate) const MAX_FILE: usize = 256 * 1024;
pub(crate) const MAX_FILES: usize = 60;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

pub(crate) fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !n.starts_with('-')
}

pub(crate) fn valid_rel(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 200
        && !p.starts_with('/')
        && !p.contains('\\')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
        && !p.chars().any(char::is_control)
}

pub(crate) fn front_matter_description(md: &str) -> String {
    let Some(rest) = md.strip_prefix("---") else {
        return String::new();
    };
    let Some(end) = rest.find("\n---") else {
        return String::new();
    };
    rest[..end]
        .lines()
        .find_map(|l| l.trim().strip_prefix("description:"))
        .map(|d| d.trim().trim_matches(['"', '\'']).to_owned())
        .unwrap_or_default()
}

fn skill_template(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {}\n---\n\n# {name}\n\n什么时候用这个技能、分几步做、要注意什么，写在这里。\n",
        if description.is_empty() {
            "一句话说明什么时候该用这个技能"
        } else {
            description
        }
    )
}

pub async fn skills(State(st): State<Shared>) -> Response {
    let rows = sqlx::query(
        "SELECT s.*, (SELECT COUNT(*) FROM skill_files f WHERE f.skill_id = s.id) AS files,
                (SELECT GROUP_CONCAT(a.name, '、') FROM agent_skills x JOIN agents a ON a.id = x.agent_id WHERE x.skill_id = s.id) AS used_by
         FROM skills s ORDER BY s.name",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    Json(
        rows.iter()
            .map(|r| {
                let s = |k: &str| r.try_get::<Option<String>, _>(k).ok().flatten();
                json!({ "id": s("id"), "name": s("name"), "description": s("description"), "source": s("source"),
                        "origin": s("origin").and_then(|o| serde_json::from_str::<Value>(&o).ok()), "version": s("version"),
                        "files": r.try_get::<i64, _>("files").unwrap_or(0), "used_by": s("used_by"), "updated_at": s("updated_at") })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

pub async fn skill(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Ok(Some(r)) = sqlx::query("SELECT * FROM skills WHERE id = ?1")
        .bind(&id)
        .fetch_optional(st.db.pool())
        .await
    else {
        return fail(StatusCode::NOT_FOUND, "没有这个技能");
    };
    let files = sqlx::query("SELECT path, content FROM skill_files WHERE skill_id = ?1 ORDER BY path != 'SKILL.md', path")
        .bind(&id)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
    let s = |k: &str| r.try_get::<Option<String>, _>(k).ok().flatten();
    Json(json!({
        "id": s("id"), "name": s("name"), "description": s("description"), "source": s("source"),
        "origin": s("origin").and_then(|o| serde_json::from_str::<Value>(&o).ok()), "version": s("version"),
        "files": files.iter().map(|f| json!({
            "path": f.try_get::<String, _>("path").unwrap_or_default(),
            "content": f.try_get::<String, _>("content").unwrap_or_default(),
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct SkillBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

async fn insert_skill(
    st: &Shared,
    name: &str,
    description: &str,
    source: Option<&str>,
    files: &[(String, String)],
) -> Result<String, String> {
    let id = uuid::Uuid::now_v7().to_string();
    let ts = now();
    sqlx::query("INSERT INTO skills (id, name, description, source, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)")
        .bind(&id)
        .bind(name)
        .bind(description)
        .bind(source)
        .bind(&ts)
        .execute(st.db.pool())
        .await
        .map_err(|e| if e.to_string().contains("UNIQUE") { "已经有同名的技能了".to_owned() } else { e.to_string() })?;
    for (path, content) in files {
        let _ =
            sqlx::query("INSERT INTO skill_files (skill_id, path, content) VALUES (?1, ?2, ?3)")
                .bind(&id)
                .bind(path)
                .bind(content)
                .execute(st.db.pool())
                .await;
    }
    Ok(id)
}

pub async fn create_skill(State(st): State<Shared>, Json(b): Json<SkillBody>) -> Response {
    let name = b.name.trim();
    if !valid_name(name) {
        return fail(
            StatusCode::BAD_REQUEST,
            "技能名只能用字母、数字、- 和 _（它会成为目录名）",
        );
    }
    let files = [(
        "SKILL.md".to_owned(),
        skill_template(name, b.description.trim()),
    )];
    match insert_skill(&st, name, b.description.trim(), None, &files).await {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}

pub async fn delete_skill(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let _ = sqlx::query("DELETE FROM skills WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub struct FileBody {
    pub path: String,
    pub content: String,
}

pub async fn put_file(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<FileBody>,
) -> Response {
    let path = b.path.trim();
    if !valid_rel(path) {
        return fail(
            StatusCode::BAD_REQUEST,
            "文件路径只能是技能目录里的相对路径",
        );
    }
    if b.content.len() > MAX_FILE {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "单个文件上限 256KB");
    }
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM skill_files WHERE skill_id = ?1 AND path != ?2")
            .bind(&id)
            .bind(path)
            .fetch_one(st.db.pool())
            .await
            .unwrap_or(0);
    if n as usize >= MAX_FILES {
        return fail(StatusCode::BAD_REQUEST, "一个技能最多 60 个文件");
    }
    let r = sqlx::query(
        "INSERT INTO skill_files (skill_id, path, content) VALUES (?1, ?2, ?3)
         ON CONFLICT (skill_id, path) DO UPDATE SET content = excluded.content",
    )
    .bind(&id)
    .bind(path)
    .bind(&b.content)
    .execute(st.db.pool())
    .await;
    if let Err(e) = r {
        return fail(StatusCode::CONFLICT, e.to_string());
    }

    if path == "SKILL.md" {
        let d = front_matter_description(&b.content);
        let _ = sqlx::query("UPDATE skills SET description = CASE WHEN ?2 = '' THEN description ELSE ?2 END, updated_at = ?3 WHERE id = ?1")
            .bind(&id)
            .bind(d)
            .bind(now())
            .execute(st.db.pool())
            .await;
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub struct FileQuery {
    pub path: String,
}

pub async fn delete_file(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<FileQuery>,
) -> Response {
    if q.path == "SKILL.md" {
        return fail(StatusCode::CONFLICT, "SKILL.md 是技能的入口，不能删");
    }
    let _ = sqlx::query("DELETE FROM skill_files WHERE skill_id = ?1 AND path = ?2")
        .bind(&id)
        .bind(&q.path)
        .execute(st.db.pool())
        .await;
    StatusCode::NO_CONTENT.into_response()
}

fn skill_roots() -> Vec<(String, std::path::PathBuf)> {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return Vec::new();
    };
    vec![
        ("~/.claude/skills".to_owned(), home.join(".claude/skills")),
        ("~/.codex/skills".to_owned(), home.join(".codex/skills")),
    ]
}

fn read_skill_dir(dir: &std::path::Path) -> Vec<(String, String)> {
    fn walk(
        base: &std::path::Path,
        dir: &std::path::Path,
        out: &mut Vec<(String, String)>,
        depth: u8,
    ) {
        if depth > 4 || out.len() >= MAX_FILES {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "__pycache__" {
                continue;
            }

            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.is_dir() {
                walk(base, &p, out, depth + 1);
            } else if meta.is_file()
                && meta.len() as usize <= MAX_FILE
                && out.len() < MAX_FILES
                && let (Ok(text), Ok(rel)) = (std::fs::read_to_string(&p), p.strip_prefix(base))
            {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if valid_rel(&rel) {
                    out.push((rel, text));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out, 0);
    out
}

pub async fn import_scan(State(st): State<Shared>) -> Response {
    let existing: Vec<String> = sqlx::query_scalar("SELECT LOWER(name) FROM skills")
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
    let found = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        for (label, root) in skill_roots() {
            let Ok(rd) = std::fs::read_dir(&root) else {
                continue;
            };
            for e in rd.filter_map(Result::ok) {
                let dir = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                let Ok(md) = std::fs::read_to_string(dir.join("SKILL.md")) else {
                    continue;
                };
                if !valid_name(&name) {
                    continue;
                }
                out.push(json!({
                    "source": format!("{label}/{name}"), "name": name,
                    "description": front_matter_description(&md),
                    "exists": existing.contains(&name.to_lowercase()),
                }));
            }
        }
        out
    })
    .await
    .unwrap_or_default();
    Json(found).into_response()
}

#[derive(Deserialize)]
pub struct ImportBody {
    pub sources: Vec<String>,

    #[serde(default)]
    pub conflict: String,
}

pub async fn import(State(st): State<Shared>, Json(b): Json<ImportBody>) -> Response {
    let roots = skill_roots();
    let mut report = Vec::new();
    for src in b.sources.iter().take(100) {
        let Some((label, name)) = src.rsplit_once('/') else {
            continue;
        };
        let Some((_, root)) = roots.iter().find(|(l, _)| l == label) else {
            continue;
        };
        if !valid_name(name) {
            continue;
        }
        let dir = root.join(name);
        let files = tokio::task::spawn_blocking(move || read_skill_dir(&dir))
            .await
            .unwrap_or_default();
        if !files.iter().any(|(p, _)| p == "SKILL.md") {
            report.push(json!({ "source": src, "result": "没有 SKILL.md，跳过" }));
            continue;
        }
        let desc = files
            .iter()
            .find(|(p, _)| p == "SKILL.md")
            .map(|(_, c)| front_matter_description(c))
            .unwrap_or_default();
        let existing: Option<String> = sqlx::query_scalar("SELECT id FROM skills WHERE name = ?1")
            .bind(name)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten();
        let mut final_name = name.to_owned();
        if let Some(old) = existing {
            match b.conflict.as_str() {
                "overwrite" => {
                    let _ = sqlx::query("DELETE FROM skills WHERE id = ?1")
                        .bind(&old)
                        .execute(st.db.pool())
                        .await;
                }
                "rename" => {
                    let mut i = 2;
                    loop {
                        let cand = format!("{name}-{i}");
                        let taken: i64 =
                            sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE name = ?1")
                                .bind(&cand)
                                .fetch_one(st.db.pool())
                                .await
                                .unwrap_or(0);
                        if taken == 0 {
                            final_name = cand;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    report.push(json!({ "source": src, "result": "已存在，跳过" }));
                    continue;
                }
            }
        }
        match insert_skill(&st, &final_name, &desc, Some(src), &files).await {
            Ok(_) => report.push(json!({ "source": src, "result": format!("已导入为 {final_name}（{} 个文件）", files.len()) })),
            Err(e) => report.push(json!({ "source": src, "result": e })),
        }
    }
    Json(report).into_response()
}

fn keys_of(raw: &str) -> Vec<String> {
    serde_json::from_str::<BTreeMap<String, String>>(raw)
        .map(|m| m.into_keys().collect())
        .unwrap_or_default()
}

fn mcp_view(r: &sqlx::sqlite::SqliteRow) -> Value {
    let s = |k: &str| r.try_get::<String, _>(k).unwrap_or_default();
    json!({
        "id": s("id"), "name": s("name"), "description": s("description"), "transport": s("transport"),
        "command": s("command"), "args": serde_json::from_str::<Value>(&s("args")).unwrap_or_else(|_| json!([])),
        "url": s("url"),

        "env_keys": keys_of(&s("env")), "header_keys": keys_of(&s("headers")),
        "used_by": r.try_get::<Option<String>, _>("used_by").ok().flatten(),
        "updated_at": s("updated_at"),
    })
}

pub async fn mcp_list(State(st): State<Shared>) -> Response {
    let rows = sqlx::query(
        "SELECT m.*, (SELECT GROUP_CONCAT(a.name, '、') FROM agent_mcp x JOIN agents a ON a.id = x.agent_id WHERE x.server_id = m.id) AS used_by
         FROM mcp_servers m ORDER BY m.name",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    Json(rows.iter().map(mcp_view).collect::<Vec<_>>()).into_response()
}

#[derive(Deserialize)]
pub struct McpBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub transport: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: String,

    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
}

fn check_mcp(b: &McpBody) -> Result<(), String> {
    if !valid_name(b.name.trim()) {
        return Err("名字只能用字母、数字、- 和 _（它会成为配置键）".into());
    }
    match b.transport.as_str() {
        "stdio" if b.command.trim().is_empty() => return Err("STDIO 类型要填启动命令".into()),
        "http" if !(b.url.starts_with("https://") || b.url.starts_with("http://")) => {
            return Err("HTTP 类型要填 http(s):// 开头的地址".into());
        }
        "stdio" | "http" => {}
        _ => return Err("类型只能是 stdio 或 http".into()),
    }
    for m in [&b.env, &b.headers].into_iter().flatten() {
        for k in m.keys() {
            if k.is_empty()
                || k.len() > 100
                || k.chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '=')
            {
                return Err(format!("键名不合法：{k}"));
            }
        }
    }
    Ok(())
}

pub async fn mcp_create(State(st): State<Shared>, Json(b): Json<McpBody>) -> Response {
    if let Err(e) = check_mcp(&b) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    let id = uuid::Uuid::now_v7().to_string();
    let r = sqlx::query(
        "INSERT INTO mcp_servers (id, name, description, transport, command, args, url, env, headers, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
    )
    .bind(&id)
    .bind(b.name.trim())
    .bind(b.description.trim())
    .bind(&b.transport)
    .bind(b.command.trim())
    .bind(serde_json::to_string(&b.args).unwrap_or_else(|_| "[]".into()))
    .bind(b.url.trim())
    .bind(serde_json::to_string(&b.env.unwrap_or_default()).unwrap_or_else(|_| "{}".into()))
    .bind(serde_json::to_string(&b.headers.unwrap_or_default()).unwrap_or_else(|_| "{}".into()))
    .bind(now())
    .execute(st.db.pool())
    .await;
    match r {
        Ok(_) => Json(json!({ "id": id })).into_response(),
        Err(e) if e.to_string().contains("UNIQUE") => {
            fail(StatusCode::CONFLICT, "已经有同名的 MCP server 了")
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn mcp_update(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<McpBody>,
) -> Response {
    if let Err(e) = check_mcp(&b) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    let r = sqlx::query(
        "UPDATE mcp_servers SET name = ?2, description = ?3, transport = ?4, command = ?5, args = ?6, url = ?7,
                env = COALESCE(?8, env), headers = COALESCE(?9, headers), updated_at = ?10 WHERE id = ?1",
    )
    .bind(&id)
    .bind(b.name.trim())
    .bind(b.description.trim())
    .bind(&b.transport)
    .bind(b.command.trim())
    .bind(serde_json::to_string(&b.args).unwrap_or_else(|_| "[]".into()))
    .bind(b.url.trim())
    .bind(b.env.map(|m| serde_json::to_string(&m).unwrap_or_else(|_| "{}".into())))
    .bind(b.headers.map(|m| serde_json::to_string(&m).unwrap_or_else(|_| "{}".into())))
    .bind(now())
    .execute(st.db.pool())
    .await;
    match r {
        Ok(r) if r.rows_affected() == 0 => fail(StatusCode::NOT_FOUND, "没有这个 MCP server"),
        Ok(_) => Json(json!({ "ok": true })).into_response(),
        Err(e) if e.to_string().contains("UNIQUE") => {
            fail(StatusCode::CONFLICT, "已经有同名的 MCP server 了")
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub async fn mcp_delete(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let _ = sqlx::query("DELETE FROM mcp_servers WHERE id = ?1")
        .bind(&id)
        .execute(st.db.pool())
        .await;
    StatusCode::NO_CONTENT.into_response()
}

pub async fn agent_caps(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let skills: Vec<String> =
        sqlx::query_scalar("SELECT skill_id FROM agent_skills WHERE agent_id = ?1")
            .bind(&id)
            .fetch_all(st.db.pool())
            .await
            .unwrap_or_default();
    let mcp: Vec<String> =
        sqlx::query_scalar("SELECT server_id FROM agent_mcp WHERE agent_id = ?1")
            .bind(&id)
            .fetch_all(st.db.pool())
            .await
            .unwrap_or_default();
    Json(json!({ "skills": skills, "mcp": mcp })).into_response()
}

#[derive(Deserialize)]
pub struct CapsBody {
    #[serde(default)]
    pub skills: Option<Vec<String>>,
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
}

pub async fn set_agent_caps(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<CapsBody>,
) -> Response {
    if let Some(skills) = b.skills {
        let _ = sqlx::query("DELETE FROM agent_skills WHERE agent_id = ?1")
            .bind(&id)
            .execute(st.db.pool())
            .await;
        for s in skills.iter().take(50) {
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO agent_skills (agent_id, skill_id) VALUES (?1, ?2)",
            )
            .bind(&id)
            .bind(s)
            .execute(st.db.pool())
            .await;
        }
    }
    if let Some(mcp) = b.mcp {
        let _ = sqlx::query("DELETE FROM agent_mcp WHERE agent_id = ?1")
            .bind(&id)
            .execute(st.db.pool())
            .await;
        for m in mcp.iter().take(50) {
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO agent_mcp (agent_id, server_id) VALUES (?1, ?2)",
            )
            .bind(&id)
            .bind(m)
            .execute(st.db.pool())
            .await;
        }
    }
    agent_caps(State(st), Path(id)).await
}

pub struct Caps {
    pub mcp: Vec<McpServerSpec>,

    pub env: BTreeMap<String, String>,

    pub skill_root: Option<String>,

    pub skills: Vec<(String, String, String)>,
}

fn env_var_name(server: &str, key: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    format!("BLAZAR_MCP_{}_{}", clean(server), clean(key))
}

pub async fn for_agent(st: &Shared, agent: &str) -> Caps {
    let mut caps = Caps {
        mcp: Vec::new(),
        env: BTreeMap::new(),
        skill_root: None,
        skills: Vec::new(),
    };
    let rows = sqlx::query("SELECT m.* FROM mcp_servers m JOIN agent_mcp x ON x.server_id = m.id WHERE x.agent_id = ?1 ORDER BY m.name")
        .bind(agent)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
    for r in &rows {
        let s = |k: &str| r.try_get::<String, _>(k).unwrap_or_default();
        let name = s("name");
        let mut indirect = |raw: String| -> BTreeMap<String, String> {
            let m: BTreeMap<String, String> = serde_json::from_str(&raw).unwrap_or_default();
            m.into_iter()
                .map(|(k, v)| {
                    let var = env_var_name(&name, &k);
                    caps.env.insert(var.clone(), v);
                    (k, format!("${{{var}}}"))
                })
                .collect()
        };
        let env = indirect(s("env"));
        let headers = indirect(s("headers"));
        caps.mcp.push(McpServerSpec {
            name: name.clone(),
            http: s("transport") == "http",
            command: s("command"),
            args: serde_json::from_str(&s("args")).unwrap_or_default(),
            url: s("url"),
            env,
            headers,
        });
    }

    let skills = sqlx::query("SELECT s.id, s.name, s.description FROM skills s JOIN agent_skills x ON x.skill_id = s.id WHERE x.agent_id = ?1 ORDER BY s.name")
        .bind(agent)
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
    if skills.is_empty() {
        return caps;
    }
    let safe_agent: String = agent
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let root = std::env::temp_dir().join("blazar-skills").join(&safe_agent);
    let base = root.join(".claude").join("skills");

    let _ = tokio::fs::remove_dir_all(&base).await;
    for sk in &skills {
        let (id, name, desc): (String, String, String) = (
            sk.try_get("id").unwrap_or_default(),
            sk.try_get("name").unwrap_or_default(),
            sk.try_get("description").unwrap_or_default(),
        );
        if !valid_name(&name) {
            continue;
        }
        let dir = base.join(&name);
        let files = sqlx::query("SELECT path, content FROM skill_files WHERE skill_id = ?1")
            .bind(&id)
            .fetch_all(st.db.pool())
            .await
            .unwrap_or_default();
        for f in &files {
            let (path, content): (String, String) = (
                f.try_get("path").unwrap_or_default(),
                f.try_get("content").unwrap_or_default(),
            );
            if !valid_rel(&path) {
                continue;
            }
            let target = dir.join(&path);
            if let Some(parent) = target.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&target, content).await;
        }
        caps.skills
            .push((name, desc, dir.join("SKILL.md").display().to_string()));
    }
    caps.skill_root = Some(root.display().to_string());
    caps
}

#[must_use]
pub fn skills_briefing(skills: &[(String, String, String)]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "你有下面这些技能可用。遇到对得上的任务，先读对应的 SKILL.md 再按它说的做：\n",
    );
    for (name, desc, path) in skills {
        out.push_str(&format!("- {name}：{desc}（{path}）\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths_cannot_escape() {
        for ok in ["pdf", "code-review", "my_skill2"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-x", "a b", "a/b", "..", "名字", "a.b"] {
            assert!(!valid_name(bad), "{bad}");
        }
        for ok in ["SKILL.md", "scripts/run.py", "ref/a/b.md"] {
            assert!(valid_rel(ok), "{ok}");
        }
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "./a", "a\\b"] {
            assert!(!valid_rel(bad), "{bad}");
        }
    }

    #[test]
    fn description_comes_from_front_matter() {
        let md = "---\nname: pdf\ndescription: \"Fill and read PDF forms\"\n---\n# PDF\n";
        assert_eq!(front_matter_description(md), "Fill and read PDF forms");
        assert_eq!(front_matter_description("# no front matter"), "");
        assert!(skill_template("x", "").contains("name: x"));
    }

    #[test]
    fn secrets_become_env_indirections() {
        assert_eq!(
            env_var_name("git-hub", "api.key"),
            "BLAZAR_MCP_GIT_HUB_API_KEY"
        );
    }

    #[test]
    fn mcp_servers_need_the_right_fields() {
        let b = |t: &str, cmd: &str, url: &str| McpBody {
            name: "srv".into(),
            description: String::new(),
            transport: t.into(),
            command: cmd.into(),
            args: vec![],
            url: url.into(),
            env: None,
            headers: None,
        };
        assert!(check_mcp(&b("stdio", "npx", "")).is_ok());
        assert!(check_mcp(&b("stdio", "", "")).is_err());
        assert!(check_mcp(&b("http", "", "https://x/mcp")).is_ok());
        assert!(check_mcp(&b("http", "", "ftp://x")).is_err());
        assert!(check_mcp(&b("sse", "", "")).is_err());
        let mut bad = b("stdio", "npx", "");
        bad.env = Some(BTreeMap::from([("A B".to_owned(), "v".to_owned())]));
        assert!(check_mcp(&bad).is_err());
    }
}
