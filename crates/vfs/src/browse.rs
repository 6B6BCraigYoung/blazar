use super::*;

const BROWSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub(crate) fn parse_listing(raw: &str) -> DirListing {
    let mut path = String::new();
    let mut parent = None;
    let mut is_repo = false;
    let mut home = None;
    let mut entries = Vec::new();

    for line in raw.lines() {
        let parts: Vec<&str> = line.trim_end().split('|').collect();
        match parts.as_slice() {
            ["PATH", p] => path = (*p).to_owned(),
            ["PARENT", p] => parent = Some((*p).to_owned()),
            ["HOME", p] if !p.is_empty() => home = Some((*p).to_owned()),
            ["SELFREPO", _] => is_repo = true,
            ["DIR", name, full, repo, cnt] => entries.push(DirEntry {
                name: (*name).to_owned(),
                path: (*full).to_owned(),
                is_repo: *repo == "1",
                children: cnt.parse().ok(),
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
        home,
    }
}

impl Vfs {
    pub async fn browse(&self, path: &str) -> Result<DirListing> {
        let target = cd_target(path);
        let script = format!(
            r#"set -e
cd {target} 2>/dev/null || cd ~ || exit 3
HERE=$(pwd -P)
PROTECTED=""
if [ "$(uname)" = Darwin ]; then
  HOME_REAL=$(cd ~ && pwd -P)
  for p in Desktop Documents Downloads Library Movies Music Pictures; do
    PROTECTED="$PROTECTED|$HOME_REAL/$p|"
  done
fi
echo "PATH|$HERE"
echo "HOME|$(cd ~ && pwd -P)"
if [ "$HERE" != "/" ]; then echo "PARENT|$(dirname "$HERE")"; fi
if [ -d "$HERE/.git" ] || [ -f "$HERE/.git" ]; then echo "SELFREPO|1"; fi
for d in */ ; do
  [ -d "$d" ] || continue
  n=${{d%/}}
  case "$n" in .*) continue;; esac
  case "$PROTECTED" in *"|$HERE/$n|"*) echo "DIR|$n|$HERE/$n|0|"; continue;; esac
  repo=0
  if [ -d "$HERE/$n/.git" ] || [ -f "$HERE/$n/.git" ]; then repo=1; fi
  cnt=$(find "$HERE/$n" -maxdepth 1 -mindepth 1 -type d ! -name '.*' 2>/dev/null | head -200 | wc -l | tr -d ' ')
  echo "DIR|$n|$HERE/$n|$repo|$cnt"
done"#,
            target = target,
        );

        let out = tokio::time::timeout(
            BROWSE_TIMEOUT,
            self.transport
                .exec(ExecSpec::new("bash").arg("-lc").arg(script)),
        )
        .await
        .map_err(|_| {
            VfsError::Folder(
                "读取目录超时：可能在等系统授权访问这个文件夹，请到这台机器上看看".into(),
            )
        })??;
        check(&out, &self.root)?;
        Ok(parse_listing(&out.stdout))
    }
}
