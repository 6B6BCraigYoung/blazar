use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_transport::ExecSpec;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::Shared;
use crate::state::ServerEvent;

mod dev;
mod ports;
mod preview;

pub use dev::{detect_url, dev_get, dev_start, dev_stop, loopback_ports};
pub use ports::{descendants, listening_ports, parse_lsof, parse_ss};
pub use preview::{Address, choose_address};

const MAX_SCRIPT: usize = 8000;
const KEEP_OUTPUT: usize = 64 * 1024;
const SETUP_TIMEOUT: u64 = 900;
const CLEANUP_TIMEOUT: u64 = 300;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub(crate) fn cd_target(path: &str) -> String {
    let p = path.trim();
    match p.strip_prefix("~/") {
        Some(rest) => format!("~/{}", q(rest)),
        None if p == "~" || p.is_empty() => "~".to_owned(),
        None => q(p),
    }
}

fn changed(st: &Shared, ws: &str) {
    if let Ok(id) = ws.parse() {
        st.emit(ServerEvent::ScriptsChanged {
            workspace_id: blazar_core_types::WorkspaceId(id),
        });
    }
}

struct Place {
    node: String,
    path: String,
    repo_root: Option<String>,
}

async fn place(st: &Shared, ws: &str) -> Option<Place> {
    let r = sqlx::query(
        "SELECT n.name AS node, w.path, w.repo_root FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()??;
    Some(Place {
        node: r.try_get("node").ok()?,
        path: r.try_get("path").ok()?,
        repo_root: r.try_get("repo_root").ok().flatten(),
    })
}

async fn load(st: &Shared, ws: &str) -> Value {
    let r = sqlx::query("SELECT setup, cleanup, dev, copy_files, preview_port, updated_at FROM workspace_scripts WHERE workspace_id = ?1")
        .bind(ws)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
    let s = |k: &str| {
        r.as_ref()
            .and_then(|r| r.try_get::<String, _>(k).ok())
            .unwrap_or_default()
    };
    json!({ "setup": s("setup"), "cleanup": s("cleanup"), "dev": s("dev"), "copy_files": s("copy_files"),
            "preview_port": r.as_ref().and_then(|r| r.try_get::<Option<i64>, _>("preview_port").ok().flatten()),
            "updated_at": r.as_ref().and_then(|r| r.try_get::<String, _>("updated_at").ok()) })
}

fn preview_port_of(v: Option<i64>) -> Result<Option<u16>, String> {
    match v {
        None => Ok(None),
        Some(p) => u16::try_from(p)
            .ok()
            .filter(|p| *p > 0)
            .map(Some)
            .ok_or_else(|| "预览端口要在 1 到 65535 之间".to_owned()),
    }
}

async fn preview_port(st: &Shared, ws: &str) -> Option<u16> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT preview_port FROM workspace_scripts WHERE workspace_id = ?1")
            .bind(ws)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .flatten();
    preview_port_of(v).ok().flatten()
}

pub async fn get(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    Json(load(&st, &id).await).into_response()
}

#[derive(Deserialize, Default)]
pub struct ScriptsBody {
    #[serde(default)]
    pub setup: String,
    #[serde(default)]
    pub cleanup: String,
    #[serde(default)]
    pub dev: String,
    #[serde(default)]
    pub copy_files: String,
    #[serde(default)]
    pub preview_port: Option<i64>,
}

fn check_copy(list: &str) -> Result<(), String> {
    for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('/') || line.starts_with('~') || line.split('/').any(|s| s == "..") {
            return Err(format!("拷贝清单里只能写仓库内的相对路径：{line}"));
        }
        if line.chars().any(char::is_control) {
            return Err("拷贝清单里有控制字符".into());
        }
    }
    Ok(())
}

pub async fn put(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<ScriptsBody>,
) -> Response {
    for (name, v) in [
        ("setup", &b.setup),
        ("cleanup", &b.cleanup),
        ("dev", &b.dev),
        ("copy_files", &b.copy_files),
    ] {
        if v.len() > MAX_SCRIPT {
            return fail(
                StatusCode::BAD_REQUEST,
                format!("{name} 太长了（上限 {MAX_SCRIPT} 字节）"),
            );
        }
    }
    if let Err(e) = check_copy(&b.copy_files) {
        return fail(StatusCode::BAD_REQUEST, e);
    }
    let port = match preview_port_of(b.preview_port) {
        Ok(p) => p.map(i64::from),
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let r = sqlx::query(
        "INSERT INTO workspace_scripts (workspace_id, setup, cleanup, dev, copy_files, preview_port, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (workspace_id) DO UPDATE SET setup = excluded.setup, cleanup = excluded.cleanup,
             dev = excluded.dev, copy_files = excluded.copy_files, preview_port = excluded.preview_port,
             updated_at = excluded.updated_at",
    )
    .bind(&id)
    .bind(b.setup.trim())
    .bind(b.cleanup.trim())
    .bind(b.dev.trim())
    .bind(b.copy_files.trim())
    .bind(port)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    match r {
        Ok(_) => Json(load(&st, &id).await).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

fn tail(s: &str, keep: usize) -> String {
    if s.len() <= keep {
        return s.to_owned();
    }
    let mut at = s.len() - keep;
    while !s.is_char_boundary(at) {
        at += 1;
    }
    format!("…（前面省略 {} 字节）\n{}", at, &s[at..])
}

async fn run_script(
    st: &Shared,
    ws: &str,
    kind: &str,
    trigger: &str,
    script: String,
    timeout: u64,
) -> (String, bool) {
    let id = uuid::Uuid::now_v7().to_string();
    let _ = sqlx::query(
        "INSERT INTO script_runs (id, workspace_id, kind, trigger, status, started_at) VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
    )
    .bind(&id)
    .bind(ws)
    .bind(kind)
    .bind(trigger)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    changed(st, ws);
    let Some(p) = place(st, ws).await else {
        return (id, false);
    };

    let body = format!(
        "cd {} || exit 97\nexec 2>&1\n{script}\n",
        cd_target(&p.path)
    );
    let spec = ExecSpec::new("bash")
        .arg("-l")
        .arg("-s")
        .stdin(body.into_bytes());
    let out = tokio::time::timeout(
        Duration::from_secs(timeout),
        st.transport(&p.node).exec(spec),
    )
    .await;
    let (status, code, output) = match out {
        Ok(Ok(o)) => (
            if o.code == 0 { "ok" } else { "failed" },
            Some(o.code),
            format!("{}{}", o.stdout, o.stderr),
        ),
        Ok(Err(e)) => ("failed", None, format!("连不上 {}：{e}", p.node)),
        Err(_) => (
            "timeout",
            None,
            format!("超过 {timeout} 秒没跑完，已放弃等待"),
        ),
    };
    let _ = sqlx::query(
        "UPDATE script_runs SET status = ?2, exit_code = ?3, output = ?4, finished_at = ?5 WHERE id = ?1",
    )
    .bind(&id)
    .bind(status)
    .bind(code)
    .bind(tail(&output, KEEP_OUTPUT))
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    changed(st, ws);
    (id, status == "ok")
}

pub async fn run(State(st): State<Shared>, Path((id, kind)): Path<(String, String)>) -> Response {
    if !matches!(kind.as_str(), "setup" | "cleanup" | "copy") {
        return fail(
            StatusCode::NOT_FOUND,
            "只有 setup / cleanup / copy 能这样跑",
        );
    }
    let cfg = load(&st, &id).await;
    if kind == "copy" {
        let st2 = st.clone();
        tokio::spawn(async move { copy_files(&st2, &id, "manual").await });
        return Json(json!({ "started": true })).into_response();
    }
    let script = cfg[kind.as_str()].as_str().unwrap_or_default().to_owned();
    if script.trim().is_empty() {
        return fail(StatusCode::CONFLICT, "这个脚本还是空的");
    }
    let st2 = st.clone();
    let timeout = if kind == "setup" {
        SETUP_TIMEOUT
    } else {
        CLEANUP_TIMEOUT
    };
    tokio::spawn(async move { run_script(&st2, &id, &kind, "manual", script, timeout).await });
    Json(json!({ "started": true })).into_response()
}

pub async fn runs(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let rows = sqlx::query(
        "SELECT id, kind, trigger, status, exit_code, output, started_at, finished_at FROM script_runs
         WHERE workspace_id = ?1 ORDER BY started_at DESC LIMIT 20",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    Json(
        rows.iter()
            .map(|r| {
                let s = |k: &str| r.try_get::<Option<String>, _>(k).ok().flatten();
                json!({ "id": s("id"), "kind": s("kind"), "trigger": s("trigger"), "status": s("status"),
                        "exit_code": r.try_get::<Option<i64>, _>("exit_code").ok().flatten(),
                        "output": s("output"), "started_at": s("started_at"), "finished_at": s("finished_at") })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn copy_files(st: &Shared, ws: &str, trigger: &str) {
    let cfg = load(st, ws).await;
    let list = cfg["copy_files"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    let Some(p) = place(st, ws).await else { return };
    let Some(src) = p.repo_root.filter(|r| !r.is_empty() && *r != p.path) else {
        if trigger == "manual" {
            run_script(
                st,
                ws,
                "copy",
                trigger,
                "echo '这不是隔离工作区（没有源仓库），没什么可拷的'; exit 1".into(),
                30,
            )
            .await;
        }
        return;
    };
    if list.is_empty() {
        return;
    }
    let script = format!(
        r#"SRC={src}; DST="$PWD"
cd "$SRC" || {{ echo "源仓库 $SRC 不在了"; exit 1; }}
N=0
while IFS= read -r pat; do
  [ -z "$pat" ] && continue
  case "$pat" in /*|*..*) echo "跳过：$pat"; continue;; esac
  FOUND=$(compgen -G "$pat") || {{ echo "没匹配到：$pat"; continue; }}
  while IFS= read -r f; do
    mkdir -p "$DST/$(dirname "$f")" && cp -Rp "$f" "$DST/$f" && {{ echo "已拷贝 $f"; N=$((N+1)); }}
  done <<< "$FOUND"
done <<'BLAZAR_COPY_LIST'
{list}
BLAZAR_COPY_LIST
echo "共 $N 项""#,
        src = cd_target(&src),
        list = list
            .lines()
            .filter(|l| l.trim() != "BLAZAR_COPY_LIST")
            .collect::<Vec<_>>()
            .join("\n"),
    );
    run_script(st, ws, "copy", trigger, script, 120).await;
}

pub async fn on_workspace_created(st: Shared, ws: String, repo_root: String) {
    let prev: Option<String> = sqlx::query_scalar(
        "SELECT s.workspace_id FROM workspace_scripts s JOIN workspaces w ON w.id = s.workspace_id
         WHERE w.repo_root = ?1 AND w.id != ?2 ORDER BY s.updated_at DESC LIMIT 1",
    )
    .bind(&repo_root)
    .bind(&ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    let Some(prev) = prev else { return };
    let _ = sqlx::query(
        "INSERT OR IGNORE INTO workspace_scripts (workspace_id, setup, cleanup, dev, copy_files, preview_port, updated_at)
         SELECT ?1, setup, cleanup, dev, copy_files, preview_port, ?3 FROM workspace_scripts WHERE workspace_id = ?2",
    )
    .bind(&ws)
    .bind(&prev)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    copy_files(&st, &ws, "create").await;
    let setup = load(&st, &ws).await["setup"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if !setup.trim().is_empty() {
        run_script(&st, &ws, "setup", "create", setup, SETUP_TIMEOUT).await;
    }
}

pub async fn on_run_finished(st: Shared, ws: blazar_core_types::WorkspaceId, status: &'static str) {
    if status != "done" {
        return;
    }
    let ws = ws.to_string();
    let cleanup = load(&st, &ws).await["cleanup"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if cleanup.trim().is_empty() {
        return;
    }
    let Some(p) = place(&st, &ws).await else {
        return;
    };
    let probe = format!(
        "cd {} && {{ git rev-parse --git-dir >/dev/null 2>&1 || exit 0; [ -n \"$(git status --porcelain | head -1)\" ]; }}",
        cd_target(&p.path)
    );
    let dirty = st
        .transport(&p.node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(probe))
        .await
        .is_ok_and(|o| o.code == 0);
    if dirty {
        run_script(&st, &ws, "cleanup", "turn_end", cleanup, CLEANUP_TIMEOUT).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_lists_stay_inside_the_repo() {
        assert!(check_copy(".env\nconfig/*.local.json\n\n  .vscode/settings.json").is_ok());
        for bad in ["/etc/passwd", "../secrets", "a/../../b", "~/.ssh/id_rsa"] {
            assert!(check_copy(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn long_output_keeps_the_tail() {
        let s = "a".repeat(100) + "尾巴";
        let t = tail(&s, 10);
        assert!(t.ends_with("尾巴") && t.starts_with('…'));
        assert_eq!(tail("short", 10), "short");
    }
}
