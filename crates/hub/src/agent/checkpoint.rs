use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_transport::ExecSpec;
use chrono::Utc;
use serde::Serialize;
use sqlx::Row;

use crate::api::Shared;

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn snapshot_script(root: &str, id: &str) -> String {
    format!(
        r#"set -e
cd {root} 2>/dev/null || exit 3
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 4
tmp=$(mktemp "${{TMPDIR:-/tmp}}/blazar-idx.XXXXXX")
trap 'rm -f "$tmp"' EXIT
idx=$(git rev-parse --git-path index)
if [ -f "$idx" ]; then cp "$idx" "$tmp"; else rm -f "$tmp"; fi
export GIT_INDEX_FILE="$tmp"
git add -A -- . >/dev/null 2>&1
tree=$(git write-tree)
parent=$(git rev-parse -q --verify HEAD || true)
c=$(printf 'blazar checkpoint\n' | git -c user.name=blazar -c user.email=blazar@localhost commit-tree "$tree" ${{parent:+-p "$parent"}})
git update-ref {r} "$c"
echo "$c"
"#,
        root = q(root),
        r = q(&format!("refs/blazar/checkpoints/{id}")),
    )
}

fn restore_script(root: &str, commit: &str) -> String {
    format!(
        r#"set -e
cd {root} 2>/dev/null || exit 3
git cat-file -e {c}^{{commit}} 2>/dev/null || {{ echo '检查点已不存在（可能被 git gc 清掉了）'; exit 5; }}
tmp=$(mktemp "${{TMPDIR:-/tmp}}/blazar-idx.XXXXXX")
trap 'rm -f "$tmp"' EXIT
export GIT_INDEX_FILE="$tmp"
git read-tree {c}
git ls-files -z -- . | git checkout-index -f -z --stdin
git ls-files -z -o --exclude-standard -- . | while IFS= read -r -d '' f; do rm -f -- "$f"; done
echo ok
"#,
        root = q(root),
        c = q(commit),
    )
}

pub async fn snapshot(st: &Shared, node: &str, root: &str) -> Option<(String, String)> {
    let id = uuid::Uuid::now_v7().to_string();
    let spec = ExecSpec::new("bash")
        .arg("-s")
        .stdin(snapshot_script(root, &id).into_bytes());
    let out = tokio::time::timeout(Duration::from_secs(20), st.transport(node).exec(spec))
        .await
        .ok()?
        .ok()?;
    if out.code != 0 {
        if out.code != 4 {
            tracing::warn!(target: "blazar::checkpoint", "拍快照失败（{}）: {}", out.code, out.stderr.trim());
        }
        return None;
    }
    let sha = out.stdout.lines().last()?.trim().to_owned();
    (sha.len() >= 40 && sha.chars().all(|c| c.is_ascii_hexdigit())).then_some((id, sha))
}

pub async fn record(
    st: &Shared,
    workspace: &str,
    snap: (String, String),
    session: Option<&str>,
    seq: Option<u64>,
    label: &str,
) {
    let _ = sqlx::query(
        "INSERT INTO checkpoints (id, workspace_id, session_id, seq, commit_sha, label, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&snap.0)
    .bind(workspace)
    .bind(session)
    .bind(seq.and_then(|s| i64::try_from(s).ok()))
    .bind(&snap.1)
    .bind(label)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
}

#[derive(Serialize)]
struct CheckpointView {
    id: String,
    session_id: Option<String>,
    seq: Option<i64>,
    label: String,
    created_at: String,
}

pub async fn list(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let rows = sqlx::query(
        "SELECT id, session_id, seq, label, created_at FROM checkpoints
         WHERE workspace_id = ?1 ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    Json(
        rows.iter()
            .map(|r| CheckpointView {
                id: r.try_get("id").unwrap_or_default(),
                session_id: r.try_get("session_id").ok().flatten(),
                seq: r.try_get("seq").ok().flatten(),
                label: r.try_get("label").unwrap_or_default(),
                created_at: r.try_get("created_at").unwrap_or_default(),
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

pub async fn restore(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match restore_to(&st, &id).await {
        Ok(undo) => Json(serde_json::json!({ "ok": true, "undo": undo })).into_response(),
        Err((status, msg)) => fail(status, msg),
    }
}

pub async fn restore_to(st: &Shared, id: &str) -> Result<Option<String>, (StatusCode, String)> {
    let row = sqlx::query(
        "SELECT c.workspace_id, c.commit_sha, n.name AS node, w.path
         FROM checkpoints c JOIN workspaces w ON w.id = c.workspace_id
         JOIN nodes n ON n.id = w.node_id WHERE c.id = ?1",
    )
    .bind(id)
    .fetch_optional(st.db.pool())
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or((StatusCode::NOT_FOUND, "没有这个检查点".to_owned()))?;
    let ws: String = row.try_get("workspace_id").unwrap_or_default();
    let sha: String = row.try_get("commit_sha").unwrap_or_default();
    let node: String = row.try_get("node").unwrap_or_default();
    let path: String = row.try_get("path").unwrap_or_default();

    let busy = st
        .running
        .read()
        .await
        .values()
        .any(|r| r.workspace_id.to_string() == ws);
    if busy {
        return Err((
            StatusCode::CONFLICT,
            "agent 还在运行，先中断再回退".to_owned(),
        ));
    }

    let undo = snapshot(st, &node, &path).await;
    if let Some(snap) = undo.clone() {
        record(st, &ws, snap, None, None, "回退前").await;
    }
    let spec = ExecSpec::new("bash")
        .arg("-s")
        .stdin(restore_script(&path, &sha).into_bytes());
    let out =
        match tokio::time::timeout(Duration::from_secs(60), st.transport(&node).exec(spec)).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err((StatusCode::BAD_GATEWAY, e.to_string())),
            Err(_) => return Err((StatusCode::GATEWAY_TIMEOUT, "回退超时".to_owned())),
        };
    if out.code != 0 {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "回退失败：{}",
                format!("{}{}", out.stdout, out.stderr).trim()
            ),
        ));
    }
    tracing::info!(target: "blazar::checkpoint", "工作区 {ws} 回退到检查点 {id}");
    Ok(undo.map(|u| u.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, dir: &std::path::Path) -> (i32, String) {
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .current_dir(dir)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }

    #[test]
    fn snapshot_then_restore_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let git = |a: &str| {
            sh(&format!("git -c user.name=t -c user.email=t@t {a}"), &root);
        };
        git("init -q");
        std::fs::write(root.join("a.txt"), "v1\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.log\n").unwrap();
        git("add -A");
        git("commit -qm init");

        std::fs::write(root.join("staged.txt"), "mine\n").unwrap();
        git("add staged.txt");
        std::fs::write(root.join("a.txt"), "v2 未提交\n").unwrap();

        let r = root.display().to_string();
        let (code, out) = sh(&snapshot_script(&r, "t1"), &root);
        assert_eq!(code, 0, "{out}");
        let sha = out.lines().last().unwrap().trim().to_owned();
        let (_, staged) = sh("git diff --cached --name-only", &root);
        assert_eq!(staged.trim(), "staged.txt", "用户的暂存区必须原样保留");

        std::fs::write(root.join("a.txt"), "agent 改坏了\n").unwrap();
        std::fs::write(root.join("new.rs"), "fn x() {}\n").unwrap();
        std::fs::write(root.join("ignored.log"), "log\n").unwrap();

        let (code, out) = sh(&restore_script(&r, &sha), &root);
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "v2 未提交\n"
        );
        assert!(!root.join("new.rs").exists(), "快照之后新建的文件要删掉");
        assert!(root.join("ignored.log").exists(), "忽略的文件不能动");
        let (_, log) = sh("git log --oneline | wc -l", &root);
        assert_eq!(log.trim(), "1", "不能产生提交记录");
    }

    #[test]
    fn non_git_directory_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let (code, _) = sh(
            &snapshot_script(&d.path().display().to_string(), "x"),
            d.path(),
        );
        assert_eq!(code, 4);
    }
}
