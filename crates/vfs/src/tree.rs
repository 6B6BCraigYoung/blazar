use super::*;

const TREE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn change_from_porcelain(code: &str) -> ChangeKind {
    match code.trim() {
        "??" => ChangeKind::Untracked,
        c if c.contains('A') => ChangeKind::Added,
        c if c.contains('D') => ChangeKind::Deleted,
        _ => ChangeKind::Modified,
    }
}

pub(crate) fn parse_tree(raw: &str) -> Vec<TreeEntry> {
    use std::collections::{BTreeMap, BTreeSet};

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

                files.insert(unquote_git_path(path), Some(change_from_porcelain(code)));
            }
            _ => {}
        }
    }

    let mut dirs = BTreeSet::new();
    for path in files.keys() {
        let mut acc = String::new();
        let parts: Vec<_> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            dirs.insert(acc.clone());
        }
    }

    let mut out: Vec<TreeEntry> = dirs
        .into_iter()
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

pub(crate) fn unquote_git_path(s: &str) -> String {
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

impl Vfs {
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
  git -c core.quotepath=false status --porcelain -uall {base}
  echo "__STAT__"
  (
    {temp_index}
    B=HEAD; git rev-parse --verify --quiet HEAD >/dev/null || B=$(git hash-object -t tree /dev/null)
    git diff --numstat "$B" 2>/dev/null | awk '{{a+=$1; d+=$2; f++}} END {{print a+0, d+0, f+0}}'
  )
else
  echo "__PLAIN__"
  set --
  if [ "$(uname)" = Darwin ]; then
    H=$(cd ~ && pwd -P)
    R=$(pwd -P)
    P=""
    if [ "$R" = "$H" ]; then P=.
    elif [ "$R" = / ]; then P=".$H"
    else case "$H" in "$R"/*) P=".${{H#"$R"}}";; esac
    fi
    if [ -n "$P" ]; then
      for d in Desktop Documents Downloads Library Movies Music Pictures; do
        set -- "$@" -path "$P/$d" -prune -o
      done
    fi
  fi
  find . "$@" -type f -not -path '*/.git/*' -not -path '*/node_modules/*' \
       -not -path '*/target/*' -not -path '*/.venv/*' -print 2>/dev/null \
    | sed 's|^\./||' | head -{cap}
fi"#,
            root = shell_quote(&self.root),
            base = base.map(shell_quote).unwrap_or_default(),
            cap = MAX_TREE_ENTRIES,
            temp_index = TEMP_INDEX,
        );

        let out = tokio::time::timeout(
            TREE_TIMEOUT,
            self.transport
                .exec(ExecSpec::new("bash").arg("-lc").arg(script)),
        )
        .await
        .map_err(|_| {
            VfsError::Folder(
                "读取文件列表超时：目录可能太大，或在等系统授权访问其中的文件夹".into(),
            )
        })??;
        check(&out, &self.root)?;
        Ok((parse_tree(&out.stdout), parse_stat(&out.stdout)))
    }
}
