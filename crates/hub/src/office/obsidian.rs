use std::path::{Path as FsPath, PathBuf};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;
use crate::office::{kv_get, kv_put};

const MAX_NOTE: usize = 512 * 1024;

const COUNT_CAP: usize = 20_000;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn app_installed() -> bool {
    if cfg!(target_os = "macos") {
        return FsPath::new("/Applications/Obsidian.app").is_dir()
            || std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join("Applications/Obsidian.app").is_dir())
                .unwrap_or(false);
    }
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("obsidian").is_file()))
}

fn registry_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/obsidian/obsidian.json"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("obsidian/obsidian.json"))
    } else {
        Some(home.join(".config/obsidian/obsidian.json"))
    }
}

pub(crate) fn vault_paths() -> Vec<PathBuf> {
    let Some(p) = registry_path() else {
        return vec![];
    };
    let Ok(text) = std::fs::read_to_string(p) else {
        return vec![];
    };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let mut out: Vec<PathBuf> = v["vaults"]
        .as_object()
        .map(|m| {
            m.values()
                .filter_map(|e| e["path"].as_str())
                .map(|s| PathBuf::from(s.trim_end_matches('/')))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

fn vault_name(p: &FsPath) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().trim().to_owned())
        .unwrap_or_default()
}

fn count(p: &FsPath) -> (usize, usize) {
    let mut notes = 0;
    let mut files = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files += 1;
                if path.extension().is_some_and(|x| x == "md") {
                    notes += 1;
                }
            }
            if files >= COUNT_CAP {
                return (notes, files);
            }
        }
    }
    (notes, files)
}

async fn prefs(st: &Shared) -> Value {
    let s = kv_get(st, "office.obsidian").await;
    json!({
        "vault": s["vault"].as_str().unwrap_or(""),
        "folder": s["folder"].as_str().unwrap_or("Blazar"),
    })
}

pub async fn status(State(st): State<Shared>) -> Response {
    let installed = app_installed();
    let paths = vault_paths();

    let rows = sqlx::query(
        "SELECT w.id, w.path FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE n.name = 'local'",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let ws_of = |p: &FsPath| -> Option<String> {
        let want = p.display().to_string();
        rows.iter().find_map(|r| {
            let path: String = r.try_get("path").ok()?;
            (path.trim_end_matches('/') == want).then(|| r.try_get::<String, _>("id").ok())?
        })
    };
    let vaults: Vec<Value> = paths
        .iter()
        .map(|p| {
            let exists = p.is_dir();
            let (notes, files) = if exists { count(p) } else { (0, 0) };
            json!({
                "name": vault_name(p),
                "path": p.display().to_string(),
                "exists": exists,
                "notes": notes,
                "files": files,
                "workspace_id": ws_of(p),
            })
        })
        .collect();
    Json(json!({
        "installed": installed,
        "vaults": vaults,
        "prefs": prefs(&st).await,
    }))
    .into_response()
}

pub async fn put_settings(State(st): State<Shared>, Json(b): Json<Value>) -> Response {
    let mut cur = prefs(&st).await;
    if let Some(v) = b["vault"].as_str() {
        let v = v.trim_end_matches('/');
        if !v.is_empty() && !vault_paths().iter().any(|p| p.display().to_string() == v) {
            return fail(StatusCode::BAD_REQUEST, "这不是 Obsidian 认识的库");
        }
        cur["vault"] = json!(v);
    }
    if let Some(f) = b["folder"].as_str() {
        let f = f.trim().trim_matches('/');
        if !valid_folder(f) {
            return fail(
                StatusCode::BAD_REQUEST,
                "文件夹名不能含 .. 或 \\，可以用 / 分层",
            );
        }
        cur["folder"] = json!(f);
    }
    match kv_put(&st, "office.obsidian", &cur).await {
        Ok(()) => Json(cur).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

fn valid_folder(f: &str) -> bool {
    f.len() <= 200
        && !f.contains('\\')
        && !f.contains('\0')
        && f.split('/')
            .all(|s| !s.is_empty() && s != ".." && s != "." && !s.starts_with('.'))
}

pub(crate) fn note_file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']'
            ) || c.is_control()
            {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let cleaned = cleaned.trim_matches(['.', ' ']);
    let mut name: String = cleaned.chars().take(120).collect();
    if name.is_empty() {
        name = "未命名".into();
    }
    format!("{name}.md")
}

#[derive(Deserialize)]
pub struct NoteBody {
    pub title: String,
    pub content: String,

    #[serde(default)]
    pub vault: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
}

struct Written {
    vault: PathBuf,
    rel: String,
}

async fn write_note(st: &Shared, b: &NoteBody) -> Result<Written, Response> {
    let p = prefs(st).await;
    let vault = b
        .vault
        .as_deref()
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| p["vault"].as_str().unwrap_or("").to_owned());
    let vault = vault.trim_end_matches('/').to_owned();
    if vault.is_empty() {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "还没选默认的库：到「办公」→ Obsidian 里选一个",
        ));
    }
    let vault_path = PathBuf::from(&vault);
    if !vault_paths().contains(&vault_path) {
        return Err(fail(StatusCode::BAD_REQUEST, "这不是 Obsidian 认识的库"));
    }
    if !vault_path.is_dir() {
        return Err(fail(StatusCode::BAD_REQUEST, "这个库的文件夹不存在"));
    }
    let folder = b
        .folder
        .as_deref()
        .map(|f| f.trim().trim_matches('/').to_owned())
        .unwrap_or_else(|| p["folder"].as_str().unwrap_or("").to_owned());
    if !folder.is_empty() && !valid_folder(&folder) {
        return Err(fail(StatusCode::BAD_REQUEST, "文件夹名不合法"));
    }
    if b.content.len() > MAX_NOTE {
        return Err(fail(StatusCode::BAD_REQUEST, "笔记太大（上限 512KB）"));
    }
    let file = note_file_name(&b.title);
    let rel = if folder.is_empty() {
        file
    } else {
        format!("{folder}/{file}")
    };
    let dir = if folder.is_empty() {
        vault_path.clone()
    } else {
        vault_path.join(&folder)
    };
    let target = dir.join(rel.rsplit('/').next().unwrap_or(&rel));

    if !target.starts_with(&vault_path) {
        return Err(fail(StatusCode::BAD_REQUEST, "路径不在库内"));
    }
    let content = b.content.clone();
    let overwrite = b.overwrite;
    let res = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        if !overwrite && target.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "已经有同名笔记",
            ));
        }
        std::fs::write(&target, content)
    })
    .await;
    match res {
        Ok(Ok(())) => Ok(Written {
            vault: vault_path,
            rel,
        }),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(fail(
            StatusCode::CONFLICT,
            "已经有同名笔记；换个标题，或者选择覆盖",
        )),
        Ok(Err(e)) => Err(fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("写不进去：{e}"),
        )),
        Err(e) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

pub(crate) fn open_uri(vault: &FsPath, rel: &str) -> String {
    let enc = |s: &str| {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    out.push(b as char);
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    };
    format!(
        "obsidian://open?vault={}&file={}",
        enc(&vault_name(vault)),
        enc(rel.trim_end_matches(".md"))
    )
}

fn written_json(w: &Written) -> Value {
    json!({ "ok": true, "vault": w.vault.display().to_string(), "path": w.rel, "open": open_uri(&w.vault, &w.rel) })
}

pub async fn note(State(st): State<Shared>, Json(b): Json<NoteBody>) -> Response {
    match write_note(&st, &b).await {
        Ok(w) => Json(written_json(&w)).into_response(),
        Err(r) => r,
    }
}

#[derive(Deserialize, Default)]
pub struct TaskNoteBody {
    #[serde(default)]
    pub overwrite: bool,
}

pub async fn task_note(
    State(st): State<Shared>,
    Path(id): Path<String>,
    body: Option<Json<TaskNoteBody>>,
) -> Response {
    let overwrite = body.is_some_and(|b| b.overwrite);
    let Ok(Some(t)) = sqlx::query(
        "SELECT number, title, description, status, created_at FROM tasks WHERE id = ?1",
    )
    .bind(&id)
    .fetch_optional(st.db.pool())
    .await
    else {
        return fail(StatusCode::NOT_FOUND, "没有这个任务");
    };
    let title: String = t.try_get("title").unwrap_or_default();
    let number: i64 = t.try_get("number").unwrap_or(0);
    let comments = sqlx::query(
        "SELECT author, body, created_at FROM task_comments WHERE task_id = ?1 ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let mut md = format!(
        "---\nsource: blazar\ntask: BLZ-{number}\nstatus: {}\ncreated: {}\ntags: [blazar]\n---\n\n# {title}\n\n{}\n",
        t.try_get::<String, _>("status").unwrap_or_default(),
        t.try_get::<String, _>("created_at")
            .unwrap_or_default()
            .chars()
            .take(10)
            .collect::<String>(),
        t.try_get::<String, _>("description").unwrap_or_default()
    );
    for c in &comments {
        let who = match c
            .try_get::<String, _>("author")
            .unwrap_or_default()
            .as_str()
        {
            "agent" => "智能体",
            "user" => "我",
            _ => "系统",
        };
        md.push_str(&format!(
            "\n## {who} · {}\n\n{}\n",
            c.try_get::<String, _>("created_at")
                .unwrap_or_default()
                .chars()
                .take(16)
                .collect::<String>()
                .replace('T', " "),
            c.try_get::<String, _>("body").unwrap_or_default()
        ));
    }
    let b = NoteBody {
        title: format!("BLZ-{number} {title}"),
        content: md,
        vault: None,
        folder: None,
        overwrite,
    };
    match write_note(&st, &b).await {
        Ok(w) => Json(written_json(&w)).into_response(),
        Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_file_names_are_safe_and_readable() {
        assert_eq!(
            note_file_name("BLZ-12 修复：登录/注册 页面"),
            "BLZ-12 修复：登录 注册 页面.md"
        );
        assert_eq!(
            note_file_name("a:b*c?d\"e<f>g|h#i^j[k]l"),
            "a b c d e f g h i j k l.md"
        );
        assert_eq!(note_file_name("   "), "未命名.md");
        assert_eq!(note_file_name("..."), "未命名.md");
        assert_eq!(note_file_name("../../etc/passwd"), "etc passwd.md");
        assert_eq!(note_file_name(&"x".repeat(300)).len(), 123);
    }

    #[test]
    fn folders_stay_inside_the_vault() {
        assert!(valid_folder("Blazar"));
        assert!(valid_folder("项目/Blazar/2026"));
        assert!(!valid_folder("../secrets"));
        assert!(!valid_folder("a/../b"));
        assert!(!valid_folder(".obsidian"));
        assert!(!valid_folder("a\\b"));
        assert!(!valid_folder("a//b"));
    }

    #[test]
    fn open_uri_encodes_vault_and_file() {
        let u = open_uri(
            FsPath::new("/Users/me/Documents/My Vault"),
            "Blazar/BLZ-1 标题.md",
        );
        assert_eq!(
            u,
            "obsidian://open?vault=My%20Vault&file=Blazar/BLZ-1%20%E6%A0%87%E9%A2%98"
        );
    }
}
