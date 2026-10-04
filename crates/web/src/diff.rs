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

#[cfg(test)]
mod tests {
    use super::*;

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
