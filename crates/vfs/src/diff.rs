use super::*;

impl Vfs {
    pub async fn diff(&self, base: &str) -> Result<String> {
        self.diff_opts(base, DiffOpts::default()).await
    }

    pub async fn diff_opts(&self, base: &str, opts: DiffOpts) -> Result<String> {
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
git rev-parse --git-dir >/dev/null 2>&1 || exit 5
{temp_index}
B={base}
{merge_base}
git rev-parse --verify --quiet "$B^{{commit}}" >/dev/null 2>&1 \
  || B=$(git hash-object -t tree /dev/null)
git --no-pager -c core.quotepath=false diff {ws} "$B"{cap}"#,
            root = shell_quote(&self.root),
            base = shell_quote(base),
            temp_index = TEMP_INDEX,
            merge_base = if opts.merge_base {
                r#"MB=$(git merge-base "$B" HEAD 2>/dev/null) && B=$MB"#
            } else {
                ""
            },
            ws = if opts.ignore_ws { "-w" } else { "" },
            // head 提前收口时 git 会收到 SIGPIPE（141），那是截断不是出错。
            cap = match opts.max_bytes {
                Some(n) => format!(
                    " | head -c {n}\nS=${{PIPESTATUS[0]}}; [ \"$S\" = 141 ] && S=0; exit $S"
                ),
                None => String::new(),
            },
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
