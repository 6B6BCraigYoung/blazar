//! 丢弃还没提交的改动：一整个文件，或者其中一处。
//! 丢之前把文件原样（内容、可执行位、暂存区里的那一项）存进 hub 自己的库，换一个 token；
//! 10 分钟内凭 token 还原。文件在丢弃之后又被改过就拒绝还原，免得盖掉新的改动。
//! 只动这一两个路径，不碰提交历史，也不对整棵树 checkout / reset。

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use blazar_transport::ExecSpec;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Ctx, Out, ctx, fail, prelude, q};
use crate::api::Shared;
use crate::state::ServerEvent;

pub const KEEP_SECS: i64 = 600;
const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct DiscardBody {
    pub path: String,

    /// 改名的文件：旧路径也要放回去。
    #[serde(default)]
    pub old_path: Option<String>,

    /// 只丢这一处；不给就丢整个文件。
    #[serde(default)]
    pub hunk: Option<HunkBody>,
}

#[derive(Debug, Deserialize)]
pub struct HunkBody {
    pub old: u32,
    pub new: u32,

    /// 差异视图里看到的那几行，每行带着 diff 的前缀：空格、+、-，或 `\`（行尾没换行的提示）。
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    path: String,
    kind: String,
    exec: bool,
    content: String,
    /// 整个文件丢弃时才会动暂存区，这里记下原来的那一项（`mode sha`，空串表示原来不在暂存区）。
    index: Option<String>,
    after: String,
}

const TOP: &str = r#"cd "$(git rev-parse --show-toplevel)" || exit 5
export GIT_LITERAL_PATHSPECS=1
state() {
  if [ -L "$1" ]; then printf 'link:%s' "$(readlink "$1")"
  elif [ -f "$1" ]; then printf 'file:%s' "$(git hash-object --no-filters -- "$1")"; [ -x "$1" ] && printf '+x'
  elif [ -e "$1" ]; then printf 'other'
  else printf 'none'; fi
}
snap() {
  F=$1
  if [ -L "$F" ]; then K=link
  elif [ -f "$F" ]; then K=file
    S=$(wc -c < "$F")
    [ $((S)) -gt __MAX__ ] && { echo "「$F」超过 8 MB，不能安全地丢弃" >&2; exit 40; }
  elif [ -e "$F" ]; then echo "「$F」不是普通文件，不能丢弃" >&2; exit 40
  else K=none; fi
  [ -n "$(git ls-files -u -- "$F")" ] && { echo "「$F」还有冲突没解决，先解决冲突" >&2; exit 41; }
  X=0; [ "$K" = file ] && [ -x "$F" ] && X=1
  echo "__ENTRY__"; echo "kind=$K"; echo "exec=$X"
  echo "index=$(git ls-files -s -- "$F" | cut -f1 | cut -d' ' -f1,2)"
  echo "__CONTENT__"
  case $K in
    file) base64 < "$F" ;;
    link) printf '%s' "$(readlink "$F")" | base64 ;;
  esac
  echo "__END__"
}
"#;

fn top() -> String {
    TOP.replace("__MAX__", &MAX_BYTES.to_string())
}

fn clean_path(p: &str) -> Result<String, String> {
    let p = blazar_vfs::safe_relative_path(p).map_err(|_| "文件路径不对".to_owned())?;
    if p.chars().any(|c| c.is_control() || c == '"' || c == '\\') {
        return Err("这个文件名带特殊字符，不能在这里丢弃".into());
    }
    Ok(p)
}

/// 用差异视图里那一处改动拼出一份只含这一处的补丁，反着打回去就是丢弃它。
fn hunk_patch(path: &str, h: &HunkBody) -> Result<Vec<u8>, String> {
    if h.lines.is_empty() || h.lines.len() > 20_000 {
        return Err("这处改动是空的".into());
    }
    let (mut old_n, mut new_n, mut changed) = (0u32, 0u32, false);
    for l in &h.lines {
        if l.contains('\n') || l.contains('\r') {
            return Err("这处改动的内容不对".into());
        }
        match l.as_bytes().first() {
            Some(b' ') => {
                old_n += 1;
                new_n += 1;
            }
            Some(b'-') => {
                old_n += 1;
                changed = true;
            }
            Some(b'+') => {
                new_n += 1;
                changed = true;
            }
            Some(b'\\') => {}
            _ => return Err("这处改动的内容不对".into()),
        }
    }
    if !changed {
        return Err("这处没有改动".into());
    }
    let mut out = format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -{},{old_n} +{},{new_n} @@\n",
        h.old, h.new
    );
    for l in &h.lines {
        out.push_str(l);
        out.push('\n');
    }
    Ok(out.into_bytes())
}

async fn run(st: &Shared, c: &Ctx, script: String, stdin: Vec<u8>) -> Result<Out, String> {
    let spec = ExecSpec::new("bash")
        .arg("-lc")
        .arg(format!("{}{}{script}", prelude(c), top()))
        .stdin(stdin);
    match tokio::time::timeout(Duration::from_secs(90), st.transport(&c.node).exec(spec)).await {
        Ok(Ok(o)) => Ok(Out {
            code: o.code,
            stdout: o.stdout,
            stderr: o.stderr,
        }),
        Ok(Err(e)) => Err(format!("连不上 {}：{e}", c.node)),
        Err(_) => Err("机器太久没回应，改动没丢".into()),
    }
}

fn parse_snap(raw: &str) -> Option<(Vec<Entry>, Vec<String>)> {
    let (snaps, after) = raw.split_once("__AFTER__\n")?;
    let mut entries = Vec::new();
    for block in snaps.split("__ENTRY__\n").skip(1) {
        let (head, rest) = block.split_once("__CONTENT__\n")?;
        let content = rest.split("__END__").next()?;
        let mut e = Entry {
            path: String::new(),
            kind: String::new(),
            exec: false,
            content: content.split_whitespace().collect(),
            index: None,
            after: String::new(),
        };
        for line in head.lines() {
            match line.split_once('=') {
                Some(("kind", v)) => e.kind = v.to_owned(),
                Some(("exec", v)) => e.exec = v == "1",
                Some(("index", v)) => e.index = Some(v.trim().to_owned()),
                _ => {}
            }
        }
        entries.push(e);
    }
    let after = after.lines().map(str::to_owned).collect();
    Some((entries, after))
}

async fn sweep_now(st: &Shared) {
    let cutoff = (Utc::now() - chrono::Duration::seconds(KEEP_SECS)).to_rfc3339();
    let _ = sqlx::query("DELETE FROM discards WHERE created_at < ?1")
        .bind(cutoff)
        .execute(st.db.pool())
        .await;
}

/// 过期的还原数据每隔一会儿清一次，免得没人再点丢弃时一直占着库。
pub async fn sweep(st: Shared) {
    let mut tick = tokio::time::interval(Duration::from_secs(120));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        sweep_now(&st).await;
    }
}

pub async fn discard(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<DiscardBody>,
) -> Response {
    let c = match ctx(&st, &id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    sweep_now(&st).await;
    let path = match clean_path(&b.path) {
        Ok(p) => p,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let mut paths = vec![path.clone()];
    if b.hunk.is_none()
        && let Some(old) = b.old_path.as_deref().filter(|o| !o.is_empty())
    {
        match clean_path(old) {
            Ok(o) if o != path => paths.push(o),
            Ok(_) => {}
            Err(e) => return fail(StatusCode::BAD_REQUEST, e),
        }
    }
    let list = paths.iter().map(|p| q(p)).collect::<Vec<_>>().join(" ");
    let (script, stdin) = match &b.hunk {
        Some(h) => match hunk_patch(&path, h) {
            Ok(patch) => (
                format!(
                    r#"snap {p}
git apply -R --whitespace=nowarn - 2>/dev/null || {{ echo "这处改动已经变了，刷新一下再试" >&2; exit 42; }}
echo "__AFTER__"; state {p}; echo"#,
                    p = q(&path)
                ),
                patch,
            ),
            Err(e) => return fail(StatusCode::BAD_REQUEST, e),
        },
        None => (
            format!(
                r#"for f in {list}; do snap "$f"; done
for f in {list}; do
  if git cat-file -e "HEAD:$f" 2>/dev/null; then
    git checkout -q HEAD -- "$f" 2>&1 || exit 43
  else
    git rm -q --cached --ignore-unmatch -- "$f" >/dev/null 2>&1
    rm -f -- "$f"
  fi
done
echo "__AFTER__"; for f in {list}; do state "$f"; echo; done"#
            ),
            Vec::new(),
        ),
    };
    let out = match run(&st, &c, script, stdin).await {
        Ok(o) => o,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    if out.code != 0 {
        let msg = out.stderr.trim();
        return fail(
            StatusCode::CONFLICT,
            if msg.is_empty() { "没丢掉" } else { msg },
        );
    }
    let Some((mut entries, after)) = parse_snap(&out.stdout) else {
        return fail(
            StatusCode::BAD_GATEWAY,
            "机器返回的内容看不懂，改动可能没丢",
        );
    };
    if entries.len() != paths.len() || after.len() < paths.len() {
        return fail(
            StatusCode::BAD_GATEWAY,
            "机器返回的内容看不懂，改动可能没丢",
        );
    }
    for (i, e) in entries.iter_mut().enumerate() {
        e.path = paths[i].clone();
        e.after = after[i].clone();
        if b.hunk.is_some() {
            e.index = None;
        }
    }
    let token = uuid::Uuid::now_v7().simple().to_string();
    let saved = sqlx::query(
        "INSERT INTO discards (token, workspace_id, entries, created_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&token)
    .bind(&id)
    .bind(serde_json::to_string(&entries).unwrap_or_default())
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
    st.emit(ServerEvent::WorkspacesChanged);
    if let Err(e) = saved {
        tracing::warn!(target: "blazar::git", "丢弃的还原数据没存上：{e}");
        return Json(json!({ "ok": true, "token": null })).into_response();
    }
    Json(json!({ "ok": true, "token": token, "expires_in": KEEP_SECS })).into_response()
}

fn restore_script(e: &Entry) -> String {
    let f = q(&e.path);
    let put = match e.kind.as_str() {
        "file" => format!(
            r#"mkdir -p -- "$(dirname -- {f})" || exit 46
T={f}.blazar-restore.$$
cat > "$T" || {{ rm -f -- "$T"; exit 46; }}
{x}mv -f -- "$T" {f} || exit 46"#,
            x = if e.exec {
                r#"chmod +x "$T"
"#
            } else {
                ""
            }
        ),
        "link" => format!(
            r#"rm -f -- {f}; mkdir -p -- "$(dirname -- {f})" || exit 46
ln -s -- "$(cat)" {f} || exit 46"#
        ),
        _ => format!("rm -f -- {f}"),
    };
    let index = match e.index.as_deref() {
        None => String::new(),
        Some("") => format!("\ngit rm -q --cached --ignore-unmatch -- {f} >/dev/null 2>&1"),
        Some(entry) => {
            let (mode, sha) = entry.split_once(' ').unwrap_or(("100644", entry));
            format!(
                "\nprintf '%s %s 0\\t%s\\n' {} {} {f} | git update-index --index-info",
                q(mode),
                q(sha)
            )
        }
    };
    format!(
        r#"[ "$(state {f})" = {after} ] || {{ echo "「"{f}"」在丢弃之后又改过了，没有撤销，免得盖掉新的改动" >&2; exit 44; }}
{put}{index}"#,
        after = q(&e.after)
    )
}

pub async fn restore(
    State(st): State<Shared>,
    Path((id, token)): Path<(String, String)>,
) -> Response {
    let c = match ctx(&st, &id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    sweep_now(&st).await;
    let raw: Option<String> =
        sqlx::query_scalar("SELECT entries FROM discards WHERE token = ?1 AND workspace_id = ?2")
            .bind(&token)
            .bind(&id)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten();
    let Some(entries) = raw.and_then(|r| serde_json::from_str::<Vec<Entry>>(&r).ok()) else {
        return fail(StatusCode::GONE, "撤销已经过期了（丢弃后只保留 10 分钟）");
    };

    // 先全部核对一遍，有一个对不上就一个都不动。
    let check = entries
        .iter()
        .map(|e| {
            format!(
                r#"[ "$(state {f})" = {a} ] || {{ echo "「"{f}"」在丢弃之后又改过了，没有撤销，免得盖掉新的改动" >&2; exit 44; }}
"#,
                f = q(&e.path),
                a = q(&e.after)
            )
        })
        .collect::<String>();
    match run(&st, &c, check, Vec::new()).await {
        Ok(o) if o.code == 0 => {}
        Ok(o) => return fail(StatusCode::CONFLICT, o.stderr.trim()),
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    }
    for e in &entries {
        let bytes = match base64::engine::general_purpose::STANDARD.decode(&e.content) {
            Ok(b) => b,
            Err(_) => {
                return fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "存下的原样坏了，没法撤销",
                );
            }
        };
        match run(&st, &c, restore_script(e), bytes).await {
            Ok(o) if o.code == 0 => {}
            Ok(o) => {
                let msg = o.stderr.trim();
                return fail(
                    StatusCode::CONFLICT,
                    if msg.is_empty() {
                        "没撤销成功"
                    } else {
                        msg
                    },
                );
            }
            Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
        }
    }
    let _ = sqlx::query("DELETE FROM discards WHERE token = ?1")
        .bind(&token)
        .execute(st.db.pool())
        .await;
    st.emit(ServerEvent::WorkspacesChanged);
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &str) -> String {
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!(
                "git -c user.name=t -c user.email=t@example.com {args}"
            ))
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args}: {:?}", out);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    async fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        git(&root, "init -q");
        std::fs::write(
            root.join("a.txt"),
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
        )
        .unwrap();
        std::fs::write(root.join("b.txt"), "keep\n").unwrap();
        git(&root, "add -A");
        git(&root, "commit -q -m init");
        let st = crate::state::AppState::with_services(
            blazar_db::Db::open_in_memory().await.unwrap(),
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, dir.path().join("mesh")),
            crate::services::Services::Isolated,
        );
        sqlx::query("INSERT INTO nodes (id, name, transport, created_at) VALUES ('node', 'local', 'local', '0')")
            .execute(st.db.pool()).await.unwrap();
        sqlx::query("INSERT INTO workspaces (id, node_id, name, path, created_at) VALUES ('ws', 'node', 'repo', ?1, '0')")
            .bind(root.display().to_string()).execute(st.db.pool()).await.unwrap();
        (dir, root, st)
    }

    async fn body(r: Response) -> (StatusCode, serde_json::Value) {
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn drop_it(st: &Shared, b: DiscardBody) -> (StatusCode, serde_json::Value) {
        body(discard(State(st.clone()), Path("ws".into()), Json(b)).await).await
    }

    async fn undo(st: &Shared, token: &str) -> (StatusCode, serde_json::Value) {
        body(restore(State(st.clone()), Path(("ws".into(), token.to_owned()))).await).await
    }

    fn whole(path: &str) -> DiscardBody {
        DiscardBody {
            path: path.into(),
            old_path: None,
            hunk: None,
        }
    }

    fn read(root: &std::path::Path, p: &str) -> String {
        std::fs::read_to_string(root.join(p)).unwrap()
    }

    #[tokio::test]
    async fn discarding_a_file_reverts_it_and_restore_brings_it_back_staged() {
        let (_d, root, st) = fixture().await;
        std::fs::write(root.join("a.txt"), "staged\n").unwrap();
        git(&root, "add a.txt");
        std::fs::write(root.join("a.txt"), "staged\nand more\n").unwrap();
        let (s, v) = drop_it(&st, whole("a.txt")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(read(&root, "a.txt").starts_with("one\n"));
        assert_eq!(git(&root, "status --porcelain"), "");
        assert_eq!(read(&root, "b.txt"), "keep\n");

        let (s, v) = undo(&st, v["token"].as_str().unwrap()).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(read(&root, "a.txt"), "staged\nand more\n");
        assert_eq!(git(&root, "show :a.txt"), "staged\n");
    }

    #[tokio::test]
    async fn discarding_an_untracked_file_deletes_it_and_restore_recreates_it() {
        let (_d, root, st) = fixture().await;
        std::fs::create_dir_all(root.join("new/dir")).unwrap();
        std::fs::write(root.join("new/dir/n.sh"), "#!/bin/sh\necho hi\n").unwrap();
        git(&root, "update-index --chmod=+x --add new/dir/n.sh");
        std::fs::set_permissions(
            root.join("new/dir/n.sh"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        std::fs::write(root.join("u.txt"), "untracked\n").unwrap();
        let (s, v) = drop_it(&st, whole("u.txt")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(!root.join("u.txt").exists());
        let (s, v) = undo(&st, v["token"].as_str().unwrap()).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(read(&root, "u.txt"), "untracked\n");
        assert_eq!(git(&root, "status --porcelain -- u.txt"), "?? u.txt\n");
        std::fs::remove_file(root.join("u.txt")).unwrap();

        let (s, v) = drop_it(&st, whole("new/dir/n.sh")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(!root.join("new/dir/n.sh").exists());
        assert_eq!(git(&root, "status --porcelain"), "");

        let (s, v) = undo(&st, v["token"].as_str().unwrap()).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(read(&root, "new/dir/n.sh"), "#!/bin/sh\necho hi\n");
        let mode = std::fs::metadata(root.join("new/dir/n.sh")).unwrap();
        assert!(std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o100 != 0);
        assert_eq!(git(&root, "status --porcelain"), "A  new/dir/n.sh\n");
    }

    #[tokio::test]
    async fn discarding_one_hunk_leaves_the_other_alone() {
        let (_d, root, st) = fixture().await;
        std::fs::write(
            root.join("a.txt"),
            "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nTEN\n",
        )
        .unwrap();
        let hunk = HunkBody {
            old: 7,
            new: 7,
            lines: vec![
                " seven".into(),
                " eight".into(),
                " nine".into(),
                "-ten".into(),
                "+TEN".into(),
            ],
        };
        let (s, v) = drop_it(
            &st,
            DiscardBody {
                path: "a.txt".into(),
                old_path: None,
                hunk: Some(hunk),
            },
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(
            read(&root, "a.txt"),
            "ONE\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n"
        );
        let (s, v) = undo(&st, v["token"].as_str().unwrap()).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(read(&root, "a.txt").ends_with("nine\nTEN\n"));

        // 文件已经不是看到的样子了：打不回去，原样不动。
        let stale = HunkBody {
            old: 7,
            new: 7,
            lines: vec![" seven".into(), "-eight".into(), "+EIGHT".into()],
        };
        let (s, v) = drop_it(
            &st,
            DiscardBody {
                path: "a.txt".into(),
                old_path: None,
                hunk: Some(stale),
            },
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert!(read(&root, "a.txt").ends_with("nine\nTEN\n"));
    }

    #[tokio::test]
    async fn restore_is_refused_when_the_file_changed_again() {
        let (_d, root, st) = fixture().await;
        std::fs::write(root.join("a.txt"), "mine\n").unwrap();
        let (_, v) = drop_it(&st, whole("a.txt")).await;
        let token = v["token"].as_str().unwrap().to_owned();
        std::fs::write(root.join("a.txt"), "the agent wrote this later\n").unwrap();
        let (s, v) = undo(&st, &token).await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert!(v["error"].as_str().unwrap().contains("又改过了"), "{v}");
        assert_eq!(read(&root, "a.txt"), "the agent wrote this later\n");
    }

    #[tokio::test]
    async fn tokens_expire_and_cannot_be_used_twice() {
        let (_d, root, st) = fixture().await;
        std::fs::write(root.join("a.txt"), "mine\n").unwrap();
        let (_, v) = drop_it(&st, whole("a.txt")).await;
        let token = v["token"].as_str().unwrap().to_owned();
        assert_eq!(undo(&st, &token).await.0, StatusCode::OK);
        assert_eq!(undo(&st, &token).await.0, StatusCode::GONE);

        let (_, v) = drop_it(&st, whole("a.txt")).await;
        let token = v["token"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE discards SET created_at = '2000-01-01T00:00:00+00:00'")
            .execute(st.db.pool())
            .await
            .unwrap();
        assert_eq!(undo(&st, &token).await.0, StatusCode::GONE);
        assert!(read(&root, "a.txt").starts_with("one\n"));
    }

    #[tokio::test]
    async fn paths_outside_the_repo_are_refused() {
        let (_d, _root, st) = fixture().await;
        for bad in ["../x", "/etc/passwd", ""] {
            assert_eq!(drop_it(&st, whole(bad)).await.0, StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn hunk_patch_counts_both_sides() {
        let h = HunkBody {
            old: 3,
            new: 4,
            lines: vec![" a".into(), "-b".into(), "+c".into(), "+d".into()],
        };
        let p = String::from_utf8(hunk_patch("x y.rs", &h).unwrap()).unwrap();
        assert!(p.contains("@@ -3,2 +4,3 @@\n a\n-b\n+c\n+d\n"), "{p}");
        let ctx_only = HunkBody {
            old: 1,
            new: 1,
            lines: vec![" a".into()],
        };
        assert!(hunk_patch("x", &ctx_only).is_err());
    }
}
