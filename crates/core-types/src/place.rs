#[must_use]
pub fn trim(path: &str) -> &str {
    let t = path.trim_end_matches('/');
    if t.is_empty() { "/" } else { t }
}

const CATCH_ALL: [&str; 9] = [
    "workspace",
    "workspaces",
    "code",
    "src",
    "projects",
    "repos",
    "dev",
    "work",
    "tmp",
];

#[must_use]
pub fn broad(path: &str) -> bool {
    let parts: Vec<&str> = trim(path).split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] | [_] => true,
        [first, _] if matches!(*first, "home" | "Users") => true,
        [.., last] => CATCH_ALL.contains(&last.to_ascii_lowercase().as_str()),
    }
}

#[must_use]
pub fn inside(path: &str, root: &str) -> bool {
    let (path, root) = (trim(path), trim(root));
    path.len() > root.len()
        && path.starts_with(root)
        && (root == "/" || path.as_bytes().get(root.len()) == Some(&b'/'))
}

#[must_use]
pub fn container<'a, I>(spots: I, node: &str, path: &str) -> Option<&'a str>
where
    I: IntoIterator<Item = (&'a str, &'a str, &'a str)>,
{
    let mut best: Option<(&'a str, usize)> = None;
    for (id, at, root) in spots {
        if at != node || broad(root) || !inside(path, root) {
            continue;
        }
        let depth = trim(root).len();
        if best.is_none_or(|(_, d)| depth > d) {
            best = Some((id, depth));
        }
    }
    best.map(|(id, _)| id)
}

#[must_use]
pub fn folder(path: &str) -> &str {
    trim(path).rsplit('/').next().unwrap_or_default()
}

#[cfg(test)]
#[path = "place_tests.rs"]
mod tests;
