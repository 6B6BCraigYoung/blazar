use super::*;

const REMOTE_SEARCH_TIMEOUT_SECS: u64 = 10;

pub(crate) fn parse_search(raw: &str) -> Vec<SearchHit> {
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

impl Vfs {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let script = format!(
            r#"cd {root} 2>/dev/null || exit 3
T="timeout -k 2 {secs}"; command -v timeout >/dev/null 2>&1 || T=""
if git rev-parse --git-dir >/dev/null 2>&1; then
  $T git --no-pager grep -n -I --no-color -e {q} -- . 2>/dev/null | head -n {limit}
elif command -v rg >/dev/null 2>&1; then
  $T rg --line-number --no-heading --color never --max-count 50 --max-filesize 2M \
     --glob '!.git' --glob '!node_modules' --glob '!target' --glob '!.venv' \
     -e {q} . 2>/dev/null | head -n {limit}
else
  $T find . -type d \
       \( -name .git -o -name node_modules -o -name target -o -name .venv -o -name .cache \) -prune -o \
       -type f -exec grep -nH --binary-files=without-match -e {q} -- {{}} + 2>/dev/null | head -n {limit}
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
}
