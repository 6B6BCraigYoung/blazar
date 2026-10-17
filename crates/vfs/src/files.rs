use super::*;

pub(crate) fn parse_file(path: &str, raw: &str) -> FileContent {
    let mut size = 0u64;
    let mut mtime = 0u64;
    let mut version = None;
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
            Some(("VERSION", v)) => version = Some(v.to_owned()),
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
        version,
    }
}

pub(crate) fn parse_dir_items(raw: &str) -> DirItems {
    let (list, ignored) = raw.split_once("__IGNORED__\n").unwrap_or((raw, ""));
    let ignored: std::collections::HashSet<&str> = ignored.lines().collect();
    let mut items: Vec<DirItem> = list
        .lines()
        .filter_map(|l| {
            let (kind, name) = l.split_once('\t')?;
            Some(DirItem {
                name: name.to_owned(),
                is_dir: kind == "D",
                ignored: ignored.contains(name),
            })
        })
        .collect();
    let truncated = items.len() > MAX_DIR_ITEMS;
    items.truncate(MAX_DIR_ITEMS);
    DirItems { items, truncated }
}

impl Vfs {
    pub async fn validate_path(&self, rel: &str) -> Result<()> {
        let rel = if rel.is_empty() || rel == "." {
            ".".to_owned()
        } else {
            self.safe_rel(rel)?
        };
        let script = format!("{}\nexit 0", guarded_path(&self.root, &rel));
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        check(&out, &rel)
    }

    pub async fn list_dir(&self, rel: &str) -> Result<DirItems> {
        let dir = if rel.is_empty() || rel == "." {
            ".".to_owned()
        } else {
            self.safe_rel(rel)?
        };
        let script = format!(
            r#"{guard}
cd -P -- "$F" 2>/dev/null || exit 3
names() {{ for f in * .[!.]* ..?*; do
  {{ [ -e "$f" ] || [ -L "$f" ]; }} || continue
  [ "$f" = .git ] && continue
  printf '%s\n' "$f"
done; }}
names | head -n {cap1} | while IFS= read -r f; do
  if [ ! -L "$f" ] && [ -d "$f" ]; then printf 'D\t%s\n' "$f"; else printf 'F\t%s\n' "$f"; fi
done
if git rev-parse --git-dir >/dev/null 2>&1; then
  echo "__IGNORED__"
  names | head -n {cap1} | git check-ignore --stdin 2>/dev/null
fi
exit 0"#,
            guard = guarded_path(&self.root, &dir),
            cap1 = MAX_DIR_ITEMS + 1,
        );
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        check(&out, &dir)?;
        Ok(parse_dir_items(&out.stdout))
    }

    pub async fn read(&self, rel: &str) -> Result<FileContent> {
        let rel = self.safe_rel(rel)?;
        let script = format!(
            r#"{guard}
{digest}
[ -f "$F" ] || exit 4
SZ=$(wc -c < "$F" | tr -d ' ')
echo "SIZE|$SZ"
echo "MTIME|$(stat -c %Y "$F" 2>/dev/null || stat -f %m "$F" 2>/dev/null)"
if [ "$SZ" -gt {max} ]; then
  echo "TOOLARGE|1"
else
  umask 077
  TMP=$(mktemp) || exit 6
  trap 'rm -f -- "$TMP"' EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
  head -c {cap} "$F" > "$TMP" || exit 6
  SZ=$(wc -c < "$TMP" | tr -d ' ')
  echo "SIZE|$SZ"
  HEAD_BYTES=$(head -c 8192 "$TMP" | wc -c | tr -d ' ')
  NUL_STRIPPED=$(head -c 8192 "$TMP" | LC_ALL=C tr -d '\000' | wc -c | tr -d ' ')
  if [ "$NUL_STRIPPED" -ne "$HEAD_BYTES" ]; then
    echo "BINARY|1"
  elif [ "$SZ" -gt {max} ]; then
    echo "TOOLARGE|1"
  else
    VERSION=$(digest < "$TMP") || exit 6
    echo "VERSION|$VERSION"
    echo "BODY|"
    cat -- "$TMP" || exit 6
  fi
fi"#,
            guard = guarded_path(&self.root, &rel),
            digest = CONTENT_DIGEST,
            max = MAX_READ_BYTES,
            cap = MAX_READ_BYTES + 1,
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
            r#"{guard}
[ -f "$F" ] || exit 4
[ "$(wc -c < "$F" | tr -d ' ')" -le {max} ] || exit 8
base64 < "$F" | tr -d '\n'"#,
            guard = guarded_path(&self.root, &rel),
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

    pub async fn archive_base64(&self, rel: &str, max: u64) -> Result<String> {
        let rel = if rel.is_empty() || rel == "." {
            ".".to_owned()
        } else {
            self.safe_rel(rel)?
        };
        let script = format!(
            r#"{guard}
[ -d "$F" ] || exit 4
cd -- "$F" || exit 3
T=$(mktemp) || exit 1
trap 'rm -f "$T"' EXIT
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files -z -co --exclude-standard -- . | tar --null -czf "$T" -T - 2>/dev/null
else
  tar -czf "$T" . 2>/dev/null
fi
[ -s "$T" ] || exit 1
[ "$(wc -c < "$T" | tr -d ' ')" -le {max} ] || exit 8
base64 < "$T" | tr -d '\n'"#,
            guard = guarded_path(&self.root, &rel),
        );
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        if out.code == 4 || out.code == 8 {
            return Err(blazar_transport::TransportError::Command {
                code: out.code,
                stderr: if out.code == 4 {
                    format!("{rel} 不是一个文件夹")
                } else {
                    format!("{rel} 打包后太大了")
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
        self.write_checked(rel, content, expect_mtime, None).await
    }

    pub async fn write_checked(
        &self,
        rel: &str,
        content: &str,
        expect_mtime: Option<u64>,
        expect_version: Option<&str>,
    ) -> Result<Written> {
        let rel = self.safe_rel(rel)?;

        if rel.split('/').any(|component| component == ".git") {
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
            r#"{guard}
{digest}
mt() {{ stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null; }}
E={expect}
EV={expect_version}
HAS_EV={has_version}
DIR=${{F%/*}}
umask 077
mkdir -p -- "$DIR" || exit 6
guard_path "$F"
KEY=$(printf '%s' "${{F##*/}}" | digest) || exit 6
LOCK="$DIR/.blazar-write-$KEY.lock"
if ! mkdir -- "$LOCK" 2>/dev/null; then
  printf '文件正在保存，或上次保存被中断。请稍后重试；确认没有保存进程后可移除锁目录：%s\n' "$LOCK" >&2
  exit 10
fi
TMP=
cleanup() {{
  [ -z "$TMP" ] || rm -f -- "$TMP"
  rm -f -- "$LOCK/owner"
  rmdir -- "$LOCK" 2>/dev/null
}}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
printf '%s\n' "$$" > "$LOCK/owner" || exit 6
current() {{
  MT=0
  SZ=0
  VERSION=
  MODE=
  if [ -e "$F" ]; then
    [ -f "$F" ] || exit 4
    MT=$(mt "$F") || exit 6
    SZ=$(wc -c < "$F" | tr -d ' ') || exit 6
    VERSION=$(digest < "$F") || exit 6
    MODE=$(stat -c %a "$F" 2>/dev/null || stat -f %Lp "$F" 2>/dev/null) || exit 6
  fi
}}
matches_expected() {{
  if [ "$HAS_EV" = 1 ]; then
    [ "$VERSION" = "$EV" ]
  elif [ -n "$E" ]; then
    [ -e "$F" ] && [ "$MT" = "$E" ]
  else
    return 0
  fi
}}
conflict() {{ printf 'CONFLICT|%s|%s|%s\n' "$MT" "$SZ" "$VERSION"; exit 0; }}
guard_path "$F"
current
matches_expected || conflict
[ ! -e "$F" ] || [ -w "$F" ] || exit 6
TMP=$(mktemp "$DIR/.blazar-write.XXXXXXXX") || exit 6
cat > "$TMP" || exit 6
NEW_VERSION=$(digest < "$TMP") || exit 6
guard_path "$F"
current
matches_expected || conflict
[ -z "$MODE" ] || chmod "$MODE" "$TMP" || exit 6
mv -f -- "$TMP" "$F" || exit 6
TMP=
printf 'SAVED|%s|%s|%s\n' "$(mt "$F")" "$(wc -c < "$F" | tr -d ' ')" "$NEW_VERSION""#,
            guard = guarded_path(&self.root, &rel),
            digest = CONTENT_DIGEST,
            expect = shell_quote(&expect_mtime.map(|m| m.to_string()).unwrap_or_default()),
            expect_version = shell_quote(expect_version.unwrap_or_default()),
            has_version = u8::from(expect_version.is_some()),
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
                stderr: format!("{rel} 不是可保存的普通文件"),
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
            version: p.next().filter(|v| !v.is_empty()).map(str::to_owned),
        })
    }

    pub async fn write_base64(&self, rel: &str, b64: &str) -> Result<()> {
        let rel = self.safe_rel(rel)?;
        let script = format!(
            r#"{guard}
DIR=${{F%/*}}
mkdir -p -- "$DIR" || exit 6
guard_path "$F"
[ ! -e "$F" ] || [ -f "$F" ] || exit 4
TMP=$(mktemp "$DIR/.blazar-copy.XXXXXXXX") || exit 6
trap 'rm -f -- "$TMP"' EXIT
base64 -d > "$TMP" || exit 6
mv -f -- "$TMP" "$F" || exit 6
trap - EXIT"#,
            guard = guarded_path(&self.root, &rel),
        );
        let out = self
            .transport
            .exec(
                ExecSpec::new("bash")
                    .arg("-lc")
                    .arg(script)
                    .stdin(b64.as_bytes().to_vec()),
            )
            .await?;
        if out.code == 4 {
            return Err(blazar_transport::TransportError::Command {
                code: 4,
                stderr: format!("{rel} 已经存在且不是普通文件"),
            }
            .into());
        }
        check(&out, &rel)?;
        Ok(())
    }

    pub async fn extract_base64(&self, rel: &str, b64: &str) -> Result<()> {
        let rel = if rel.is_empty() || rel == "." {
            ".".to_owned()
        } else {
            self.safe_rel(rel)?
        };
        let script = format!(
            r#"{guard}
mkdir -p -- "$F" || exit 6
guard_path "$F"
[ -d "$F" ] || exit 4
T=$(mktemp) || exit 6
trap 'rm -f -- "$T"' EXIT
base64 -d > "$T" || exit 6
tar -tzf "$T" | grep -Eq '(^|/)\.\.(/|$)|^/' && exit 9
tar -xzf "$T" -C "$F" --no-same-owner || exit 6"#,
            guard = guarded_path(&self.root, &rel),
        );
        let out = self
            .transport
            .exec(
                ExecSpec::new("bash")
                    .arg("-lc")
                    .arg(script)
                    .stdin(b64.as_bytes().to_vec()),
            )
            .await?;
        if out.code == 4 {
            return Err(blazar_transport::TransportError::Command {
                code: 4,
                stderr: format!("{rel} 不是文件夹"),
            }
            .into());
        }
        check(&out, &rel)
    }

    pub async fn mkdir(&self, parent: &str, name: &str) -> Result<String> {
        let name = folder_name(name)?;
        let script = format!(
            r#"set -e
cd {target} 2>/dev/null || exit {no_dir}
HERE=$(pwd -P)
[ -e "$HERE/"{name} ] && exit {exists}
mkdir -- "$HERE/"{name} || exit {failed}
echo "PATH|$HERE/"{name}"#,
            target = cd_target(parent),
            name = shell_quote(name),
            no_dir = EXIT_NO_DIR,
            exists = EXIT_EXISTS,
            failed = EXIT_MKDIR,
        );
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        match out.code {
            EXIT_EXISTS => {
                return Err(VfsError::Folder(format!(
                    "已经有同名的文件或文件夹：{name}"
                )));
            }
            EXIT_MKDIR => {
                return Err(VfsError::Folder(format!(
                    "创建不了文件夹：{}",
                    out.stderr.trim()
                )));
            }
            _ => check(&out, parent)?,
        }
        Ok(out
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("PATH|"))
            .unwrap_or_default()
            .to_owned())
    }
}
