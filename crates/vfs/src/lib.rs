use std::path::Path;
use std::sync::Arc;

use blazar_transport::{ExecSpec, NodeTransport};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("传输失败: {0}")]
    Transport(#[from] blazar_transport::TransportError),

    #[error("路径逃逸出工作区: {0}")]
    PathEscape(String),

    #[error("不是 git 仓库: {0}")]
    NotARepo(String),
}

pub type Result<T> = std::result::Result<T, VfsError>;

pub const MAX_TREE_ENTRIES: usize = 20_000;

const EXIT_NO_DIR: i32 = 3;

const EXIT_NO_GIT: i32 = 5;

const EXIT_SSH: i32 = 255;

fn check(out: &blazar_transport::ExecOutput, what: &str) -> Result<()> {
    if out.code == 0 {
        return Ok(());
    }
    let stderr = out.stderr.trim();
    let detail = match out.code {
        EXIT_SSH => format!("机器连不上：{stderr}"),
        EXIT_NO_DIR => format!("目录不存在或无权限访问（{what}）"),
        EXIT_NO_GIT => format!("不是 git 仓库，没有可比较的基线（{what}）"),
        c => format!("退出码 {c}：{stderr}"),
    };
    Err(blazar_transport::TransportError::Command {
        code: out.code,
        stderr: detail,
    }
    .into())
}

const TEMP_INDEX: &str = r#"__IDX=$(mktemp 2>/dev/null || echo "/tmp/blazar-idx-$$")
trap 'rm -f "$__IDX"' EXIT
# 还没提交过的仓库里 index 文件根本不存在。这时不能留一个空文件 ——
# git 会以 "index file smaller than expected" 拒收；删掉它，git 会当作空 index
cp "$(git rev-parse --git-path index)" "$__IDX" 2>/dev/null || rm -f "$__IDX"
export GIT_INDEX_FILE="$__IDX"
git add -N . >/dev/null 2>&1"#;

const REMOTE_SEARCH_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub is_dir: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<ChangeKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Untracked,
}

impl ChangeKind {
    fn from_porcelain(code: &str) -> Self {
        match code.trim() {
            "??" => Self::Untracked,
            c if c.contains('A') => Self::Added,
            c if c.contains('D') => Self::Deleted,
            _ => Self::Modified,
        }
    }
}

pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub size: u64,

    pub too_large: bool,

    pub binary: bool,

    #[serde(default)]
    pub mtime: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Written {
    pub saved: bool,

    pub mtime: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,

    pub path: String,

    pub is_repo: bool,

    pub children: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String,

    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,

    pub is_repo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiffOpts {
    pub merge_base: bool,

    pub ignore_ws: bool,
}

pub struct Vfs {
    transport: Arc<dyn NodeTransport>,
    root: String,
}

impl Vfs {
    pub fn new(transport: Arc<dyn NodeTransport>, root: impl Into<String>) -> Self {
        Self {
            transport,
            root: root.into(),
        }
    }

    fn safe_rel(&self, rel: &str) -> Result<String> {
        let p = Path::new(rel);
        if p.is_absolute()
            || rel.is_empty()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(VfsError::PathEscape(rel.to_owned()));
        }
        Ok(rel.replace('\\', "/"))
    }

    pub async fn tree(&self, base: Option<&str>) -> Result<Vec<TreeEntry>> {
        Ok(self.tree_with_stat(base).await?.0)
    }

    pub async fn tree_with_stat(
        &self,
        base: Option<&str>,
    ) -> Result<(Vec<TreeEntry>, Option<DiffStat>)> {
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
if git rev-parse --git-dir >/dev/null 2>&1; then
  echo "__TRACKED__"
  git ls-files -z | tr '\0' '\n'
  echo "__STATUS__"
  # -uall 让未跟踪目录展开到文件，否则只显示目录名。
  # core.quotepath=false 不能省：默认 git 会把非 ASCII 路径转义成
  # "\346\226\207..." 这种八进制串，而上面的 `ls-files -z` 不转义，
  # 两份路径对不上 —— 结果是中文文件的改动标记挂到一个幽灵路径上，
  # 真文件反而显示"没改过"。中文文件名在这里是常态不是边角情况。
  git -c core.quotepath=false status --porcelain -uall {base}
  # 改动量：同一次往返里顺手算掉，列表页就不必为每个工作区再跑一趟远程
  echo "__STAT__"
  (
    {temp_index}
    B=HEAD; git rev-parse --verify --quiet HEAD >/dev/null || B=$(git hash-object -t tree /dev/null)
    # 二进制文件在 numstat 里是 "-"，awk 把它当 0，正好
    git diff --numstat "$B" 2>/dev/null | awk '{{a+=$1; d+=$2; f++}} END {{print a+0, d+0, f+0}}'
  )
else
  echo "__PLAIN__"
  find . -type f -not -path '*/.git/*' -not -path '*/node_modules/*' \
       -not -path '*/target/*' -not -path '*/.venv/*' 2>/dev/null \
    | sed 's|^\./||' | head -{cap}
fi"#,
            root = shell_quote(&self.root),
            base = base.map(shell_quote).unwrap_or_default(),
            cap = MAX_TREE_ENTRIES,
            temp_index = TEMP_INDEX,
        );

        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        check(&out, &self.root)?;
        Ok((parse_tree(&out.stdout), parse_stat(&out.stdout)))
    }

    pub async fn read(&self, rel: &str) -> Result<FileContent> {
        let rel = self.safe_rel(rel)?;
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
F={file}
[ -f "$F" ] || exit 4
SZ=$(wc -c < "$F" | tr -d ' ')
echo "SIZE|$SZ"
# GNU 与 BSD 的 stat 参数不一样
echo "MTIME|$(stat -c %Y "$F" 2>/dev/null || stat -f %m "$F" 2>/dev/null)"
# 前 8KB 里出现 NUL 就当二进制 —— 比调 file(1) 可靠且到处都有。
# LC_ALL=C 不能省：BSD tr 在 UTF-8 locale 下，遇到被 head -c 从中间
# 切断的多字节字符会以 "Illegal byte sequence" 中止并少吐几个字节，
# 于是每一个中文文件都会被算成"含 NUL"，全部当二进制拒之门外。
HEAD_BYTES=$(head -c 8192 "$F" | wc -c | tr -d ' ')
NUL_STRIPPED=$(head -c 8192 "$F" | LC_ALL=C tr -d '\000' | wc -c | tr -d ' ')
if [ "$NUL_STRIPPED" -ne "$HEAD_BYTES" ]; then
  echo "BINARY|1"
elif [ "$SZ" -gt {max} ]; then
  echo "TOOLARGE|1"
else
  echo "BODY|"
  cat -- "$F"
fi"#,
            root = shell_quote(&self.root),
            file = shell_quote(&rel),
            max = MAX_READ_BYTES,
        );

        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        if out.code == 4 {
            return Err(blazar_transport::TransportError::Command {
                code: 4,
                stderr: format!("{rel} 不是一个文件"),
            }
            .into());
        }
        check(&out, &rel)?;
        Ok(parse_file(&rel, &out.stdout))
    }

    pub async fn read_base64(&self, rel: &str, max: u64) -> Result<String> {
        let rel = self.safe_rel(rel)?;
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
F={file}
[ -f "$F" ] || exit 4
[ "$(wc -c < "$F" | tr -d ' ')" -le {max} ] || exit 8
base64 < "$F" | tr -d '\n'"#,
            root = shell_quote(&self.root),
            file = shell_quote(&rel),
        );
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        if out.code == 4 || out.code == 8 {
            return Err(blazar_transport::TransportError::Command {
                code: out.code,
                stderr: if out.code == 4 {
                    format!("{rel} 不是一个文件")
                } else {
                    format!("{rel} 太大了")
                },
            }
            .into());
        }
        check(&out, &rel)?;
        Ok(out.stdout.trim().to_owned())
    }

    pub async fn write(
        &self,
        rel: &str,
        content: &str,
        expect_mtime: Option<u64>,
    ) -> Result<Written> {
        let rel = self.safe_rel(rel)?;

        if rel == ".git" || rel.starts_with(".git/") || rel.contains("/.git/") {
            return Err(VfsError::PathEscape(rel));
        }
        if content.len() as u64 > MAX_READ_BYTES {
            return Err(blazar_transport::TransportError::Command {
                code: 7,
                stderr: "文件超过 2 MB，不在这里保存".into(),
            }
            .into());
        }
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
F={file}
mt() {{ stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null; }}
[ -d "$F" ] && exit 4
E={expect}
if [ -n "$E" ] && [ -e "$F" ] && [ "$(mt "$F")" != "$E" ]; then
  echo "CONFLICT|$(mt "$F")|$(wc -c < "$F" | tr -d ' ')"; exit 0
fi
mkdir -p "$(dirname "$F")" || exit 6
cat > "$F" || exit 6
echo "SAVED|$(mt "$F")|$(wc -c < "$F" | tr -d ' ')""#,
            root = shell_quote(&self.root),
            file = shell_quote(&rel),
            expect = shell_quote(&expect_mtime.map(|m| m.to_string()).unwrap_or_default()),
        );
        let out = self
            .transport
            .exec(
                ExecSpec::new("bash")
                    .arg("-lc")
                    .arg(script)
                    .stdin(content.as_bytes().to_vec()),
            )
            .await?;
        if out.code == 4 {
            return Err(blazar_transport::TransportError::Command {
                code: 4,
                stderr: format!("{rel} 是一个目录"),
            }
            .into());
        }
        check(&out, &rel)?;
        let line = out.stdout.lines().last().unwrap_or_default();
        let mut p = line.split('|');
        let kind = p.next().unwrap_or_default();
        let mtime = p.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        let size = p.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        Ok(Written {
            saved: kind == "SAVED",
            mtime,
            size,
        })
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
# 远端也要有超时：中心侧断开连接并不会杀掉已经跑起来的 grep，
# 否则一次误搜就会在目标机器上留下一个扫全盘的孤儿进程。
T="timeout -k 2 {secs}"; command -v timeout >/dev/null 2>&1 || T=""
if git rev-parse --git-dir >/dev/null 2>&1; then
  # 只扫受跟踪文件：在 home 这类巨大目录里，这是唯一能秒回的方式
  $T git --no-pager grep -n -I --no-color -e {q} -- . 2>/dev/null | head -n {limit}
elif command -v rg >/dev/null 2>&1; then
  $T rg --line-number --no-heading --color never --max-count 50 --max-filesize 2M \
     --glob '!.git' --glob '!node_modules' --glob '!target' --glob '!.venv' \
     -e {q} . 2>/dev/null | head -n {limit}
else
  $T grep -rn --binary-files=without-match \
       --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=target \
       --exclude-dir=.venv --exclude-dir=.cache \
       -e {q} . 2>/dev/null | head -n {limit}
fi"#,
            root = shell_quote(&self.root),
            q = shell_quote(query),
            limit = limit,
            secs = REMOTE_SEARCH_TIMEOUT_SECS,
        );
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;

        if out.code != 0 && out.code != 1 {
            check(&out, &self.root)?;
        }
        Ok(parse_search(&out.stdout))
    }

    pub async fn browse(&self, path: &str) -> Result<DirListing> {
        let target = cd_target(path);
        let script = format!(
            r#"set -e
cd {target} 2>/dev/null || cd ~ || exit 3
HERE=$(pwd -P)
echo "PATH|$HERE"
if [ "$HERE" != "/" ]; then echo "PARENT|$(dirname "$HERE")"; fi
if [ -d "$HERE/.git" ] || [ -f "$HERE/.git" ]; then echo "SELFREPO|1"; fi
# -maxdepth 1 只看一层；隐藏目录跳过，但 .config 这类用户想进的可以手输路径
for d in */ ; do
  [ -d "$d" ] || continue
  n=${{d%/}}
  case "$n" in .*) continue;; esac
  repo=0
  if [ -d "$HERE/$n/.git" ] || [ -f "$HERE/$n/.git" ]; then repo=1; fi
  # 子目录计数只数一层且不深入，避免在巨大目录上卡住
  cnt=$(find "$HERE/$n" -maxdepth 1 -mindepth 1 -type d ! -name '.*' 2>/dev/null | head -200 | wc -l | tr -d ' ')
  echo "DIR|$n|$HERE/$n|$repo|$cnt"
done"#,
            target = target,
        );

        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        check(&out, &self.root)?;
        Ok(parse_listing(&out.stdout))
    }

    pub async fn diff(&self, base: &str) -> Result<String> {
        self.diff_opts(base, DiffOpts::default()).await
    }

    pub async fn diff_opts(&self, base: &str, opts: DiffOpts) -> Result<String> {
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
git rev-parse --git-dir >/dev/null 2>&1 || exit 5
# `add -N` 把未跟踪文件纳入 diff，否则新建的文件完全不出现 —— 但在临时 index 上做
{temp_index}
B={base}
{merge_base}
# 还没有任何提交时 HEAD 不存在，`git diff HEAD` 会以 128 退出，
# 整页 diff 变成一条 git 的用法提示。退到 git 的空树对象上，
# 让"全部是新增"照常显示出来 —— 新建的仓库正是最常看 diff 的时候。
git rev-parse --verify --quiet "$B^{{commit}}" >/dev/null 2>&1 \
  || B=$(git hash-object -t tree /dev/null)
git --no-pager -c core.quotepath=false diff {ws} "$B""#,
            root = shell_quote(&self.root),
            base = shell_quote(base),
            temp_index = TEMP_INDEX,
            merge_base = if opts.merge_base {
                r#"MB=$(git merge-base "$B" HEAD 2>/dev/null) && B=$MB"#
            } else {
                ""
            },
            ws = if opts.ignore_ws { "-w" } else { "" },
        );
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        if out.code == EXIT_NO_GIT {
            return Err(VfsError::NotARepo(self.root.clone()));
        }
        check(&out, &self.root)?;
        Ok(out.stdout)
    }

    pub async fn head_commit(&self) -> Result<Option<String>> {
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(format!(
                "cd {} && git rev-parse HEAD 2>/dev/null",
                shell_quote(&self.root)
            )))
            .await?;
        let s = out.stdout.trim().to_owned();
        Ok((!s.is_empty()).then_some(s))
    }
}

fn cd_target(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() || p == "~" {
        return "~".to_owned();
    }
    match p.strip_prefix("~/") {
        Some(rest) => format!("~/{}", shell_quote(rest)),
        None => shell_quote(p),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn parse_tree(raw: &str) -> Vec<TreeEntry> {
    use std::collections::BTreeMap;

    let mut files: BTreeMap<String, Option<ChangeKind>> = BTreeMap::new();
    let mut section = "";

    for line in raw.lines() {
        match line.trim_end() {
            "__TRACKED__" | "__PLAIN__" => {
                section = "files";
                continue;
            }
            "__STATUS__" => {
                section = "status";
                continue;
            }

            "__STAT__" => {
                section = "stat";
                continue;
            }
            _ => {}
        }
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        match section {
            "files" => {
                files.insert(line.to_owned(), None);
            }
            "status" => {
                if line.len() < 4 {
                    continue;
                }
                let (code, rest) = line.split_at(2);
                let path = rest.trim();

                let path = path.rsplit(" -> ").next().unwrap_or(path);

                files.insert(
                    unquote_git_path(path),
                    Some(ChangeKind::from_porcelain(code)),
                );
            }
            _ => {}
        }
    }

    let mut dirs: BTreeMap<String, ()> = BTreeMap::new();
    for path in files.keys() {
        let mut acc = String::new();
        let parts: Vec<_> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            dirs.insert(acc.clone(), ());
        }
    }

    let mut out: Vec<TreeEntry> = dirs
        .into_keys()
        .map(|path| TreeEntry {
            path,
            is_dir: true,
            change: None,
        })
        .chain(files.into_iter().map(|(path, change)| TreeEntry {
            path,
            is_dir: false,
            change,
        }))
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn unquote_git_path(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) else {
        return s.to_owned();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut it = inner.bytes();
    while let Some(b) = it.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match it.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'"') => bytes.push(b'"'),
            Some(b'\\') => bytes.push(b'\\'),

            Some(d0 @ b'0'..=b'7') => {
                let mut v = u32::from(d0 - b'0');
                for _ in 0..2 {
                    match it.next() {
                        Some(d @ b'0'..=b'7') => v = v * 8 + u32::from(d - b'0'),

                        Some(other) => {
                            bytes.push(u8::try_from(v).unwrap_or(b'?'));
                            bytes.push(other);
                            v = u32::MAX;
                            break;
                        }
                        None => break,
                    }
                }
                if v != u32::MAX {
                    bytes.push(u8::try_from(v).unwrap_or(b'?'));
                }
            }
            Some(other) => bytes.push(other),
            None => break,
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| s.to_owned())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStat {
    pub added: u64,
    pub removed: u64,
    pub files: u64,
}

#[must_use]
pub fn parse_stat(raw: &str) -> Option<DiffStat> {
    let mut lines = raw.lines();
    lines.find(|l| l.trim_end() == "__STAT__")?;
    let mut it = lines.next()?.split_whitespace().map(str::parse::<u64>);
    Some(DiffStat {
        added: it.next()?.ok()?,
        removed: it.next()?.ok()?,
        files: it.next()?.ok()?,
    })
}

fn parse_file(path: &str, raw: &str) -> FileContent {
    let mut size = 0u64;
    let mut mtime = 0u64;
    let mut binary = false;
    let mut too_large = false;
    let mut content = String::new();

    if let Some(idx) = raw.find("BODY|\n") {
        content = raw[idx + 6..].to_owned();
    }
    for line in raw[..raw.find("BODY|\n").unwrap_or(raw.len())].lines() {
        match line.split_once('|') {
            Some(("SIZE", v)) => size = v.trim().parse().unwrap_or(0),
            Some(("MTIME", v)) => mtime = v.trim().parse().unwrap_or(0),
            Some(("BINARY", _)) => binary = true,
            Some(("TOOLARGE", _)) => too_large = true,
            _ => {}
        }
    }
    FileContent {
        path: path.to_owned(),
        content,
        size,
        too_large,
        binary,
        mtime,
    }
}

fn parse_listing(raw: &str) -> DirListing {
    let mut path = String::new();
    let mut parent = None;
    let mut is_repo = false;
    let mut entries = Vec::new();

    for line in raw.lines() {
        let parts: Vec<&str> = line.trim_end().split('|').collect();
        match parts.as_slice() {
            ["PATH", p] => path = (*p).to_owned(),
            ["PARENT", p] => parent = Some((*p).to_owned()),
            ["SELFREPO", _] => is_repo = true,
            ["DIR", name, full, repo, cnt] => entries.push(DirEntry {
                name: (*name).to_owned(),
                path: (*full).to_owned(),
                is_repo: *repo == "1",
                children: cnt.parse().unwrap_or(0),
            }),
            _ => {}
        }
    }

    entries.sort_by(|a, b| {
        b.is_repo
            .cmp(&a.is_repo)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    DirListing {
        path,
        parent,
        entries,
        is_repo,
    }
}

fn parse_search(raw: &str) -> Vec<SearchHit> {
    raw.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("./").unwrap_or(line);
            let (path, rest) = rest.split_once(':')?;
            let (line_no, text) = rest.split_once(':')?;
            Some(SearchHit {
                path: path.to_owned(),
                line: line_no.parse().ok()?,
                text: text.trim_end().chars().take(400).collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_marks_changes_and_derives_dirs() {
        let raw = "__TRACKED__\nsrc/main.rs\nsrc/lib.rs\nREADME.md\n\
                   __STATUS__\n M src/main.rs\n?? src/new.rs\n";
        let tree = parse_tree(raw);

        let by_path = |p: &str| tree.iter().find(|e| e.path == p).cloned();
        assert_eq!(
            by_path("src/main.rs").unwrap().change,
            Some(ChangeKind::Modified)
        );
        assert_eq!(
            by_path("src/new.rs").unwrap().change,
            Some(ChangeKind::Untracked)
        );
        assert_eq!(by_path("README.md").unwrap().change, None);
        assert!(by_path("src").unwrap().is_dir, "目录节点应自动推导出来");
    }

    #[test]
    fn rename_takes_new_path() {
        let raw = "__TRACKED__\na.rs\n__STATUS__\nR  old.rs -> new.rs\n";
        let tree = parse_tree(raw);
        assert!(tree.iter().any(|e| e.path == "new.rs"));
    }

    #[test]
    fn plain_dir_without_git_still_lists() {
        let raw = "__PLAIN__\nnotes/a.md\nnotes/b.md\n";
        let tree = parse_tree(raw);
        assert_eq!(tree.iter().filter(|e| !e.is_dir).count(), 2);
        assert!(tree.iter().any(|e| e.is_dir && e.path == "notes"));
    }

    #[test]
    fn search_parses_rg_and_grep_shape() {
        let raw = "./src/main.rs:42:    let x = 1;\nsrc/lib.rs:7:fn main() {}\n";
        let hits = parse_search(raw);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "src/main.rs");
        assert_eq!(hits[0].line, 42);
        assert_eq!(hits[1].line, 7);
    }

    #[test]
    fn search_ignores_unparseable_lines() {
        let hits = parse_search("--\nbinary file matches\nsrc/a.rs:1:ok\n");
        assert_eq!(hits.len(), 1);
    }

    fn vfs() -> Vfs {
        Vfs::new(Arc::new(blazar_transport::LocalTransport), "/tmp/ws")
    }

    fn out(code: i32, stderr: &str) -> blazar_transport::ExecOutput {
        blazar_transport::ExecOutput {
            code,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn unreachable_node_is_an_error_not_an_empty_result() {
        let e = check(
            &out(255, "ssh: connect to host gpu9: No route to host"),
            "/x",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("机器连不上"), "错误信息要指向机器: {e}");
        assert!(e.contains("No route to host"), "要保留原始原因: {e}");
    }

    #[test]
    fn missing_directory_says_so() {
        let e = check(&out(3, ""), "/home/me/proj").unwrap_err().to_string();
        assert!(e.contains("目录不存在"), "{e}");
        assert!(e.contains("/home/me/proj"), "要说清是哪个目录: {e}");
    }

    #[test]
    fn success_passes_through() {
        assert!(check(&out(0, ""), "/x").is_ok());
    }

    #[test]
    fn file_body_keeps_newlines_and_pipes() {
        let raw = "SIZE|42\nBODY|\nfn main() {\n    let a = b | c;\n}\n";
        let f = parse_file("src/main.rs", raw);
        assert_eq!(f.size, 42);
        assert!(!f.binary && !f.too_large);
        assert!(f.content.contains("let a = b | c;"));
        assert!(f.content.starts_with("fn main()"));
    }

    #[test]
    fn binary_and_oversize_return_no_content() {
        let b = parse_file("a.bin", "SIZE|1024\nBINARY|1\n");
        assert!(b.binary && b.content.is_empty() && b.size == 1024);

        let l = parse_file("big.log", "SIZE|30000000\nTOOLARGE|1\n");
        assert!(l.too_large && l.content.is_empty());
        assert_eq!(l.size, 30_000_000, "要把实际大小告诉用户，而不是只说太大");
    }

    #[test]
    fn empty_file_reads_as_empty_not_error() {
        let f = parse_file("empty.txt", "SIZE|0\nBODY|\n");
        assert_eq!(f.size, 0);
        assert!(f.content.is_empty());
        assert!(!f.binary && !f.too_large);
    }

    #[test]
    fn home_expands_but_paths_stay_quoted() {
        assert_eq!(cd_target(""), "~");
        assert_eq!(cd_target("~"), "~");
        assert_eq!(cd_target("~/my proj"), "~/'my proj'");

        assert_eq!(cd_target("/home/a b"), "'/home/a b'");

        let hostile = cd_target("/tmp; rm -rf /");
        assert_eq!(hostile, "'/tmp; rm -rf /'");

        assert_eq!(cd_target("/tmp/it's"), "'/tmp/it'\\''s'");
    }

    #[test]
    fn listing_puts_repos_first_and_finds_parent() {
        let raw = "PATH|/home/me\nPARENT|/home\n\
                   DIR|zebra|/home/me/zebra|0|3\n\
                   DIR|Alpha|/home/me/Alpha|0|0\n\
                   DIR|myrepo|/home/me/myrepo|1|12\n";
        let l = parse_listing(raw);
        assert_eq!(l.path, "/home/me");
        assert_eq!(l.parent.as_deref(), Some("/home"));
        assert!(!l.is_repo);

        assert_eq!(l.entries[0].name, "myrepo");
        assert!(l.entries[0].is_repo);
        assert_eq!(l.entries[1].name, "Alpha");
        assert_eq!(l.entries[2].name, "zebra");
        assert_eq!(l.entries[0].children, 12);
    }

    #[test]
    fn root_has_no_parent() {
        let l = parse_listing("PATH|/\nDIR|tmp|/tmp|0|5\n");
        assert!(l.parent.is_none(), "根目录不应有上一级");
    }

    #[test]
    fn self_repo_is_detected() {
        let l = parse_listing("PATH|/home/me/proj\nPARENT|/home/me\nSELFREPO|1\n");
        assert!(
            l.is_repo,
            "当前目录是仓库时要标出来，否则用户不知道能不能直接选"
        );
    }

    #[test]
    fn path_escape_is_rejected() {
        let v = vfs();
        assert!(v.safe_rel("../../etc/passwd").is_err());
        assert!(v.safe_rel("/etc/passwd").is_err());
        assert!(v.safe_rel("").is_err());
        assert!(v.safe_rel("src/main.rs").is_ok());
    }

    #[test]
    fn nested_parent_dir_still_rejected() {
        let v = vfs();
        assert!(v.safe_rel("src/../../../etc/passwd").is_err());
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("blazar-vfs-test-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    #[tokio::test]
    async fn write_round_trips_and_detects_conflicts() {
        let dir = scratch("write-roundtrip");
        let vfs = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            dir.display().to_string(),
        );

        let body = "第一行 'quoted' $(not run)\nline2\n";
        let w = vfs.write("docs/说明.md", body, None).await.unwrap();
        assert!(w.saved && w.size == body.len() as u64);
        let r = vfs.read("docs/说明.md").await.unwrap();
        assert_eq!(r.content, body);
        assert_eq!(r.mtime, w.mtime);

        let stale = vfs
            .write("docs/说明.md", "mine", Some(w.mtime.saturating_sub(100)))
            .await
            .unwrap();
        assert!(!stale.saved);
        assert_eq!(vfs.read("docs/说明.md").await.unwrap().content, body);

        assert!(
            vfs.write("docs/说明.md", "mine", Some(w.mtime))
                .await
                .unwrap()
                .saved
        );
        assert_eq!(vfs.read("docs/说明.md").await.unwrap().content, "mine");

        assert!(vfs.write("../escape.txt", "x", None).await.is_err());
        assert!(vfs.write("/etc/hosts", "x", None).await.is_err());
        assert!(vfs.write(".git/config", "x", None).await.is_err());
        assert!(
            vfs.write("a/.git/hooks/pre-commit", "x", None)
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_keeps_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("write-mode");
        let f = dir.join("run.sh");
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vfs = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            dir.display().to_string(),
        );
        vfs.write("run.sh", "#!/bin/sh\necho hi\n", None)
            .await
            .unwrap();
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    fn sh(dir: &std::path::Path, cmd: &str) {
        let st = std::process::Command::new("bash")
            .arg("-lc")
            .arg(cmd)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success(), "命令失败: {cmd}");
    }

    #[test]
    fn git_octal_escapes_round_trip() {
        assert_eq!(
            unquote_git_path(r#""\346\226\207\346\241\243/\350\257\264\346\230\216.md""#),
            "文档/说明.md"
        );
        assert_eq!(unquote_git_path("plain.txt"), "plain.txt");
        assert_eq!(unquote_git_path(r#""a b.txt""#), "a b.txt");
        assert_eq!(unquote_git_path(r#""say \"hi\".txt""#), r#"say "hi".txt"#);
        assert_eq!(unquote_git_path(r#""back\\slash""#), r"back\slash");
        assert_eq!(unquote_git_path(r#""tab\there""#), "tab\there");

        assert_eq!(unquote_git_path(r#""\xyz""#), "xyz");
    }

    #[tokio::test]
    async fn chinese_paths_get_their_change_marker() {
        let d = scratch("zhpath");
        sh(
            &d,
            "git init -q . && git config user.email t@t && git config user.name t",
        );
        std::fs::create_dir_all(d.join("文档")).unwrap();
        std::fs::write(d.join("文档/说明.md"), "a\n").unwrap();
        std::fs::write(d.join("plain.txt"), "b\n").unwrap();
        sh(&d, "git add -A && git commit -qm init");
        std::fs::write(d.join("文档/说明.md"), "changed\n").unwrap();

        let v = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            d.to_str().unwrap(),
        );
        let tree = v.tree(None).await.unwrap();
        let find = |p: &str| tree.iter().find(|e| e.path == p);

        assert_eq!(
            find("文档/说明.md").and_then(|e| e.change),
            Some(ChangeKind::Modified),
            "中文文件要拿到自己的改动标记，树: {:?}",
            tree.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
        assert!(
            !tree.iter().any(|e| e.path.contains("\\3")),
            "不能出现八进制转义的幽灵条目: {:?}",
            tree.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn utf8_text_is_not_mistaken_for_binary() {
        let d = scratch("utf8");
        let mut text = String::new();
        for i in 0..600 {
            text.push_str(&format!("中文测试内容需要足够长以便被截断 {i}\n"));
        }
        std::fs::write(d.join("zh.md"), &text).unwrap();
        std::fs::write(d.join("bin.dat"), [0u8, 1, 2, 3, 0, 5]).unwrap();

        let v = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            d.to_str().unwrap(),
        );
        let f = v.read("zh.md").await.unwrap();
        assert!(!f.binary, "中文纯文本不能被判成二进制");
        assert!(f.content.contains("中文测试内容"), "内容要真的读回来");

        let b = v.read("bin.dat").await.unwrap();
        assert!(b.binary, "真的含 NUL 的文件仍要判成二进制");
    }

    #[tokio::test]
    async fn diff_works_before_the_first_commit() {
        let d = scratch("nocommit");
        sh(
            &d,
            "git init -q . && git config user.email t@t && git config user.name t",
        );
        std::fs::write(d.join("a.txt"), "hello\n").unwrap();

        let v = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            d.to_str().unwrap(),
        );
        let out = v
            .diff("HEAD")
            .await
            .expect("没有提交也要出 diff 而不是报错");
        assert!(out.contains("a.txt"), "新增文件要出现在 diff 里: {out}");
        assert!(out.contains("+hello"), "内容要出现在 diff 里: {out}");
    }

    #[tokio::test]
    async fn diff_on_a_plain_directory_says_so() {
        let d = scratch("nogit");
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let v = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            d.to_str().unwrap(),
        );
        let err = v.diff("HEAD").await.unwrap_err();
        assert!(
            matches!(err, VfsError::NotARepo(_)),
            "要能结构化地区分出「不是仓库」，调用方才好显示成空态而不是错误: {err}"
        );
    }

    #[tokio::test]
    async fn looking_at_the_diff_does_not_change_the_users_repo() {
        let d = scratch("index-untouched");
        sh(
            &d,
            "git init -q . && git config user.email t@t && git config user.name t \
                && echo a > a && git add -A && git commit -qm i",
        );
        std::fs::write(d.join("新文件.txt"), "new\n").unwrap();
        let status = |d: &std::path::Path| {
            String::from_utf8(
                std::process::Command::new("git")
                    .args(["status", "--porcelain"])
                    .current_dir(d)
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
        };
        let before = status(&d);

        let v = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            d.to_str().unwrap(),
        );
        let diff = v.diff("HEAD").await.unwrap();
        let _ = v.tree_with_stat(None).await.unwrap();

        assert!(
            diff.contains("新文件.txt"),
            "未跟踪的新文件仍要出现在 diff 里: {diff}"
        );
        assert_eq!(
            before,
            status(&d),
            "看 diff / 树不能改变用户仓库的 git status"
        );
    }

    #[tokio::test]
    async fn tree_reports_how_much_changed_in_the_same_round_trip() {
        let d = scratch("diffstat");
        sh(
            &d,
            "git init -q . && git config user.email t@t && git config user.name t \
                && printf 'a\nb\nc\n' > f.txt && git add -A && git commit -qm i",
        );
        std::fs::write(d.join("f.txt"), "a\nB\nc\nd\n").unwrap();
        std::fs::write(d.join("new.txt"), "x\ny\n").unwrap();

        let v = Vfs::new(
            Arc::new(blazar_transport::LocalTransport),
            d.to_str().unwrap(),
        );
        let (_, stat) = v.tree_with_stat(None).await.unwrap();
        let stat = stat.expect("git 仓库要有改动量");
        assert_eq!(stat.files, 2, "{stat:?}");
        assert_eq!(stat.added, 4, "f.txt +2、new.txt +2: {stat:?}");
        assert_eq!(stat.removed, 1, "{stat:?}");
    }

    #[test]
    fn stat_section_is_parsed_and_absent_outside_git() {
        assert_eq!(
            parse_stat("__TRACKED__\na\n__STATUS__\n__STAT__\n12 3 4\n"),
            Some(DiffStat {
                added: 12,
                removed: 3,
                files: 4
            })
        );
        assert_eq!(parse_stat("__PLAIN__\na\n"), None, "非 git 目录没有改动量");

        let t = parse_tree("__TRACKED__\na.rs\n__STATUS__\n__STAT__\n1 2 3\n");
        assert!(t.iter().all(|e| e.path != "1 2 3"), "{t:?}");
    }
}
