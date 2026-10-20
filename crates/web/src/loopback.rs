fn bare(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

pub fn away_from_page(url: &str, page_host: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return url.to_owned();
    }
    let (host, port) = match authority.rfind(':') {
        Some(at) if !authority[at..].contains(']') => authority.split_at(at),
        _ => (authority, ""),
    };
    let host = bare(host);
    if host != bare(page_host) || !matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return url.to_owned();
    }
    let other = if host == "localhost" {
        "127.0.0.1"
    } else {
        "localhost"
    };
    format!("{scheme}://{other}{port}{tail}")
}

#[cfg(test)]
mod tests {
    use super::away_from_page;

    #[test]
    fn preview_never_shares_the_page_host() {
        for (url, page, want) in [
            (
                "http://127.0.0.1:5173/",
                "127.0.0.1",
                "http://localhost:5173/",
            ),
            (
                "http://127.0.0.1:5173/a?b#c",
                "127.0.0.1",
                "http://localhost:5173/a?b#c",
            ),
            (
                "https://localhost:3000",
                "localhost",
                "https://127.0.0.1:3000",
            ),
            (
                "http://LOCALHOST:3000/x",
                "localhost",
                "http://127.0.0.1:3000/x",
            ),
            ("http://[::1]:8000/", "[::1]", "http://localhost:8000/"),
            (
                "http://localhost:5173/",
                "127.0.0.1",
                "http://localhost:5173/",
            ),
            (
                "http://127.0.0.1:5173/",
                "localhost",
                "http://127.0.0.1:5173/",
            ),
            (
                "http://127.0.0.1:5173/",
                "hub.lan",
                "http://127.0.0.1:5173/",
            ),
            (
                "http://example.com:80/",
                "example.com",
                "http://example.com:80/",
            ),
            ("", "127.0.0.1", ""),
            ("not a url", "127.0.0.1", "not a url"),
        ] {
            assert_eq!(away_from_page(url, page), want, "{url} on {page}");
        }
    }
}
