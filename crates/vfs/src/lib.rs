use std::path::Path;
use std::sync::Arc;

use blazar_transport::{ExecSpec, NodeTransport};
use serde::{Deserialize, Serialize};

mod browse;
mod diff;
mod download;
mod files;
mod search;
#[cfg(test)]
mod tests;
mod tree;

pub use blazar_core_types::api::DiffStat;
pub use download::Download;
pub use tree::parse_stat;

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("传输失败: {0}")]
    Transport(#[from] blazar_transport::TransportError),

    #[error("路径逃逸出工作区: {0}")]
    PathEscape(String),

    #[error("不是 git 仓库: {0}")]
    NotARepo(String),

    #[error("{0}")]
    Folder(String),
}

pub type Result<T> = std::result::Result<T, VfsError>;

pub const MAX_TREE_ENTRIES: usize = 20_000;

pub const MAX_DIR_ITEMS: usize = 5_000;

const EXIT_NO_DIR: i32 = 3;

const EXIT_NO_GIT: i32 = 5;

const EXIT_SSH: i32 = 255;
const EXIT_PATH_ESCAPE: i32 = 9;
const EXIT_EXISTS: i32 = 6;
const EXIT_MKDIR: i32 = 7;
const EXIT_MISSING: i32 = 4;
const MAX_NAME_BYTES: usize = 255;

const CONTENT_DIGEST: &str = r#"digest() {
  local value
  if command -v sha256sum >/dev/null 2>&1; then
    value=$(sha256sum) || return 1
    value=${value%% *}
  elif command -v shasum >/dev/null 2>&1; then
    value=$(shasum -a 256) || return 1
    value=${value%% *}
  elif command -v openssl >/dev/null 2>&1; then
    value=$(openssl dgst -sha256) || return 1
    value=${value##* }
  else
    printf '%s\n' '无法校验文件版本：需要 sha256sum、shasum 或 openssl' >&2
    return 1
  fi
  [ "${#value}" -eq 64 ] || return 1
  case "$value" in *[!0-9a-f]*) return 1;; esac
  printf '%s' "$value"
}"#;

pub fn folder_name(name: &str) -> Result<&str> {
    let name = name.trim();
    let bad = if name.is_empty() {
        "文件夹名不能为空"
    } else if name == "." || name == ".." {
        "文件夹名不能是 . 或 .."
    } else if name.starts_with('.') {
        "文件夹名不能以 . 开头，列表里看不到"
    } else if name.contains(['/', '\\', '\0']) {
        "文件夹名不能包含 / 或 \\"
    } else if name.chars().any(char::is_control) {
        "文件夹名不能包含控制字符"
    } else if name.len() > MAX_NAME_BYTES {
        "文件夹名太长"
    } else {
        return Ok(name);
    };
    Err(VfsError::Folder(bad.to_owned()))
}

pub fn entry_name(name: &str) -> Result<&str> {
    let bad = if name.trim().is_empty() {
        "名称不能为空"
    } else if name == "." || name == ".." {
        "名称不能是 . 或 .."
    } else if name.contains(['/', '\\', '\0']) {
        "名称不能包含 / 或 \\"
    } else if name.chars().any(char::is_control) {
        "名称不能包含控制字符"
    } else if name.len() > MAX_NAME_BYTES {
        "名称太长"
    } else {
        return Ok(name);
    };
    Err(VfsError::Folder(bad.to_owned()))
}

pub fn safe_relative_path(rel: &str) -> Result<String> {
    let normalized = rel.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.contains('\0')
        || (normalized.as_bytes().get(1) == Some(&b':')
            && normalized.as_bytes()[0].is_ascii_alphabetic())
        || normalized.split('/').any(|part| part == "..")
    {
        return Err(VfsError::PathEscape(rel.to_owned()));
    }
    let parts: Vec<_> = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return Err(VfsError::PathEscape(rel.to_owned()));
    }
    Ok(parts.join("/"))
}

pub fn confined_local_path(root: &Path, rel: &str) -> Result<std::path::PathBuf> {
    let rel = safe_relative_path(rel)?;
    let mut path = root
        .canonicalize()
        .map_err(blazar_transport::TransportError::from)?;
    for component in rel.split('/') {
        path.push(component);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(VfsError::PathEscape(rel));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(blazar_transport::TransportError::from(error).into()),
        }
    }
    Ok(path)
}

fn guarded_path(root: &str, rel: &str) -> String {
    format!(
        r#"cd -P -- {root} 2>/dev/null || exit 3
F={rel}
guard_path() {{
  local rest="$1" current=. component
  while [ -n "$rest" ]; do
    component=${{rest%%/*}}
    current="$current/$component"
    [ ! -L "$current" ] || exit 9
    case "$rest" in */*) rest=${{rest#*/}};; *) break;; esac
  done
}}
guard_path "$F"
"#,
        root = shell_quote(root),
        rel = shell_quote(&format!("./{rel}"))
    )
}

fn check(out: &blazar_transport::ExecOutput, what: &str) -> Result<()> {
    if out.code == 0 {
        return Ok(());
    }
    if out.code == EXIT_PATH_ESCAPE {
        return Err(VfsError::PathEscape(what.to_owned()));
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
cp "$(git rev-parse --git-path index)" "$__IDX" 2>/dev/null || rm -f "$__IDX"
export GIT_INDEX_FILE="$__IDX"
git add -N . >/dev/null 2>&1"#;

pub use blazar_core_types::api::{ChangeKind, DirItem, DirItems, FileContent, TreeEntry, Written};

pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,

    pub path: String,

    pub is_repo: bool,

    pub children: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String,

    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,

    pub is_repo: bool,

    #[serde(default)]
    pub home: Option<String>,
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

    /// 远端只回传前这么多字节，超大 diff 不再整份经 ssh 拉回来。
    pub max_bytes: Option<usize>,
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
        safe_relative_path(rel)
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
