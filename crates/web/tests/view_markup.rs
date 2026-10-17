use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn literal_end(src: &[u8], i: usize) -> Option<usize> {
    let ident_before = i > 0 && (src[i - 1].is_ascii_alphanumeric() || src[i - 1] == b'_');
    match src[i] {
        b'r' if !ident_before && matches!(src.get(i + 1), Some(b'#' | b'"')) => {
            let hashes = src[i + 1..].iter().take_while(|&&c| c == b'#').count();
            let body = i + 1 + hashes;
            if src.get(body) != Some(&b'"') {
                return None;
            }
            let close = [&b"\""[..], &vec![b'#'; hashes]].concat();
            let at = src[body + 1..]
                .windows(close.len())
                .position(|w| w == close)?;
            Some(body + 1 + at + close.len())
        }
        b'"' => {
            let mut j = i + 1;
            while j < src.len() && src[j] != b'"' {
                j += if src[j] == b'\\' { 2 } else { 1 };
            }
            Some(j + 1)
        }
        b'\'' if src.get(i + 2) == Some(&b'\'') => Some(i + 3),
        b'\'' if src.get(i + 1) == Some(&b'\\') => {
            let at = src[i + 2..].iter().position(|&c| c == b'\'')?;
            Some(i + 2 + at + 1)
        }
        _ => None,
    }
}

fn group_end(src: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < src.len() {
        if let Some(next) = literal_end(src, i) {
            i = next;
            continue;
        }
        match src[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    src.len()
}

fn tag_end(src: &[u8], mut i: usize, stop: usize) -> usize {
    while i < stop {
        if let Some(next) = literal_end(src, i) {
            i = next;
            continue;
        }
        match src[i] {
            b'(' | b'[' | b'{' => i = group_end(src, i),
            b'>' => return i,
            _ => {}
        }
        i += 1;
    }
    stop
}

fn cut_tags(src: &[u8], start: usize, stop: usize) -> Vec<usize> {
    let mut found = Vec::new();
    let mut i = start;
    while i < stop {
        if let Some(next) = literal_end(src, i) {
            i = next;
            continue;
        }
        match src[i] {
            b'(' | b'[' | b'{' => i = group_end(src, i),
            b'<' if src.get(i + 1).is_some_and(u8::is_ascii_alphabetic) => {
                let end = tag_end(src, i + 1, stop);
                let next = src[end + 1..stop].iter().find(|c| !c.is_ascii_whitespace());
                if src[end - 1] != b'/' && !matches!(next, None | Some(b'"' | b'{' | b'<')) {
                    found.push(end);
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    found
}

#[test]
fn attribute_values_never_end_their_tag_early() {
    let mut files = Vec::new();
    sources(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut files,
    );
    let mut broken = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let src = text.as_bytes();
        for (at, _) in text.match_indices("view!") {
            let Some(open) = src[at..].iter().position(|c| matches!(c, b'{' | b'(')) else {
                continue;
            };
            let open = at + open;
            for end in cut_tags(src, open + 1, group_end(src, open)) {
                let line = text[..end].lines().count();
                broken.push(format!("{}:{line}", file.display()));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "a `>` inside an unbraced attribute value closes the tag; wrap the value in braces: {broken:#?}"
    );
}

#[test]
fn the_scanner_reads_markup_the_way_the_view_macro_does() {
    let tags = |s: &str| cut_tags(s.as_bytes(), 0, s.len()).len();
    assert_eq!(
        tags(r#"<b disabled=move||n.len()>=3 on:click=f>"x"</b>"#),
        1
    );
    assert_eq!(
        tags(r#"<b disabled=move||{n.len()>=3} on:click=f>"x"</b>"#),
        0
    );
    assert_eq!(tags(r##"<b title="a > b" class=r#"c>d"#>{x}</b>"##), 0);
    assert_eq!(tags(r#"<i class=('>', true)/><b>"ok"</b>"#), 0);
}
