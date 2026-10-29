#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Added,
    Deleted,
    Renamed,
    Modified,
}

impl Status {
    pub fn mark(self) -> (&'static str, &'static str) {
        match self {
            Self::Added => ("A", "added"),
            Self::Deleted => ("D", "deleted"),
            Self::Renamed => ("R", "renamed"),
            Self::Modified => ("M", "modified"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Add,
    Del,
    Ctx,

    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old: u32,
    pub new: u32,
    pub ctx: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub old_path: String,
    pub path: String,
    pub status: Status,
    pub hunks: Vec<Hunk>,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,

    pub lines: u32,
}

fn unquote(s: &str) -> &str {
    s.trim_matches('"')
}

fn header_paths(line: &str) -> (String, String) {
    let rest = line.trim_start_matches("diff --git ");
    if let Some(i) = rest.find(" b/").or_else(|| rest.find(" \"b/")) {
        let a = unquote(&rest[..i]);
        let b = unquote(&rest[i + 1..]);
        (
            a.strip_prefix("a/").unwrap_or(a).to_owned(),
            b.strip_prefix("b/").unwrap_or(b).to_owned(),
        )
    } else {
        (String::new(), String::new())
    }
}

fn hunk_header(line: &str) -> Option<(u32, u32, String)> {
    let body = line.strip_prefix("@@ -")?;
    let (ranges, ctx) = body.split_once(" @@")?;
    let (o, n) = ranges.split_once(" +")?;
    let start = |r: &str| r.split(',').next()?.parse::<u32>().ok();
    Some((start(o)?, start(n)?, ctx.trim().to_owned()))
}

pub fn parse(raw: &str) -> Vec<File> {
    let mut files: Vec<File> = Vec::new();
    let (mut o, mut n) = (0u32, 0u32);
    for line in raw.lines() {
        if line.starts_with("diff --git ") {
            let (old_path, path) = header_paths(line);
            files.push(File {
                old_path,
                path,
                status: Status::Modified,
                hunks: Vec::new(),
                added: 0,
                removed: 0,
                binary: false,
                lines: 0,
            });
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if let Some((ho, hn, ctx)) = hunk_header(line) {
            f.hunks.push(Hunk {
                old: ho,
                new: hn,
                ctx,
                lines: Vec::new(),
            });
            o = ho;
            n = hn;
            continue;
        }
        let Some(h) = f.hunks.last_mut() else {
            if line.starts_with("new file mode") {
                f.status = Status::Added;
            } else if line.starts_with("deleted file mode") {
                f.status = Status::Deleted;
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.status = Status::Renamed;
                p.clone_into(&mut f.old_path);
            } else if let Some(p) = line.strip_prefix("rename to ") {
                p.clone_into(&mut f.path);
            } else if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                f.binary = true;
            } else if let Some(p) = line.strip_prefix("+++ ")
                && p != "/dev/null"
            {
                let p = unquote(p);
                p.strip_prefix("b/").unwrap_or(p).clone_into(&mut f.path);
            }
            continue;
        };
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (Kind::Add, &line[1..]),
            Some(b'-') => (Kind::Del, &line[1..]),
            Some(b' ') => (Kind::Ctx, &line[1..]),
            Some(b'\\') => (Kind::Note, line.get(2..).unwrap_or("")),

            None => (Kind::Ctx, ""),
            _ => continue,
        };
        let (lo, ln) = match kind {
            Kind::Add => {
                n += 1;
                f.added += 1;
                (None, Some(n - 1))
            }
            Kind::Del => {
                o += 1;
                f.removed += 1;
                (Some(o - 1), None)
            }
            Kind::Ctx => {
                o += 1;
                n += 1;
                (Some(o - 1), Some(n - 1))
            }
            Kind::Note => (None, None),
        };
        if kind != Kind::Note {
            f.lines += 1;
        }
        h.lines.push(Line {
            kind,
            old: lo,
            new: ln,
            text: text.to_owned(),
        });
    }
    files
}

/// 一行里改动过的字节区间 `[start, end)`，按顺序、不重叠。
pub type Spans = Vec<(usize, usize)>;

/// 超过这么长的行不做词级对比：太长的行逐词标出来也看不清，还费时间。
pub const WORD_MAX_LINE: usize = 400;
/// 超过这么多行的一处改动整块跳过词级对比。
pub const WORD_MAX_HUNK: usize = 200;

/// 0 = 单词（ASCII 字母数字、下划线、非 CJK 的字母），1 = 空白，2 = 单个符号或 CJK 字。
fn token_class(c: char) -> u8 {
    if c.is_whitespace() {
        1
    } else if c == '_' || (c.is_alphanumeric() && (c.is_ascii() || (c as u32) < 0x2E80)) {
        0
    } else {
        2
    }
}

/// 把一行切成词：连续的单词字符、连续的空白各算一个，其余每个字符单独一个。
fn tokens(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let class = token_class(c);
        let mut end = i + c.len_utf8();
        if class != 2 {
            while let Some(&(j, d)) = it.peek() {
                if token_class(d) != class {
                    break;
                }
                end = j + d.len_utf8();
                it.next();
            }
        }
        out.push((i, end));
    }
    out
}

/// 一对「删掉的行、加上的行」里具体改了哪些词。返回 `None` 表示不值得逐词标：
/// 行太长、两行一样、或者两行几乎完全不同（那样整行的底色已经说明了一切）。
pub fn word_diff(old: &str, new: &str) -> Option<(Spans, Spans)> {
    if old.len() > WORD_MAX_LINE || new.len() > WORD_MAX_LINE || old == new {
        return None;
    }
    let a = tokens(old);
    let b = tokens(new);
    let at = |t: &(usize, usize)| &old[t.0..t.1];
    let bt = |t: &(usize, usize)| &new[t.0..t.1];
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && at(&a[pre]) == bt(&b[pre]) {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre
        && suf < b.len() - pre
        && at(&a[a.len() - 1 - suf]) == bt(&b[b.len() - 1 - suf])
    {
        suf += 1;
    }
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    // 中间那段做最长公共子序列；行长有上限，表最多 400×400。
    let (n, m) = (am.len(), bm.len());
    let mut dp = vec![0u16; (n + 1) * (m + 1)];
    let w = m + 1;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * w + j] = if at(&am[i]) == bt(&bm[j]) {
                dp[(i + 1) * w + j + 1] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + j + 1])
            };
        }
    }
    let mut keep_a = vec![false; n];
    let mut keep_b = vec![false; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if at(&am[i]) == bt(&bm[j]) {
            keep_a[i] = true;
            keep_b[j] = true;
            i += 1;
            j += 1;
        } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let solid = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
    let same: usize = a[..pre]
        .iter()
        .chain(&a[a.len() - suf..])
        .chain(am.iter().zip(&keep_a).filter(|x| *x.1).map(|x| x.0))
        .map(|t| solid(at(t)))
        .sum();
    let longest = solid(old).max(solid(new));
    // 不变的部分不到四成，就当整行重写，不逐词标。
    if longest == 0 || same * 10 < longest * 4 {
        return None;
    }
    let spans = |toks: &[(usize, usize)], keep: &[bool]| {
        let mut out: Spans = Vec::new();
        for (t, k) in toks.iter().zip(keep) {
            if *k {
                continue;
            }
            match out.last_mut() {
                Some(last) if last.1 == t.0 => last.1 = t.1,
                _ => out.push(*t),
            }
        }
        out
    };
    let sa = spans(am, &keep_a);
    let sb = spans(bm, &keep_b);
    if sa.is_empty() && sb.is_empty() {
        return None;
    }
    Some((sa, sb))
}

/// 一处改动里每一行要加重标出的词，下标和 `h.lines` 对齐；没有的就是空。
/// 连着的一串删除行和紧跟的一串新增行按顺序一一配对（和并排视图的配法一样）。
pub fn hunk_words(h: &Hunk) -> Vec<Spans> {
    let ls = &h.lines;
    let mut out = vec![Vec::new(); ls.len()];
    if ls.len() > WORD_MAX_HUNK {
        return out;
    }
    let mut i = 0;
    while i < ls.len() {
        if ls[i].kind != Kind::Del {
            i += 1;
            continue;
        }
        let mut dels = Vec::new();
        let mut adds = Vec::new();
        while i < ls.len() && matches!(ls[i].kind, Kind::Del | Kind::Note) {
            if ls[i].kind == Kind::Del {
                dels.push(i);
            }
            i += 1;
        }
        while i < ls.len() && matches!(ls[i].kind, Kind::Add | Kind::Note) {
            if ls[i].kind == Kind::Add {
                adds.push(i);
            }
            i += 1;
        }
        for (&d, &a) in dels.iter().zip(&adds) {
            if let Some((sd, sa)) = word_diff(&ls[d].text, &ls[a].text) {
                out[d] = sd;
                out[a] = sa;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(s: &str, spans: &[(usize, usize)]) -> String {
        let mut out = String::new();
        let mut at = 0;
        for &(a, b) in spans {
            out.push_str(&s[at..a]);
            out.push('[');
            out.push_str(&s[a..b]);
            out.push(']');
            at = b;
        }
        out.push_str(&s[at..]);
        out
    }

    #[test]
    fn word_diff_marks_only_changed_words() {
        let (o, n) = ("let x = foo(a, b);", "let x = bar(a, b, c);");
        let (so, sn) = word_diff(o, n).unwrap();
        assert_eq!(marked(o, &so), "let x = [foo](a, b);");
        assert_eq!(marked(n, &sn), "let x = [bar](a, b[, c]);");
    }

    #[test]
    fn word_diff_handles_pure_insertions_and_cjk() {
        let (o, n) = ("合到 main", "合到 main 分支");
        let (so, sn) = word_diff(o, n).unwrap();
        assert!(so.is_empty());
        assert_eq!(marked(n, &sn), "合到 main[ 分支]");
        let (so, sn) = word_diff("这次的改动", "这条分支的改动").unwrap();
        assert_eq!(marked("这次的改动", &so), "这[次]的改动");
        assert_eq!(marked("这条分支的改动", &sn), "这[条分支]的改动");
    }

    #[test]
    fn word_diff_skips_rewrites_equal_and_long_lines() {
        assert_eq!(word_diff("two", "TWO"), None);
        assert_eq!(word_diff("same", "same"), None);
        assert_eq!(
            word_diff("completely different", "nothing alike here"),
            None
        );
        let long = "a ".repeat(WORD_MAX_LINE);
        assert_eq!(word_diff(&long, &format!("{long}b")), None);
    }

    #[test]
    fn hunk_words_pairs_runs_in_order() {
        let f = parse(
            "diff --git a/x b/x
--- a/x
+++ b/x
@@ -1,4 +1,4 @@
 keep
-let a = 1;
-let b = 2;
+let a = 10;
+let b = 2; // two
 tail
",
        );
        let h = &f[0].hunks[0];
        let w = hunk_words(h);
        assert_eq!(w.len(), h.lines.len());
        assert!(w[0].is_empty() && w[5].is_empty());
        assert_eq!(marked(&h.lines[1].text, &w[1]), "let a = [1];");
        assert_eq!(marked(&h.lines[3].text, &w[3]), "let a = [10];");
        assert!(w[2].is_empty());
        assert_eq!(marked(&h.lines[4].text, &w[4]), "let b = 2;[ // two]");
    }

    #[test]
    fn hunk_words_gives_up_on_huge_hunks() {
        let mut lines = Vec::new();
        for k in 0..=WORD_MAX_HUNK {
            lines.push(Line {
                kind: if k % 2 == 0 { Kind::Del } else { Kind::Add },
                old: None,
                new: None,
                text: format!("value {k}"),
            });
        }
        let h = Hunk {
            old: 1,
            new: 1,
            ctx: String::new(),
            lines,
        };
        assert!(hunk_words(&h).iter().all(Vec::is_empty));
    }

    const SAMPLE: &str = "diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@ fn main
 one
-two
+TWO
 three
\\ No newline at end of file
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hi
diff --git a/old.rs b/moved.rs
similarity index 100%
rename from old.rs
rename to moved.rs
diff --git a/img.png b/img.png
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_files_hunks_and_line_numbers() {
        let f = parse(SAMPLE);
        assert_eq!(f.len(), 4);
        assert_eq!(
            (f[0].path.as_str(), f[0].added, f[0].removed),
            ("src/a.rs", 1, 1)
        );
        let h = &f[0].hunks[0];
        assert_eq!(h.ctx, "fn main");
        assert_eq!(
            (h.lines[1].kind, h.lines[1].old, h.lines[1].new),
            (Kind::Del, Some(2), None)
        );
        assert_eq!(
            (h.lines[2].kind, h.lines[2].old, h.lines[2].new),
            (Kind::Add, None, Some(2))
        );
        assert_eq!((h.lines[3].old, h.lines[3].new), (Some(3), Some(3)));
        assert_eq!(h.lines[4].kind, Kind::Note);
        assert_eq!(f[0].lines, 4);
        assert_eq!(f[1].status, Status::Added);
        assert_eq!(f[1].hunks[0].lines[0].new, Some(1));
        assert_eq!(
            (f[2].status, f[2].old_path.as_str(), f[2].path.as_str()),
            (Status::Renamed, "old.rs", "moved.rs")
        );
        assert!(f[3].binary);
    }
}
