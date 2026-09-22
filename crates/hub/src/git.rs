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

const AI_DIFF_CHARS: usize = 14_000;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn cd_target(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() || p == "~" {
        return "~".to_owned();
    }
    match p.strip_prefix("~/") {
        Some(rest) => format!("~/{}", q(rest)),
        None => q(p),
    }
}

fn valid_ref(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.ends_with('/')
        && !name.ends_with(".lock")
        && name
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !"~^:?*[\\".contains(c))
}

struct Ctx {
    node: String,
    path: String,
    name: String,
    target: Option<String>,
    pr_url: Option<String>,
    pr_number: Option<i64>,
    pr_state: Option<String>,
    running: i64,
}

async fn ctx(st: &Shared, id: &str) -> Result<Ctx, Response> {
    let row = sqlx::query(
        "SELECT n.name AS node, w.path, w.name, w.target_branch, w.pr_url, w.pr_number, w.pr_state,
                (SELECT COUNT(*) FROM sessions s WHERE s.workspace_id = w.id AND s.status = 'running') AS running
         FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(id)
    .fetch_optional(st.db.pool())
    .await
    .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| fail(StatusCode::NOT_FOUND, "没有这个工作区"))?;
    Ok(Ctx {
        node: row.try_get("node").unwrap_or_default(),
        path: row.try_get("path").unwrap_or_default(),
        name: row.try_get("name").unwrap_or_default(),
        target: row
            .try_get::<Option<String>, _>("target_branch")
            .ok()
            .flatten()
            .filter(|t| valid_ref(t)),
        pr_url: row.try_get("pr_url").ok().flatten(),
        pr_number: row.try_get("pr_number").ok().flatten(),
        pr_state: row.try_get("pr_state").ok().flatten(),
        running: row.try_get("running").unwrap_or(0),
    })
}

fn prelude(c: &Ctx) -> String {
    format!(
        r#"cd {root} 2>/dev/null || {{ echo "目录不在了（工作区可能已冻结）" >&2; exit 3; }}
git rev-parse --git-dir >/dev/null 2>&1 || {{ echo "这个目录不是 git 仓库" >&2; exit 5; }}
export GIT_TERMINAL_PROMPT=0 GIT_EDITOR=true GIT_MERGE_AUTOEDIT=no LC_MESSAGES=C
TO=""; command -v timeout >/dev/null 2>&1 && TO="timeout 60"
[ -z "$TO" ] && command -v gtimeout >/dev/null 2>&1 && TO="gtimeout 60"
T={target}
TGUESS=0
if [ -z "$T" ]; then
  TGUESS=1
  for c in main master develop trunk; do
    git show-ref --verify -q "refs/heads/$c" && {{ T=$c; break; }}
  done
  [ -z "$T" ] && T=$(git symbolic-ref --short -q refs/remotes/origin/HEAD 2>/dev/null)
fi
BR=$(git symbolic-ref --short -q HEAD 2>/dev/null)
"#,
        root = cd_target(&c.path),
        target = q(c.target.as_deref().unwrap_or("")),
    )
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

async fn sh(st: &Shared, node: &str, script: String) -> Result<Out, String> {
    let out = st
        .transport(node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await
        .map_err(|e| format!("连不上 {node}：{e}"))?;
    Ok(Out {
        code: out.code,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

const STATUS_SCRIPT: &str = r#"G=$(git rev-parse --git-dir)
# 变基到一半时 HEAD 是游离的：分支名得从变基的状态目录里读
if [ -z "$BR" ]; then for f in "$G/rebase-merge/head-name" "$G/rebase-apply/head-name"; do
  [ -f "$f" ] && BR=$(sed 's#^refs/heads/##' "$f") && break; done; fi
echo "branch=$BR"
echo "head=$(git rev-parse --short HEAD 2>/dev/null)"
echo "target=$T"
echo "target_guess=$TGUESS"
if [ -n "$T" ] && git rev-parse --verify -q "$T^{commit}" >/dev/null 2>&1; then
  echo "target_ok=1"
  echo "counts=$(git rev-list --left-right --count "$T"...HEAD 2>/dev/null)"
  git show-ref --verify -q "refs/heads/$T" && echo "target_local=1"
fi
U=$(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null)
echo "upstream=$U"
[ -n "$U" ] && echo "upcounts=$(git rev-list --left-right --count "$U"...HEAD 2>/dev/null)"
echo "remote=$(git remote get-url origin 2>/dev/null)"
if [ -d "$G/rebase-merge" ] || [ -d "$G/rebase-apply" ]; then echo "op=rebase"
elif [ -f "$G/MERGE_HEAD" ]; then echo "op=merge"
elif [ -f "$G/CHERRY_PICK_HEAD" ]; then echo "op=cherry-pick"; fi
command -v gh >/dev/null 2>&1 && echo "gh=1"
echo "__STATUS__"
git -c core.quotepath=false status --porcelain=v1 2>/dev/null | head -400
echo "__COMMITS__"
[ -n "$T" ] && git log -n 40 --format='%h%x09%an%x09%ct%x09%s' "$T"..HEAD 2>/dev/null
true
"#;

fn parse_counts(v: &str) -> (i64, i64) {
    let mut it = v.split_whitespace().filter_map(|n| n.parse::<i64>().ok());
    let behind = it.next().unwrap_or(0);
    let ahead = it.next().unwrap_or(0);
    (behind, ahead)
}

fn parse_status(raw: &str) -> Value {
    let mut kv = std::collections::BTreeMap::new();
    let mut section = "kv";
    let mut files = Vec::new();
    let mut conflicts = Vec::new();
    let mut untracked = 0;
    let mut commits = Vec::new();
    for line in raw.lines() {
        match line {
            "__STATUS__" => {
                section = "status";
                continue;
            }
            "__COMMITS__" => {
                section = "commits";
                continue;
            }
            _ => {}
        }
        match section {
            "kv" => {
                if let Some((k, v)) = line.split_once('=') {
                    kv.insert(k.to_owned(), v.trim().to_owned());
                }
            }
            "status" if line.len() > 3 => {
                let code = &line[..2];
                let path = line[3..].trim_matches('"').to_owned();
                if code == "??" {
                    untracked += 1;
                }

                if matches!(code, "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD") {
                    conflicts.push(path.clone());
                }
                files.push(json!({ "code": code.trim(), "path": path }));
            }
            "commits" => {
                let mut p = line.splitn(4, '\t');
                if let (Some(sha), Some(author), Some(at), Some(subject)) =
                    (p.next(), p.next(), p.next(), p.next())
                {
                    commits.push(json!({
                        "sha": sha, "author": author,
                        "at": at.parse::<i64>().unwrap_or(0), "subject": subject,
                    }));
                }
            }
            _ => {}
        }
    }
    let get = |k: &str| kv.get(k).cloned().unwrap_or_default();
    let (behind, ahead) = parse_counts(&get("counts"));
    let (up_behind, up_ahead) = parse_counts(&get("upcounts"));
    let branch = get("branch");
    let target = get("target");
    let remote = get("remote");
    let web = (!remote.is_empty() && !branch.is_empty())
        .then(|| blazar_worktree::web_links(&remote, &branch))
        .flatten();
    json!({
        "repo": true,
        "branch": branch,
        "detached": branch.is_empty(),
        "head": get("head"),
        "target": target,
        "target_guess": get("target_guess") == "1",
        "target_ok": get("target_ok") == "1",
        "target_local": get("target_local") == "1",
        "same": !branch.is_empty() && branch == target,
        "ahead": ahead,
        "behind": behind,
        "upstream": get("upstream"),
        "up_ahead": up_ahead,
        "up_behind": up_behind,
        "remote": remote,
        "web": web,
        "op": kv.get("op"),
        "gh": get("gh") == "1",
        "uncommitted": files.len(),
        "untracked": untracked,
        "conflicts": conflicts,
        "files": files,
        "commits": commits,
    })
}

async fn status_of(st: &Shared, c: &Ctx) -> Value {
    let script = format!("{}{}", prelude(c), STATUS_SCRIPT);
    let mut v = match sh(st, &c.node, script).await {
        Ok(o) if o.code == 0 => parse_status(&o.stdout),
        Ok(o) => json!({ "repo": false, "reason": o.stderr.trim() }),
        Err(e) => json!({ "repo": false, "reason": e }),
    };
    v["running"] = json!(c.running);
    v["pr"] = match &c.pr_url {
        Some(url) => json!({ "url": url, "number": c.pr_number, "state": c.pr_state }),
        None => Value::Null,
    };
    v
}

pub async fn status(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match ctx(&st, &id).await {
        Ok(c) => Json(status_of(&st, &c).await).into_response(),
        Err(r) => r,
    }
}

pub async fn branches(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let c = match ctx(&st, &id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let script = format!(
        "{}git for-each-ref --sort=-committerdate --format='%(refname:short)' refs/heads refs/remotes | grep -v '/HEAD$' | head -200",
        prelude(&c)
    );
    match sh(&st, &c.node, script).await {
        Ok(o) if o.code == 0 || o.code == 1 => Json(json!({
            "branches": o.stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>()
        }))
        .into_response(),
        Ok(o) => fail(StatusCode::CONFLICT, o.stderr.trim()),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct OpBody {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
}

const NEED_CLEAN: &str = r#"[ -n "$(git status --porcelain --untracked-files=no)" ] && { echo "有没提交的改动：先提交（或让 agent 收尾）再来" >&2; exit 20; }
"#;
const NEED_BRANCH: &str = r#"[ -z "$BR" ] && { echo "当前不在任何分支上（detached HEAD）" >&2; exit 22; }
"#;
const NEED_TARGET: &str = r#"{ [ -n "$T" ] && git rev-parse --verify -q "$T^{commit}" >/dev/null 2>&1; } || { echo "没有可用的目标分支：先在 Git 面板里选一个" >&2; exit 23; }
"#;
const NEED_ORIGIN: &str = r#"git remote get-url origin >/dev/null 2>&1 || { echo "这个仓库没有配置 origin 远端" >&2; exit 13; }
"#;

fn op_script(op: &str, b: &OpBody) -> Result<Option<String>, String> {
    Ok(Some(match op {
        "fetch" => format!("{NEED_ORIGIN}$TO git fetch --prune origin 2>&1"),
        "commit" => {
            let msg = b.message.as_deref().map(str::trim).unwrap_or_default();
            if msg.is_empty() {
                return Err("提交信息不能为空".into());
            }
            format!(
                r#"git add -A 2>&1 || exit 1
if git diff --cached --quiet; then echo "__NOCHANGE__"; exit 0; fi
git commit -q -m {m} 2>&1 && echo "commit=$(git rev-parse --short HEAD)""#,
                m = q(msg)
            )
        }
        "rebase" => format!(
            r#"{NEED_BRANCH}{NEED_TARGET}{NEED_CLEAN}[ "$BR" = "$T" ] && {{ echo "当前就在目标分支上，没什么可变基的" >&2; exit 27; }}
git rebase "$T" 2>&1"#
        ),

        "continue" => r#"G=$(git rev-parse --git-dir)
LEFT=$(git diff --name-only --diff-filter=U -z | while IFS= read -r -d '' f; do
  [ -f "$f" ] && grep -q '^<<<<<<< ' "$f" 2>/dev/null && printf '%s\n' "$f"; done)
[ -n "$LEFT" ] && { echo "这些文件里还有冲突标记：" >&2; echo "$LEFT" >&2; exit 21; }
git add -A 2>&1
if [ -d "$G/rebase-merge" ] || [ -d "$G/rebase-apply" ]; then git rebase --continue 2>&1
elif [ -f "$G/MERGE_HEAD" ]; then git commit --no-edit 2>&1
elif [ -f "$G/CHERRY_PICK_HEAD" ]; then git cherry-pick --continue 2>&1
else echo "没有进行到一半的变基或合并" >&2; exit 28; fi"#
            .to_owned(),
        "abort" => r#"G=$(git rev-parse --git-dir)
if [ -d "$G/rebase-merge" ] || [ -d "$G/rebase-apply" ]; then git rebase --abort 2>&1
elif [ -f "$G/MERGE_HEAD" ]; then git merge --abort 2>&1
elif [ -f "$G/CHERRY_PICK_HEAD" ]; then git cherry-pick --abort 2>&1
else echo "没有进行到一半的变基或合并" >&2; exit 28; fi"#
            .to_owned(),

        "update" => {
            format!(r#"{NEED_BRANCH}{NEED_TARGET}{NEED_CLEAN}git merge --no-edit "$T" 2>&1"#)
        }

        "merge" => format!(
            r#"{NEED_BRANCH}{NEED_TARGET}{NEED_CLEAN}[ "$BR" = "$T" ] && {{ echo "当前就在目标分支上" >&2; exit 27; }}
git show-ref --verify -q "refs/heads/$T" || {{ echo "$T 不是本地分支：远端分支请推送后走 PR" >&2; exit 24; }}
WT=$(git worktree list --porcelain | awk -v b="refs/heads/$T" '/^worktree /{{w=substr($0,10)}} /^branch /{{if($2==b) print w}}' | head -1)
if [ -n "$WT" ]; then
  [ -n "$(git -C "$WT" status --porcelain --untracked-files=no)" ] && {{ echo "$T 签出在 $WT，那边有没提交的改动，不替你合并" >&2; exit 25; }}
  if ! git -C "$WT" merge --no-ff --no-edit -m "Merge branch '$BR' into $T" "$BR" 2>&1; then
    git -C "$WT" merge --abort 2>/dev/null
    echo "合并有冲突：先变基到 $T 把冲突解决掉，再来合并" >&2; exit 26
  fi
else
  git merge-base --is-ancestor "$T" HEAD || {{ echo "$T 上有这条分支没有的提交，不能快进：先变基再合并" >&2; exit 26; }}
  git update-ref "refs/heads/$T" HEAD 2>&1 && echo "fast-forward $T -> $(git rev-parse --short HEAD)"
fi
echo "__MERGED__""#
        ),
        "push" => format!(
            r#"{NEED_BRANCH}{NEED_ORIGIN}$TO git push -u {force} origin "$BR" 2>&1"#,
            force = if b.force { "--force-with-lease" } else { "" }
        ),
        "rename-branch" => {
            let name = b.name.as_deref().map(str::trim).unwrap_or_default();
            if !valid_ref(name) {
                return Err("分支名不合法".into());
            }
            format!(
                r#"{NEED_BRANCH}git check-ref-format --branch {n} >/dev/null 2>&1 || {{ echo "分支名不合法" >&2; exit 29; }}
git branch -m {n} 2>&1 && echo "__RENAMED__""#,
                n = q(name)
            )
        }
        "pr-create" => {
            let title = b.title.as_deref().map(str::trim).unwrap_or_default();
            if title.is_empty() {
                return Err("PR 标题不能为空".into());
            }
            format!(
                r#"{NEED_BRANCH}{NEED_TARGET}{NEED_ORIGIN}command -v gh >/dev/null 2>&1 || {{ echo "这台机器上没有装 gh（GitHub CLI）" >&2; exit 30; }}
[ "$BR" = "${{T#origin/}}" ] && {{ echo "当前分支就是目标分支，没法对自己开 PR" >&2; exit 27; }}
$TO git push -u origin "$BR" 2>&1 || {{ echo "推送失败，PR 没开" >&2; exit 31; }}
$TO gh pr create --title {t} --body {bd} --base "${{T#origin/}}" --head "$BR" {draft} 2>&1"#,
                t = q(title),
                bd = q(b.body.as_deref().unwrap_or("")),
                draft = if b.draft { "--draft" } else { "" },
            )
        }
        _ => return Ok(None),
    }))
}

fn tidy(text: &str) -> String {
    text.replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("hint:")
                && !l.starts_with("Rebasing (")
                && !matches!(*l, "__NOCHANGE__" | "__MERGED__" | "__RENAMED__")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn disruptive(op: &str) -> bool {
    matches!(
        op,
        "rebase" | "continue" | "abort" | "update" | "merge" | "rename-branch"
    )
}

fn find_pr_url(out: &str) -> Option<String> {
    out.split_whitespace()
        .rev()
        .find(|w| w.starts_with("https://") && w.contains("/pull/"))
        .map(|w| w.trim_end_matches(['.', ',', ')']).to_owned())
}

fn pr_number_of(url: &str) -> Option<i64> {
    url.rsplit('/').next()?.parse().ok()
}

pub async fn op(
    State(st): State<Shared>,
    Path((id, op)): Path<(String, String)>,
    body: Option<Json<OpBody>>,
) -> Response {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let c = match ctx(&st, &id).await {
        Ok(c) => c,
        Err(r) => return r,
    };

    if op == "set-target" {
        let t = b.branch.as_deref().map(str::trim).unwrap_or_default();
        if !t.is_empty() && !valid_ref(t) {
            return fail(StatusCode::BAD_REQUEST, "分支名不合法");
        }
        let _ = sqlx::query("UPDATE workspaces SET target_branch = ?2 WHERE id = ?1")
            .bind(&id)
            .bind((!t.is_empty()).then_some(t))
            .execute(st.db.pool())
            .await;
        return match ctx(&st, &id).await {
            Ok(c) => {
                Json(json!({ "ok": true, "status": status_of(&st, &c).await })).into_response()
            }
            Err(r) => r,
        };
    }

    if disruptive(&op) && c.running > 0 {
        return fail(
            StatusCode::CONFLICT,
            "agent 正在这个工作区里干活，等它停下来再动分支",
        );
    }
    let script = match op_script(&op, &b) {
        Ok(Some(s)) => s,
        Ok(None) => return fail(StatusCode::NOT_FOUND, format!("不认识的 git 操作：{op}")),
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let out = match sh(&st, &c.node, format!("{}{}", prelude(&c), script)).await {
        Ok(o) => o,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let ok = out.code == 0;
    let text = format!("{}{}", out.stdout, out.stderr);
    let mut resp = json!({ "ok": ok, "output": tidy(&text) });

    match op.as_str() {
        "commit" => {
            resp["changed"] = json!(!text.contains("__NOCHANGE__"));
            resp["commit"] = json!(
                text.lines()
                    .find_map(|l| l.strip_prefix("commit="))
                    .map(str::trim)
            );
        }

        "push" if !ok => {
            resp["needs_force"] = json!(
                text.contains("non-fast-forward")
                    || text.contains("[rejected]")
                    || text.contains("stale info")
            );
        }
        "merge" if ok && text.contains("__MERGED__") => {
            let n =
                crate::tasks::complete_for_workspace(&st, &id, "分支已合并，任务自动完成。").await;
            resp["tasks_done"] = json!(n);
        }
        "rename-branch" if ok => {
            let name = b.name.as_deref().map(str::trim).unwrap_or_default();
            let _ = sqlx::query(
                "UPDATE workspaces SET branch = ?2 WHERE id = ?1 AND branch IS NOT NULL",
            )
            .bind(&id)
            .bind(name)
            .execute(st.db.pool())
            .await;
            st.emit(ServerEvent::WorkspacesChanged);
        }
        "pr-create" => {
            if let Some(url) = find_pr_url(&text) {
                save_pr(&st, &id, &url, pr_number_of(&url), "OPEN").await;
                resp["ok"] = json!(true);
                resp["url"] = json!(url);
                resp["existing"] = json!(!ok);
            }
        }
        _ => {}
    }

    if let Ok(c) = ctx(&st, &id).await {
        resp["status"] = status_of(&st, &c).await;
    }
    Json(resp).into_response()
}

async fn save_pr(st: &Shared, id: &str, url: &str, number: Option<i64>, state: &str) {
    let _ = sqlx::query(
        "UPDATE workspaces SET pr_url = ?2, pr_number = ?3, pr_state = ?4, pr_checked_at = ?5 WHERE id = ?1",
    )
    .bind(id)
    .bind(url)
    .bind(number)
    .bind(state)
    .bind(Utc::now().to_rfc3339())
    .execute(st.db.pool())
    .await;
}

fn parse_pr(raw: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(raw.trim()).ok()?;
    let url = v["url"].as_str()?.to_owned();
    let (mut pass, mut failed, mut pending) = (0, 0, 0);
    for c in v["statusCheckRollup"].as_array().into_iter().flatten() {
        let verdict = c["conclusion"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| c["state"].as_str())
            .unwrap_or("");
        match verdict {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => pass += 1,
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" => failed += 1,
            _ => pending += 1,
        }
    }
    Some(json!({
        "url": url,
        "number": v["number"].as_i64(),
        "state": v["state"].as_str().unwrap_or("OPEN"),
        "title": v["title"].as_str().unwrap_or(""),
        "draft": v["isDraft"].as_bool().unwrap_or(false),
        "mergeable": v["mergeable"].as_str().unwrap_or(""),
        "review": v["reviewDecision"].as_str().unwrap_or(""),
        "checks": { "pass": pass, "failed": failed, "pending": pending },
    }))
}

async fn refresh_pr(st: &Shared, id: &str, c: &Ctx) -> Result<Option<Value>, String> {
    let script = format!(
        r#"{}command -v gh >/dev/null 2>&1 || {{ echo "__NOGH__"; exit 0; }}
$TO gh pr view --json number,url,state,title,isDraft,mergeable,reviewDecision,statusCheckRollup 2>/dev/null || echo "__NOPR__""#,
        prelude(c)
    );
    let out = sh(st, &c.node, script).await?;
    if out.code != 0 {
        return Err(out.stderr.trim().to_owned());
    }
    if out.stdout.contains("__NOGH__") {
        return Err("这台机器上没有装 gh（GitHub CLI）".into());
    }
    let Some(pr) = parse_pr(&out.stdout) else {
        return Ok(None);
    };
    let state = pr["state"].as_str().unwrap_or("OPEN").to_owned();
    let was = c.pr_state.clone().unwrap_or_default();
    save_pr(
        st,
        id,
        pr["url"].as_str().unwrap_or_default(),
        pr["number"].as_i64(),
        &state,
    )
    .await;
    if state == "MERGED" && was != "MERGED" {
        crate::tasks::complete_for_workspace(st, id, "PR 已合并，任务自动完成。").await;
    }
    if state != was {
        st.emit(ServerEvent::WorkspacesChanged);
    }
    Ok(Some(pr))
}

pub async fn pr(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let c = match ctx(&st, &id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match refresh_pr(&st, &id, &c).await {
        Ok(pr) => Json(json!({ "pr": pr })).into_response(),
        Err(e) => Json(json!({ "pr": null, "reason": e })).into_response(),
    }
}

pub async fn poll_prs(st: Shared) {
    let mut tick = tokio::time::interval(Duration::from_secs(150));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM workspaces WHERE pr_state = 'OPEN' AND status = 'active'",
        )
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
        for id in ids {
            if let Ok(c) = ctx(&st, &id).await {
                let _ = refresh_pr(&st, &id, &c).await;
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DescribeBody {
    pub kind: String,
}

pub async fn describe(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<DescribeBody>,
) -> Response {
    let c = match ctx(&st, &id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let pr = b.kind == "pr";
    let gather = if pr {
        format!(
            r#"{NEED_TARGET}echo "[commits]"; git log --format='- %s' "$T"..HEAD | head -60
echo "[stat]"; git --no-pager diff --stat "$T"...HEAD | tail -40
echo "[diff]"; git --no-pager diff "$T"...HEAD | head -c 60000"#
        )
    } else {
        r#"__IDX=$(mktemp 2>/dev/null || echo "/tmp/blazar-idx-$$")
trap 'rm -f "$__IDX"' EXIT
cp "$(git rev-parse --git-path index)" "$__IDX" 2>/dev/null || rm -f "$__IDX"
export GIT_INDEX_FILE="$__IDX"
git add -N . >/dev/null 2>&1
echo "[stat]"; git --no-pager diff --stat HEAD | tail -40
echo "[diff]"; git --no-pager diff HEAD | head -c 60000"#
            .to_owned()
    };
    let out = match sh(&st, &c.node, format!("{}{}", prelude(&c), gather)).await {
        Ok(o) if o.code == 0 => o.stdout,
        Ok(o) => return fail(StatusCode::CONFLICT, o.stderr.trim()),
        Err(e) => return fail(StatusCode::BAD_GATEWAY, e),
    };
    let material: String = out.chars().take(AI_DIFF_CHARS).collect();
    if !material.contains("diff --git") && !material.contains("- ") {
        return fail(StatusCode::CONFLICT, "没有改动可写");
    }
    let prompt = if pr {
        format!(
            "下面是一条分支（工作区「{}」）相对目标分支的提交、统计和 diff。为它写一个 Pull Request。\n\
             第一行是标题（不超过 70 个字符，不要前缀和句号），空一行，后面是 Markdown 描述：\
             先用一两句话说清楚改了什么、为什么，再用要点列出主要改动，最后如有需要列出验证方式。\n\
             用提交信息本身的语言。只输出标题和描述，不要解释，不要代码围栏。\n\n{material}",
            c.name
        )
    } else {
        format!(
            "下面是一个 git 仓库里还没提交的改动。写一条提交信息：第一行是不超过 70 个字符的概括\
             （祈使语气，不要句号），如果改动不止一处，空一行后用要点列出。\n\
             用代码注释和已有内容里的主要语言。只输出提交信息本身，不要解释，不要代码围栏。\n\n{material}"
        )
    };
    let program = crate::api::local_program(&st, "claude").await;
    let Some(text) = crate::titles::ask_haiku(&program, &prompt, 60).await else {
        return fail(
            StatusCode::BAD_GATEWAY,
            "本机的 Claude 没给出内容（没装、没登录或超时），自己写一段吧",
        );
    };
    let text = text.trim().trim_matches('`').trim();
    let (title, body) = match text.split_once('\n') {
        Some((t, rest)) => (t.trim(), rest.trim()),
        None => (text, ""),
    };
    Json(json!({
        "title": title.trim_start_matches('#').trim(),
        "body": body,
        "text": text,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_that_could_be_options_or_garbage_are_rejected() {
        for ok in [
            "main",
            "origin/main",
            "feat/登录-超时",
            "release-1.2",
            "a_b.c",
        ] {
            assert!(valid_ref(ok), "{ok}");
        }
        for bad in [
            "",
            "-rf",
            "--upload-pack=x",
            "a b",
            "a..b",
            "a:b",
            "x/",
            "x.lock",
            "a\tb",
            "a~1",
        ] {
            assert!(!valid_ref(bad), "{bad}");
        }
    }

    #[test]
    fn status_output_is_parsed_into_sections() {
        let raw = "branch=blazar/fix-login\nhead=abc1234\ntarget=main\ntarget_guess=1\ntarget_ok=1\n\
                   counts=2\t5\ntarget_local=1\nupstream=origin/blazar/fix-login\nupcounts=0\t1\n\
                   remote=git@github.com:acme/app.git\nop=rebase\ngh=1\n__STATUS__\n\
                   UU src/a.rs\n M src/b.rs\n?? notes.md\n__COMMITS__\n\
                   abc1234\tAlice\t1758000000\tfix: 登录\t超时\n";
        let v = parse_status(raw);
        assert_eq!(v["branch"], "blazar/fix-login");
        assert_eq!(v["ahead"], 5);
        assert_eq!(v["behind"], 2);
        assert_eq!(v["up_ahead"], 1);
        assert_eq!(v["op"], "rebase");
        assert_eq!(v["uncommitted"], 3);
        assert_eq!(v["untracked"], 1);
        assert_eq!(v["conflicts"], json!(["src/a.rs"]));
        assert_eq!(v["target_guess"], true);
        assert_eq!(v["same"], false);

        assert_eq!(v["commits"][0]["subject"], "fix: 登录\t超时");
        assert!(
            v["web"]["new_pr"]
                .as_str()
                .is_some_and(|u| u.contains("github.com/acme/app"))
        );
    }

    #[test]
    fn not_a_branch_means_detached() {
        let v = parse_status("branch=\nhead=abc\ntarget=main\n__STATUS__\n__COMMITS__\n");
        assert_eq!(v["detached"], true);
        assert_eq!(v["op"], Value::Null);
        assert_eq!(v["uncommitted"], 0);
    }

    #[test]
    fn unknown_ops_and_bad_input_are_refused() {
        assert!(matches!(
            op_script("reset-hard", &OpBody::default()),
            Ok(None)
        ));
        assert!(op_script("commit", &OpBody::default()).is_err());
        assert!(
            op_script(
                "rename-branch",
                &OpBody {
                    name: Some("--force".into()),
                    ..OpBody::default()
                }
            )
            .is_err()
        );
        assert!(op_script("pr-create", &OpBody::default()).is_err());
    }

    #[test]
    fn user_text_is_quoted_not_interpolated() {
        let s = op_script(
            "commit",
            &OpBody {
                message: Some("it's $(rm -rf ~) `x`".into()),
                ..OpBody::default()
            },
        )
        .unwrap()
        .unwrap();
        assert!(s.contains(r"'it'\''s $(rm -rf ~) `x`'"));
        let forced = op_script(
            "push",
            &OpBody {
                force: true,
                ..OpBody::default()
            },
        )
        .unwrap()
        .unwrap();

        assert!(forced.contains("--force-with-lease") && !forced.contains("--force "));
    }

    #[test]
    fn git_output_is_tidied_for_humans() {
        let raw = "Rebasing (1/3)\rAuto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\n\
                   hint: Resolve all conflicts manually\n__MERGED__\n\n";
        assert_eq!(
            tidy(raw),
            "Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt"
        );
    }

    #[test]
    fn pr_url_is_found_in_both_success_and_already_exists_output() {
        assert_eq!(
            find_pr_url("Creating pull request…\nhttps://github.com/acme/app/pull/42\n").as_deref(),
            Some("https://github.com/acme/app/pull/42")
        );
        assert_eq!(
            find_pr_url("a pull request for branch \"x\" already exists:\nhttps://github.com/acme/app/pull/7")
                .as_deref(),
            Some("https://github.com/acme/app/pull/7")
        );
        assert_eq!(find_pr_url("error: not a github repo"), None);
        assert_eq!(
            pr_number_of("https://github.com/acme/app/pull/42"),
            Some(42)
        );
    }

    #[test]
    fn pr_checks_are_rolled_up() {
        let raw = r#"{"number":42,"url":"https://github.com/acme/app/pull/42","state":"OPEN",
            "title":"Fix login","isDraft":true,"mergeable":"MERGEABLE","reviewDecision":"",
            "statusCheckRollup":[
              {"status":"COMPLETED","conclusion":"SUCCESS"},
              {"status":"COMPLETED","conclusion":"FAILURE"},
              {"status":"IN_PROGRESS","conclusion":""},
              {"state":"SUCCESS"}]}"#;
        let pr = parse_pr(raw).unwrap();
        assert_eq!(pr["number"], 42);
        assert_eq!(pr["draft"], true);
        assert_eq!(
            pr["checks"],
            json!({ "pass": 2, "failed": 1, "pending": 1 })
        );
        assert!(parse_pr("__NOPR__").is_none());
    }
}
