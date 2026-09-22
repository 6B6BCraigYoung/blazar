use std::sync::Arc;

use blazar_transport::{ExecSpec, NodeTransport};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("传输失败: {0}")]
    Transport(#[from] blazar_transport::TransportError),

    #[error("git 操作失败: {0}")]
    Git(String),

    #[error("名称非法: {0}")]
    BadName(String),
}

pub type Result<T> = std::result::Result<T, WorktreeError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEnv {
    pub worktree: String,

    pub branch: String,

    pub base_commit: String,

    pub tmpdir: String,

    pub config_root: String,
}

impl TaskEnv {
    #[must_use]
    pub fn env_vars(&self) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("TMPDIR".into(), self.tmpdir.clone());
        m.insert("TMP".into(), self.tmpdir.clone());
        m.insert("TEMP".into(), self.tmpdir.clone());

        m
    }
}

const GIT_REF_FORBIDDEN: &[char] = &[' ', '~', '^', ':', '?', '*', '[', ']', '\\', '/', '@'];

#[must_use]
pub fn sanitize_branch(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in raw.chars() {
        let ok = (c.is_alphanumeric() || c == '.' || c == '_')
            && !c.is_control()
            && !GIT_REF_FORBIDDEN.contains(&c);
        if ok {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }

    while out.contains("..") {
        out = out.replace("..", ".");
    }
    let mut trimmed = out.trim_matches(['-', '.']).to_owned();
    while let Some(stripped) = trimmed.strip_suffix(".lock") {
        trimmed = stripped.trim_matches(['-', '.']).to_owned();
    }
    if trimmed.is_empty() {
        return "task".to_owned();
    }

    let cut: String = trimmed.chars().take(48).collect();
    let cut = cut.trim_matches(['-', '.']);

    let cut = cut
        .strip_suffix(".lock")
        .unwrap_or(cut)
        .trim_matches(['-', '.']);
    if cut.is_empty() {
        "task".to_owned()
    } else {
        cut.to_owned()
    }
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BranchInfo {
    pub name: String,

    pub remote_only: bool,

    pub current: bool,
    pub last_commit_at: String,
    pub author: String,
    pub subject: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Pushed {
    pub remote: String,
    pub branch: String,

    pub committed: Option<String>,

    pub error: Option<String>,

    pub web: Option<WebLinks>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WebLinks {
    pub branch: String,
    pub new_pr: String,
}

#[must_use]
pub fn web_links(remote: &str, branch: &str) -> Option<WebLinks> {
    let r = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let (host, path) = if let Some(rest) = r.strip_prefix("git@") {
        rest.split_once(':')?
    } else if let Some(rest) = r
        .strip_prefix("ssh://")
        .or_else(|| r.strip_prefix("https://"))
        .or_else(|| r.strip_prefix("http://"))
    {
        let rest = rest.split_once('@').map_or(rest, |(_, h)| h);
        let (h, p) = rest.split_once('/')?;

        let h = if r.starts_with("ssh://") {
            h.split(':').next()?
        } else {
            h
        };
        (h, p)
    } else {
        return None;
    };
    if host.is_empty() || !path.contains('/') {
        return None;
    }

    let enc: String = branch
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let base = format!("https://{host}/{path}");
    Some(if host.contains("github") {
        WebLinks {
            branch: format!("{base}/tree/{enc}"),
            new_pr: format!("{base}/compare/{enc}?expand=1"),
        }
    } else if host.contains("bitbucket") {
        WebLinks {
            branch: format!("{base}/branch/{enc}"),
            new_pr: format!("{base}/pull-requests/new?source={enc}"),
        }
    } else {
        WebLinks {
            branch: format!("{base}/-/tree/{enc}"),
            new_pr: format!("{base}/-/merge_requests/new?merge_request%5Bsource_branch%5D={enc}"),
        }
    })
}

fn parse_branches(raw: &str) -> Vec<BranchInfo> {
    let current = raw
        .lines()
        .find_map(|l| l.strip_prefix("current="))
        .unwrap_or("")
        .trim()
        .to_owned();
    let mut local_names = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut f = line.splitn(4, '\t');
        let (Some(r), Some(at), Some(who), Some(subj)) = (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let (name, remote_only) = if let Some(n) = r.strip_prefix("refs/heads/") {
            local_names.insert(n.to_owned());
            (n.to_owned(), false)
        } else if let Some(n) = r.strip_prefix("refs/remotes/origin/") {
            if n == "HEAD" {
                continue;
            }
            (format!("origin/{n}"), true)
        } else {
            continue;
        };
        out.push(BranchInfo {
            current: !remote_only && name == current,
            name,
            remote_only,
            last_commit_at: at.to_owned(),
            author: who.to_owned(),
            subject: subj.to_owned(),
        });
    }

    out.retain(|b| !b.remote_only || !local_names.contains(b.name.trim_start_matches("origin/")));
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeState {
    Valid,

    Missing,

    Broken,
}

#[derive(Debug, Clone)]
pub struct Paused {
    pub commit: Option<String>,

    pub worktree_was_missing: bool,
}

#[derive(Debug, Clone)]
pub struct Resumed {
    pub env: TaskEnv,

    pub already_present: bool,

    pub moved_aside: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Destroyed {
    pub branch_kept: Option<String>,
}

pub struct WorktreeManager {
    transport: Arc<dyn NodeTransport>,

    repo: String,

    root: String,
    branch_prefix: String,
}

impl WorktreeManager {
    pub fn new(transport: Arc<dyn NodeTransport>, repo: impl Into<String>) -> Self {
        Self {
            transport,
            repo: repo.into(),
            root: "$HOME/.blazar".to_owned(),
            branch_prefix: "blazar/".to_owned(),
        }
    }

    #[must_use]
    pub fn with_root(mut self, root: impl Into<String>) -> Self {
        self.root = root.into();
        self
    }

    #[must_use]
    pub fn with_branch_prefix(mut self, p: impl Into<String>) -> Self {
        self.branch_prefix = p.into();
        self
    }

    async fn sh(&self, script: String) -> Result<String> {
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        if out.code != 0 {
            return Err(WorktreeError::Git(format!(
                "退出码 {}: {}",
                out.code,
                out.stderr.trim()
            )));
        }
        Ok(out.stdout)
    }

    pub async fn create(&self, task_name: &str) -> Result<TaskEnv> {
        let slug = sanitize_branch(task_name);
        if slug.is_empty() {
            return Err(WorktreeError::BadName(task_name.to_owned()));
        }
        let branch = format!("{}{slug}", self.branch_prefix);

        let script = format!(
            r#"set -e
ROOT={root}; REPO={repo}
STAMP=$(date +%s)-$$
WT="$ROOT/worktrees/{slug}-$STAMP"
TMP="$ROOT/tmp/{slug}-$STAMP"
CFG="$ROOT/config/{slug}-$STAMP"
mkdir -p "$WT" "$TMP" "$CFG/codex"
chmod 700 "$TMP" "$CFG"
cd "$REPO"
# 先清理已被删除目录但仍登记在册的 worktree，否则它们会一直占着分支
git worktree prune
BASE=$(git rev-parse HEAD)
BR={branch}
if git show-ref --verify --quiet "refs/heads/$BR"; then
  # 同名分支已经存在（上一个同名任务留下的，或者销毁时没删掉的）。
  # **不要挂上去复用**：新任务会从别人的半成品上开始跑，而 BASE 记的是
  # 主仓 HEAD，diff 基线也跟着错了 —— 用户看到的"改动"里混着上一个人的活。
  # 一律另起一个唯一分支。要基于已有分支开工，走显式的 create_from_branch。
  BR="$BR-$STAMP"
fi
git worktree add -b "$BR" "$WT" "$BASE" >/dev/null
echo "worktree=$WT"
echo "branch=$BR"
echo "base=$BASE"
echo "tmpdir=$TMP"
echo "config=$CFG""#,
            root = self.root,
            repo = q(&self.repo),
            slug = slug,
            branch = q(&branch),
        );

        let out = self.sh(script).await?;
        parse_env(&out).ok_or_else(|| WorktreeError::Git(format!("无法解析建立结果: {out}")))
    }

    pub async fn diff(&self, env: &TaskEnv) -> Result<String> {
        self.sh(format!(
            "cd {} || exit 9; I=$(mktemp); trap 'rm -f \"$I\"' EXIT; \
             cp \"$(git rev-parse --git-path index)\" \"$I\" 2>/dev/null || rm -f \"$I\"; \
             GIT_INDEX_FILE=\"$I\" git add -N . >/dev/null 2>&1; \
             GIT_INDEX_FILE=\"$I\" git --no-pager diff {}",
            q(&env.worktree),
            q(&env.base_commit)
        ))
        .await
    }

    pub async fn commit(&self, env: &TaskEnv, message: &str) -> Result<Option<String>> {
        let out = self
            .sh(format!(
                r#"cd {wt} 2>/dev/null || exit 9
git add -A
if git diff --cached --quiet; then echo "__NOCHANGE__"; else
  git -c user.name=blazar -c user.email=blazar@local commit -m {msg} --no-verify >/dev/null
  git rev-parse HEAD
fi"#,
                wt = q(&env.worktree),
                msg = q(message),
            ))
            .await?;
        let t = out.trim();
        Ok((t != "__NOCHANGE__" && !t.is_empty()).then(|| t.to_owned()))
    }

    pub async fn pause(&self, env: &TaskEnv) -> Result<Paused> {
        let state = self.probe(env).await?;
        let commit = match state {
            WorktreeState::Valid => self.commit(env, "blazar: 冻结工作区前自动提交").await?,

            WorktreeState::Missing | WorktreeState::Broken => None,
        };

        self.sh(format!(
            r#"set -e
cd {repo} 2>/dev/null || exit 9
if [ -e {wt} ]; then git worktree remove --force {wt} 2>/dev/null || rm -rf {wt}; fi
git worktree prune
# 私有目录跟着走，否则冻结等于没省空间
rm -rf {tmp} {cfg}"#,
            repo = q(&self.repo),
            wt = q(&env.worktree),
            tmp = q(&env.tmpdir),
            cfg = q(&env.config_root),
        ))
        .await?;
        Ok(Paused {
            commit,
            worktree_was_missing: state != WorktreeState::Valid,
        })
    }

    pub async fn resume(&self, env: &TaskEnv) -> Result<Resumed> {
        match self.probe(env).await? {
            WorktreeState::Valid => {
                return Ok(Resumed {
                    env: env.clone(),
                    already_present: true,
                    moved_aside: None,
                });
            }
            WorktreeState::Broken | WorktreeState::Missing => {}
        }

        let out = self
            .sh(format!(
                r#"set -e
cd {repo} 2>/dev/null || exit 9
# 分支被主仓 checkout 着时 git worktree add 会以 128 失败，报错只有一句
# "is already checked out"。先查出来，给一句人能看懂的原因。
MAIN_BR=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
if [ "$MAIN_BR" = {br} ]; then
  echo "分支 {br_raw} 正被主仓库 checkout，无法同时挂到 worktree 上。请先在主仓切到别的分支。" >&2
  exit 10
fi
MOVED=""
if [ -e {wt} ]; then
  # 目录在但不是有效 worktree：里面可能有人的东西，挪开而不是删掉
  MOVED={wt}.orphan-$(date +%s)
  mv {wt} "$MOVED"
fi
git worktree prune
mkdir -p {tmp} {cfg}/codex
chmod 700 {tmp} {cfg}
git worktree add {wt} {br} >/dev/null
echo "moved=$MOVED""#,
                wt = q(&env.worktree),
                tmp = q(&env.tmpdir),
                cfg = q(&env.config_root),
                repo = q(&self.repo),
                br = q(&env.branch),
                br_raw = env.branch.replace(['\'', '"'], ""),
            ))
            .await?;
        let moved = out
            .lines()
            .find_map(|l| l.strip_prefix("moved="))
            .filter(|m| !m.trim().is_empty())
            .map(|m| m.trim().to_owned());
        Ok(Resumed {
            env: env.clone(),
            already_present: false,
            moved_aside: moved,
        })
    }

    pub async fn list_branches(&self, fetch: bool) -> Result<Vec<BranchInfo>> {
        let out = self
            .sh(format!(
                r#"cd {repo} 2>/dev/null || exit 9
{fetch}
# 主仓当前 checkout 的分支单独标出来：它不能同时挂到 worktree 上
CUR=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
echo "current=$CUR"
git for-each-ref --sort=-committerdate   --format='%(refname)%09%(committerdate:iso-strict)%09%(authorname)%09%(subject)'   refs/heads refs/remotes/origin"#,
                repo = q(&self.repo),

                fetch = if fetch {
                    "timeout 20 git fetch --prune --quiet origin 2>/dev/null \
                       || gtimeout 20 git fetch --prune --quiet origin 2>/dev/null || true"
                } else {
                    ""
                },
            ))
            .await?;
        Ok(parse_branches(&out))
    }

    pub async fn create_from_branch(&self, branch: &str) -> Result<TaskEnv> {
        let local = branch.strip_prefix("origin/").unwrap_or(branch);
        let slug = sanitize_branch(local);
        let script = format!(
            r#"set -e
ROOT={root}; REPO={repo}
STAMP=$(date +%s)-$$
WT="$ROOT/worktrees/{slug}-$STAMP"
TMP="$ROOT/tmp/{slug}-$STAMP"
CFG="$ROOT/config/{slug}-$STAMP"
cd "$REPO"
git worktree prune
BR={local}
if ! git show-ref --verify --quiet "refs/heads/$BR"; then
  if git show-ref --verify --quiet "refs/remotes/origin/$BR"; then
    git branch --track "$BR" "origin/$BR" >/dev/null
  else
    echo "分支 $BR 在本地和 origin 上都不存在" >&2; exit 11
  fi
fi
# 一个分支同一时间只能挂在一个工作树上。被占着就说清楚是被谁占着
HOLDER=$(git worktree list --porcelain | awk -v b="branch refs/heads/$BR"   '/^worktree /{{w=$2}} $0==b{{print w}}')
if [ -n "$HOLDER" ]; then
  echo "分支 $BR 已经挂在 $HOLDER 上了（可能是主仓库本身，也可能是另一个工作区）" >&2
  exit 12
fi
mkdir -p "$WT" "$TMP" "$CFG/codex"
chmod 700 "$TMP" "$CFG"
git worktree add "$WT" "$BR" >/dev/null
BASE=$(git -C "$WT" rev-parse HEAD)
echo "worktree=$WT"
echo "branch=$BR"
echo "base=$BASE"
echo "tmpdir=$TMP"
echo "config=$CFG""#,
            root = self.root,
            repo = q(&self.repo),
            slug = slug,
            local = q(local),
        );
        let out = self.sh(script).await?;
        parse_env(&out).ok_or_else(|| WorktreeError::Git(format!("无法解析建立结果: {out}")))
    }

    pub async fn push(&self, env: &TaskEnv) -> Result<Pushed> {
        let committed = if self.probe(env).await? == WorktreeState::Valid {
            self.commit(env, "blazar: 推送前自动提交").await?
        } else {
            None
        };
        let out = self
            .sh(format!(
                r#"cd {repo} 2>/dev/null || exit 9
URL=$(git remote get-url origin 2>/dev/null) || {{ echo "这个仓库没有配置 origin 远端" >&2; exit 13; }}
echo "remote=$URL"
# -u 顺手建立跟踪关系；超时兜底，免得远端不可达时把调用方挂住
T=""; command -v timeout >/dev/null 2>&1 && T="timeout 60"
command -v gtimeout >/dev/null 2>&1 && [ -z "$T" ] && T="gtimeout 60"
if ERR=$($T git push -u origin {br} 2>&1); then
  echo "pushed=1"
else
  echo "pushed=0"
  echo "error=$ERR" | tr '
' ' '; echo
fi"#,
                repo = q(&self.repo),
                br = q(&env.branch),
            ))
            .await?;
        let get = |k: &str| {
            out.lines()
                .find_map(|l| l.strip_prefix(k))
                .map(|v| v.trim().to_owned())
        };
        let remote = get("remote=").unwrap_or_default();
        let pushed = get("pushed=").as_deref() == Some("1");
        Ok(Pushed {
            web: web_links(&remote, &env.branch),
            remote,
            branch: env.branch.clone(),
            committed,
            error: if pushed { None } else { get("error=") },
        })
    }

    async fn probe(&self, env: &TaskEnv) -> Result<WorktreeState> {
        let out = self
            .sh(format!(
                r#"if [ ! -e {wt} ]; then echo missing
elif git -C {wt} rev-parse --is-inside-work-tree >/dev/null 2>&1; then echo valid
else echo broken; fi"#,
                wt = q(&env.worktree),
            ))
            .await?;
        Ok(match out.trim() {
            "valid" => WorktreeState::Valid,
            "missing" => WorktreeState::Missing,
            _ => WorktreeState::Broken,
        })
    }

    pub async fn destroy(&self, env: &TaskEnv) -> Result<Destroyed> {
        let out = self
            .sh(format!(
                r#"cd {repo} 2>/dev/null || exit 9
git worktree remove --force {wt} >/dev/null 2>&1 || rm -rf {wt}
git worktree prune
rm -rf {tmp} {cfg}
# 分支删不掉时**说出来**，不要 `|| true` 吞掉：用户看到"已销毁"而分支其实还在，
# 下一次派同名任务时它会被当成现成分支。最常见的原因是主仓正 checkout 着它。
MAIN_BR=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
if [ "$MAIN_BR" = {br} ]; then
  echo "branch_kept=主仓库正 checkout 着这个分支"
elif git show-ref --verify --quiet refs/heads/{br_raw}; then
  if ERR=$(git branch -D {br} 2>&1); then echo "branch_kept="
  else echo "branch_kept=$ERR" | tr '\n' ' '; echo; fi
else
  echo "branch_kept="
fi"#,
                repo = q(&self.repo),
                wt = q(&env.worktree),
                br = q(&env.branch),
                br_raw = env.branch.replace(['\'', '"', ' '], ""),
                tmp = q(&env.tmpdir),
                cfg = q(&env.config_root),
            ))
            .await?;
        let kept = out
            .lines()
            .find_map(|l| l.strip_prefix("branch_kept="))
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_owned);
        Ok(Destroyed { branch_kept: kept })
    }

    pub async fn gc(&self, ttl_days: u32) -> Result<u32> {
        let found = survey(self.transport.as_ref(), &self.root).await?;
        let stale: Vec<String> = found
            .iter()
            .filter(|l| l.idle_days >= ttl_days && l.dirty == 0)
            .map(|l| l.leaf.clone())
            .collect();
        let r = sweep(self.transport.as_ref(), &self.root, &stale, false).await?;
        Ok(u32::try_from(r.removed.len()).unwrap_or(u32::MAX))
    }

    pub async fn branches(&self) -> Result<Vec<String>> {
        let out = self
            .sh(format!(
                "cd {} && git for-each-ref --format='%(refname:short)' refs/heads/{}",
                q(&self.repo),
                self.branch_prefix.trim_end_matches('/')
            ))
            .await?;
        Ok(out
            .lines()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Leftover {
    pub leaf: String,
    pub worktree: Option<String>,
    pub has_tmp: bool,
    pub has_config: bool,

    pub repo: Option<String>,
    pub branch: Option<String>,

    pub dirty: u32,

    pub size_kb: u64,

    pub idle_days: u32,

    pub broken: bool,
}

pub async fn survey(transport: &dyn NodeTransport, root: &str) -> Result<Vec<Leftover>> {
    let out = transport
        .exec(ExecSpec::new("bash").arg("-lc").arg(format!(
            r#"ROOT={root}
[ -d "$ROOT" ] || exit 0
NOW=$(date +%s)
# 三个目录下的名字取并集：只剩 tmp 或只剩 config 的也要列出来
for leaf in $( (ls -1 "$ROOT/worktrees" "$ROOT/tmp" "$ROOT/config" 2>/dev/null) | grep -v ':$' | grep -v '^$' | sort -u ); do
  WT="$ROOT/worktrees/$leaf"; TMP="$ROOT/tmp/$leaf"; CFG="$ROOT/config/$leaf"
  SIZE=$(du -sk "$WT" "$TMP" "$CFG" 2>/dev/null | awk '{{s+=$1}} END {{print s+0}}')
  # 最近一次改动时间：取三者里最新的
  M=0
  for p in "$WT" "$TMP" "$CFG"; do
    [ -e "$p" ] || continue
    t=$(stat -c %Y "$p" 2>/dev/null || stat -f %m "$p" 2>/dev/null || echo 0)
    [ "$t" -gt "$M" ] && M=$t
  done
  IDLE=$(( (NOW - M) / 86400 ))
  REPO=""; BR=""; DIRTY=0; BROKEN=0; HASWT=0
  if [ -e "$WT" ]; then
    HASWT=1
    if git -C "$WT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
      GD=$(sed -n 's/^gitdir: //p' "$WT/.git" 2>/dev/null)
      REPO=${{GD%/.git/worktrees/*}}
      BR=$(git -C "$WT" symbolic-ref --quiet --short HEAD 2>/dev/null)
      DIRTY=$(git -C "$WT" status --porcelain 2>/dev/null | wc -l | tr -d ' ')
    else
      BROKEN=1
      # 坏掉的目录里有文件就当它有人的东西
      DIRTY=$(find "$WT" -type f 2>/dev/null | head -1000 | wc -l | tr -d ' ')
    fi
  fi
  printf 'L	%s	%s	%s	%s	%s	%s	%s	%s	%s	%s
'     "$leaf" "$HASWT" "$([ -e "$TMP" ] && echo 1 || echo 0)" "$([ -e "$CFG" ] && echo 1 || echo 0)"     "$REPO" "$BR" "$DIRTY" "$SIZE" "$IDLE" "$BROKEN"
done"#,
            root = root,
        )))
        .await?;
    if out.code != 0 {
        return Err(WorktreeError::Git(format!(
            "清点残留失败，退出码 {}: {}",
            out.code,
            out.stderr.trim()
        )));
    }
    Ok(parse_survey(root, &out.stdout))
}

fn parse_survey(root: &str, raw: &str) -> Vec<Leftover> {
    raw.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() != 11 || f[0] != "L" {
                return None;
            }
            let n = |s: &str| s.trim().parse::<u64>().unwrap_or(0);
            let opt = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
            Some(Leftover {
                leaf: f[1].to_owned(),
                worktree: (f[2] == "1").then(|| format!("{root}/worktrees/{}", f[1])),
                has_tmp: f[3] == "1",
                has_config: f[4] == "1",
                repo: opt(f[5]),
                branch: opt(f[6]),
                dirty: u32::try_from(n(f[7])).unwrap_or(u32::MAX),
                size_kb: n(f[8]),
                idle_days: u32::try_from(n(f[9])).unwrap_or(u32::MAX),
                broken: f[10] == "1",
            })
        })
        .collect()
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Swept {
    pub removed: Vec<String>,

    pub skipped: Vec<(String, String)>,
}

pub async fn sweep(
    transport: &dyn NodeTransport,
    root: &str,
    leaves: &[String],
    force: bool,
) -> Result<Swept> {
    let found = survey(transport, root).await?;
    let mut r = Swept::default();
    for leaf in leaves {
        if leaf.is_empty() || leaf.contains('/') || leaf == "." || leaf == ".." {
            r.skipped.push((leaf.clone(), "名字不合法".into()));
            continue;
        }
        let Some(l) = found.iter().find(|l| &l.leaf == leaf) else {
            r.skipped.push((leaf.clone(), "已经不在了".into()));
            continue;
        };
        if l.dirty > 0 && !force {
            r.skipped.push((
                leaf.clone(),
                format!("有 {} 处未提交的改动，没动它", l.dirty),
            ));
            continue;
        }
        let deregister = match (&l.repo, &l.worktree) {
            (Some(repo), Some(wt)) => format!(
                "git -C {} worktree remove --force {} 2>/dev/null; git -C {} worktree prune 2>/dev/null;",
                q(repo),
                q(wt),
                q(repo)
            ),
            _ => String::new(),
        };
        let out = transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(format!(
                r#"ROOT={root}
{deregister}
rm -rf "$ROOT/worktrees/"{leaf} "$ROOT/tmp/"{leaf} "$ROOT/config/"{leaf}"#,
                leaf = q(leaf),
            )))
            .await?;
        if out.code == 0 {
            r.removed.push(leaf.clone());
        } else {
            r.skipped.push((leaf.clone(), out.stderr.trim().to_owned()));
        }
    }
    Ok(r)
}

fn parse_env(raw: &str) -> Option<TaskEnv> {
    let mut m = std::collections::HashMap::new();
    for line in raw.lines() {
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim(), v.trim().to_owned());
        }
    }
    Some(TaskEnv {
        worktree: m.remove("worktree")?,
        branch: m.remove("branch")?,
        base_commit: m.remove("base")?,
        tmpdir: m.remove("tmpdir")?,
        config_root: m.remove("config")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_survive_hostile_titles() {
        assert_eq!(sanitize_branch("重构 solver 模块!"), "重构-solver-模块");
        assert_eq!(sanitize_branch("fix: A/B test"), "fix-a-b-test");
        assert_eq!(sanitize_branch("   "), "task");
        assert_eq!(sanitize_branch("--x--"), "x");

        let long = sanitize_branch(&"阈".repeat(200));
        assert_eq!(long.chars().count(), 48);
        assert!(long.chars().all(|c| c == '阈'));
    }

    #[test]
    fn git_hard_rules_on_ref_names_are_enforced() {
        assert_eq!(sanitize_branch("a..b"), "a.b", "ref 里不能出现 ..");
        assert_eq!(sanitize_branch("收敛..实验"), "收敛.实验");
        assert_eq!(sanitize_branch("x.lock"), "x", "ref 不能以 .lock 结尾");
        assert_eq!(sanitize_branch("...."), "task", "清干净后为空要落到兜底名");
        assert!(!sanitize_branch("a...b..c").contains(".."));

        let long = "很长很长".repeat(30);
        let hostile = [
            "a..b",
            "x.lock",
            "收敛..实验",
            "....",
            "..a.lock",
            "....lock",
            "--x--",
            "重构 solver 模块!",
            "fix: A/B test",
            "   ",
            "@{",
            "a\\b",
            "tail.",
            ".head",
            long.as_str(),
        ];
        for t in hostile {
            let b = sanitize_branch(t);
            assert!(!b.is_empty(), "{t:?} 清洗成了空串");
            let full = format!("refs/heads/blazar/{b}");
            let ok = std::process::Command::new("git")
                .args(["check-ref-format", &full])
                .status()
                .map(|s| s.success())
                .unwrap_or(true);
            assert!(ok, "git 拒绝了 {t:?} 清洗出来的分支名 {b:?}");
        }
    }

    #[test]
    fn different_chinese_titles_get_different_branches() {
        let a = sanitize_branch("收敛阈值实验");
        let b = sanitize_branch("并行度实验");
        assert_ne!(a, b);
        assert_ne!(a, "task");
        assert_ne!(b, "task");
    }

    #[test]
    fn git_forbidden_characters_are_stripped() {
        for bad in [
            "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b", "a@b", "a/b",
        ] {
            let s = sanitize_branch(bad);
            assert!(
                !s.chars().any(|c| GIT_REF_FORBIDDEN.contains(&c)),
                "{bad} 清洗后仍含非法字符: {s}"
            );
        }
    }

    #[test]
    fn branch_never_ends_with_dot() {
        assert!(!sanitize_branch("v1.0.").ends_with('.'));
        assert!(!sanitize_branch("...").ends_with('.'));
    }

    #[test]
    fn branch_name_has_no_path_separators() {
        assert!(!sanitize_branch("feat/some thing").contains('/'));
    }

    #[test]
    fn env_vars_isolate_tmp_but_keep_the_machines_own_login() {
        let env = TaskEnv {
            worktree: "/w".into(),
            branch: "blazar/x".into(),
            base_commit: "abc".into(),
            tmpdir: "/t/x".into(),
            config_root: "/c/x".into(),
        };
        let v = env.env_vars();

        assert_eq!(v["TMPDIR"], "/t/x");
        assert_eq!(v["TMP"], "/t/x");
        assert_eq!(v["TEMP"], "/t/x");

        assert!(!v.contains_key("CODEX_HOME"), "{v:?}");
    }

    #[test]
    fn parses_creation_output() {
        let raw = "worktree=/home/c/.blazar/worktrees/x-1\nbranch=blazar/x\n\
                   base=deadbeef\ntmpdir=/home/c/.blazar/tmp/x-1\nconfig=/home/c/.blazar/config/x-1\n";
        let e = parse_env(raw).unwrap();
        assert_eq!(e.branch, "blazar/x");
        assert_eq!(e.base_commit, "deadbeef");
    }

    #[test]
    fn parse_fails_loudly_on_incomplete_output() {
        assert!(parse_env("worktree=/w\nbranch=b\n").is_none());
    }

    fn fake_home_repo(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("blazar-wt-test-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let run = |c: &str| {
            assert!(
                std::process::Command::new("bash")
                    .arg("-lc")
                    .arg(c)
                    .current_dir(&d)
                    .status()
                    .unwrap()
                    .success(),
                "命令失败: {c}"
            );
        };
        run("git init -q . && git config user.email t@t && git config user.name t");
        std::fs::write(d.join("secret.txt"), "私密内容\n").unwrap();
        run("git add -A && git commit -qm init");
        std::fs::write(d.join("secret.txt"), "改过了\n").unwrap();
        d
    }

    #[tokio::test]
    async fn commit_refuses_instead_of_committing_the_home_directory() {
        let home = fake_home_repo("commit-guard");
        let mgr = WorktreeManager::new(
            std::sync::Arc::new(blazar_transport::LocalTransport),
            home.to_str().unwrap(),
        );
        let env = TaskEnv {
            worktree: "/tmp/blazar-definitely-not-here-xyz".into(),
            branch: "blazar/x".into(),
            base_commit: "HEAD".into(),
            tmpdir: "/tmp/blazar-wt-tmp".into(),
            config_root: "/tmp/blazar-wt-cfg".into(),
        };

        let before = git_head(&home);
        let r = mgr.commit(&env, "不该发生的提交").await;
        assert!(r.is_err(), "worktree 不在时必须报错，而不是换个地方提交");
        assert_eq!(before, git_head(&home), "不能在别的仓库里留下提交");
    }

    #[tokio::test]
    async fn destroy_refuses_instead_of_deleting_someone_elses_branch() {
        let home = fake_home_repo("destroy-guard");
        assert!(
            std::process::Command::new("bash")
                .arg("-lc")
                .arg("git branch -q keepme")
                .current_dir(&home)
                .status()
                .unwrap()
                .success()
        );
        let mgr = WorktreeManager::new(
            std::sync::Arc::new(blazar_transport::LocalTransport),
            "/tmp/blazar-definitely-not-a-repo-xyz",
        );
        let env = TaskEnv {
            worktree: "/tmp/blazar-nope".into(),
            branch: "keepme".into(),
            base_commit: "HEAD".into(),
            tmpdir: "/tmp/blazar-wt-tmp2".into(),
            config_root: "/tmp/blazar-wt-cfg2".into(),
        };
        assert!(
            mgr.destroy(&env).await.is_err(),
            "仓库目录不在时必须报错，而不是跑到别处去 git branch -D"
        );
    }

    fn git_head(dir: &std::path::Path) -> String {
        String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
    }

    fn real_repo(name: &str) -> (std::path::PathBuf, WorktreeManager) {
        let base = std::env::temp_dir().join(format!("blazar-wt-e2e-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh_in(
            &repo,
            "git init -q -b main . && git config user.email t@t && git config user.name t",
        );
        std::fs::write(repo.join("a.txt"), "v1\n").unwrap();
        sh_in(&repo, "git add -A && git commit -qm init");
        let mgr = WorktreeManager::new(
            std::sync::Arc::new(blazar_transport::LocalTransport),
            repo.to_str().unwrap(),
        )
        .with_root(base.join("root").to_str().unwrap());
        (repo, mgr)
    }

    fn sh_in(dir: &std::path::Path, cmd: &str) {
        let ok = std::process::Command::new("bash")
            .arg("-lc")
            .arg(cmd)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "命令失败: {cmd}");
    }

    #[tokio::test]
    async fn a_new_task_never_inherits_a_stale_same_name_branch() {
        let (repo, mgr) = real_repo("stale-branch");
        sh_in(
            &repo,
            "git branch blazar/修复 && git checkout -q blazar/修复 \
                      && echo 别人的半成品 > b.txt && git add -A && git commit -qm wip \
                      && git checkout -q main",
        );
        let env = mgr.create("修复").await.unwrap();
        assert_ne!(env.branch, "blazar/修复", "不能复用残留的同名分支");
        assert!(
            !std::path::Path::new(&env.worktree).join("b.txt").exists(),
            "新任务的目录里不该出现上一个分支的文件"
        );
    }

    #[tokio::test]
    async fn pause_survives_a_worktree_that_was_deleted_by_hand() {
        let (_repo, mgr) = real_repo("pause-missing");
        let env = mgr.create("任务").await.unwrap();
        std::fs::remove_dir_all(&env.worktree).unwrap();

        let p = mgr.pause(&env).await.expect("目录没了也要能冻结");
        assert!(p.worktree_was_missing, "要如实报告目录已经不在了");
        assert!(p.commit.is_none());
    }

    #[tokio::test]
    async fn resume_leaves_a_live_worktree_and_its_uncommitted_work_alone() {
        let (_repo, mgr) = real_repo("resume-live");
        let env = mgr.create("任务").await.unwrap();
        let wip = std::path::Path::new(&env.worktree).join("未提交.txt");
        std::fs::write(&wip, "千万别丢").unwrap();

        let r = mgr.resume(&env).await.unwrap();
        assert!(r.already_present);
        assert_eq!(std::fs::read_to_string(&wip).unwrap(), "千万别丢");
    }

    #[tokio::test]
    async fn resume_moves_a_broken_worktree_aside_instead_of_destroying_it() {
        let (_repo, mgr) = real_repo("resume-broken");
        let env = mgr.create("任务").await.unwrap();
        mgr.pause(&env).await.unwrap();
        std::fs::create_dir_all(&env.worktree).unwrap();
        std::fs::write(
            std::path::Path::new(&env.worktree).join("手写的.txt"),
            "重要",
        )
        .unwrap();

        let r = mgr.resume(&env).await.expect("坏目录要能自愈");
        let moved = r.moved_aside.expect("要告诉调用方东西挪去了哪");
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&moved).join("手写的.txt")).unwrap(),
            "重要",
            "挪开的东西必须原样还在"
        );
        assert!(
            std::path::Path::new(&env.worktree).join("a.txt").exists(),
            "原位置要重建出有效的 worktree"
        );
    }

    #[tokio::test]
    async fn resume_explains_when_the_main_repo_holds_the_branch() {
        let (repo, mgr) = real_repo("resume-held");
        let env = mgr.create("任务").await.unwrap();
        mgr.pause(&env).await.unwrap();
        sh_in(&repo, &format!("git checkout -q '{}'", env.branch));

        let err = mgr.resume(&env).await.unwrap_err().to_string();
        assert!(
            err.contains("主仓库"),
            "原因要说人话，而不是 git 的 128: {err}"
        );
    }

    #[tokio::test]
    async fn destroy_reports_a_branch_it_could_not_delete() {
        let (repo, mgr) = real_repo("destroy-held");
        let env = mgr.create("任务").await.unwrap();
        mgr.pause(&env).await.unwrap();
        sh_in(&repo, &format!("git checkout -q '{}'", env.branch));

        let d = mgr.destroy(&env).await.unwrap();
        let why = d.branch_kept.expect("分支没删掉必须说出来");
        assert!(why.contains("主仓库"), "{why}");

        sh_in(&repo, "git checkout -q main");
        let env2 = mgr.create("另一个").await.unwrap();
        let d2 = mgr.destroy(&env2).await.unwrap();
        assert!(d2.branch_kept.is_none(), "{:?}", d2.branch_kept);
    }

    fn repo_with_origin(name: &str) -> (std::path::PathBuf, std::path::PathBuf, WorktreeManager) {
        let base = std::env::temp_dir().join(format!("blazar-wt-origin-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let origin = base.join("origin.git");
        let other = base.join("colleague");
        let repo = base.join("repo");
        sh_in(&base, "git init -q --bare -b main origin.git");
        sh_in(&base, "git clone -q origin.git colleague 2>/dev/null");
        sh_in(
            &other,
            "git config user.email c@c && git config user.name 同事 \
                       && echo v1 > a.txt && git add -A && git commit -qm init && git push -q origin main \
                       && git checkout -q -b feat/同事的活 && echo wip > b.txt && git add -A \
                       && git commit -qm 同事的半成品 && git push -q origin feat/同事的活",
        );
        sh_in(&base, "git clone -q origin.git repo 2>/dev/null");
        sh_in(&repo, "git config user.email t@t && git config user.name t");
        let mgr = WorktreeManager::new(
            std::sync::Arc::new(blazar_transport::LocalTransport),
            repo.to_str().unwrap(),
        )
        .with_root(base.join("root").to_str().unwrap());
        (origin, repo, mgr)
    }

    #[tokio::test]
    async fn branches_come_newest_first_and_include_remote_only_ones() {
        let (_o, _repo, mgr) = repo_with_origin("list");
        let bs = mgr.list_branches(true).await.unwrap();
        let names: Vec<_> = bs.iter().map(|b| b.name.as_str()).collect();
        let feat = bs
            .iter()
            .find(|b| b.name == "origin/feat/同事的活")
            .expect("只在 origin 上的分支必须列出来 —— 接着同事的活往下做就是从它开始");
        assert!(feat.remote_only);
        assert_eq!(feat.author, "同事");
        assert!(
            bs.iter().any(|b| b.name == "main" && b.current),
            "主仓当前分支要标出来: {names:?}"
        );
        assert!(
            !names.contains(&"origin/main"),
            "本地已有的不要再列一遍远程副本: {names:?}"
        );
        assert!(!names.iter().any(|n| n.ends_with("HEAD")), "{names:?}");
    }

    #[tokio::test]
    async fn a_workspace_can_start_from_a_colleagues_remote_branch() {
        let (_o, _repo, mgr) = repo_with_origin("from-remote");
        let env = mgr
            .create_from_branch("origin/feat/同事的活")
            .await
            .unwrap();
        assert_eq!(
            env.branch, "feat/同事的活",
            "agent 的提交要落在那个分支本身上"
        );
        assert!(
            std::path::Path::new(&env.worktree).join("b.txt").exists(),
            "工作区里要有同事已经做的东西"
        );

        let tip = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&env.worktree)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert_eq!(env.base_commit, tip.trim());
    }

    #[tokio::test]
    async fn starting_from_a_branch_that_is_already_checked_out_explains_who_holds_it() {
        let (_o, _repo, mgr) = repo_with_origin("held");
        let err = mgr
            .create_from_branch("main")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("已经挂在"),
            "要说清楚被谁占着，而不是 git 的 128: {err}"
        );
    }

    #[tokio::test]
    async fn push_commits_pending_work_and_lands_the_branch_on_origin() {
        let (origin, _repo, mgr) = repo_with_origin("push");
        let env = mgr.create("推送测试").await.unwrap();
        std::fs::write(
            std::path::Path::new(&env.worktree).join("成果.txt"),
            "done\n",
        )
        .unwrap();

        let p = mgr.push(&env).await.unwrap();
        assert!(p.error.is_none(), "{:?}", p.error);
        assert!(p.committed.is_some(), "没提交的改动要先提交再推");
        let on_origin = std::process::Command::new("git")
            .args([
                "show-ref",
                "--verify",
                &format!("refs/heads/{}", env.branch),
            ])
            .current_dir(&origin)
            .status()
            .unwrap()
            .success();
        assert!(on_origin, "分支要真的出现在 origin 上");
    }

    #[test]
    fn web_links_cover_the_common_remote_url_shapes() {
        let gh = web_links("git@github.com:acme/blazar.git", "blazar/修复").unwrap();
        assert_eq!(
            gh.new_pr,
            "https://github.com/acme/blazar/compare/blazar%2F%E4%BF%AE%E5%A4%8D?expand=1"
        );

        let gl = web_links("ssh://git@10.99.0.9:2222/team/sub/proj.git", "x").unwrap();
        assert!(
            gl.branch
                .starts_with("https://10.99.0.9/team/sub/proj/-/tree/"),
            "{gl:?}"
        );
        assert!(gl.new_pr.contains("merge_requests/new"), "{gl:?}");
        let https = web_links("https://gitlab.corp/team/proj.git", "x").unwrap();
        assert_eq!(https.branch, "https://gitlab.corp/team/proj/-/tree/x");

        assert_eq!(web_links("/srv/git/proj.git", "x"), None);
        assert_eq!(web_links("", "x"), None);
    }

    #[tokio::test]
    async fn survey_finds_everything_on_disk_and_flags_dirty_ones() {
        let (repo, mgr) = real_repo("survey");
        let clean = mgr.create("干净的").await.unwrap();
        let dirty = mgr.create("有活的").await.unwrap();
        std::fs::write(
            std::path::Path::new(&dirty.worktree).join("没提交.txt"),
            "x",
        )
        .unwrap();

        let root = repo.parent().unwrap().join("root");
        std::fs::create_dir_all(root.join("tmp/孤零零的-1")).unwrap();

        let t = blazar_transport::LocalTransport;
        let found = survey(&t, root.to_str().unwrap()).await.unwrap();
        let by = |w: &TaskEnv| {
            let leaf = w.worktree.rsplit('/').next().unwrap();
            found.iter().find(|l| l.leaf == leaf).cloned().unwrap()
        };
        assert_eq!(by(&clean).dirty, 0);
        assert!(by(&dirty).dirty > 0, "有未提交改动的要标出来");
        assert!(by(&clean).branch.is_some() && by(&clean).repo.is_some());
        let orphan = found
            .iter()
            .find(|l| l.leaf == "孤零零的-1")
            .expect("只剩 tmp 的残片也要列出来");
        assert!(orphan.worktree.is_none() && orphan.has_tmp);
    }

    #[tokio::test]
    async fn sweep_leaves_uncommitted_work_alone_unless_forced() {
        let (repo, mgr) = real_repo("sweep");
        let clean = mgr.create("干净的").await.unwrap();
        let dirty = mgr.create("有活的").await.unwrap();
        std::fs::write(
            std::path::Path::new(&dirty.worktree).join("没提交.txt"),
            "x",
        )
        .unwrap();
        let root = repo.parent().unwrap().join("root");
        let root = root.to_str().unwrap();
        let leaf = |w: &TaskEnv| w.worktree.rsplit('/').next().unwrap().to_owned();
        let t = blazar_transport::LocalTransport;

        let r = sweep(&t, root, &[leaf(&clean), leaf(&dirty)], false)
            .await
            .unwrap();
        assert_eq!(r.removed, vec![leaf(&clean)]);
        assert!(
            r.skipped
                .iter()
                .any(|(l, why)| *l == leaf(&dirty) && why.contains("未提交"))
        );
        assert!(!std::path::Path::new(&clean.worktree).exists());
        assert!(
            !std::path::Path::new(&clean.tmpdir).exists(),
            "私有目录要一起清掉"
        );
        assert!(
            std::path::Path::new(&dirty.worktree)
                .join("没提交.txt")
                .exists()
        );

        let list = String::from_utf8(
            std::process::Command::new("git")
                .args(["worktree", "list", "--porcelain"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(!list.contains(&clean.worktree), "主仓里还登记着: {list}");

        let r = sweep(&t, root, &[leaf(&dirty)], true).await.unwrap();
        assert_eq!(r.removed, vec![leaf(&dirty)], "force 才会动有改动的");
    }

    #[tokio::test]
    async fn sweep_refuses_names_that_escape_the_root() {
        let (repo, _mgr) = real_repo("sweep-escape");
        let root = repo.parent().unwrap().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let t = blazar_transport::LocalTransport;
        let r = sweep(
            &t,
            root.to_str().unwrap(),
            &["../repo".into(), "..".into(), String::new()],
            true,
        )
        .await
        .unwrap();
        assert!(r.removed.is_empty(), "不能借清扫之名删到别处去: {r:?}");
        assert!(repo.join("a.txt").exists());
    }
}
