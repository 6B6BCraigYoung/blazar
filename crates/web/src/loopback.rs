fn bare(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

struct Parts<'a> {
    scheme: &'a str,
    host: String,
    port: &'a str,
    tail: &'a str,
}

fn parts(url: &str) -> Option<Parts<'_>> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.rfind(':') {
        Some(at) if !authority[at..].contains(']') => authority.split_at(at),
        _ => (authority, ""),
    };
    Some(Parts {
        scheme,
        host: bare(host),
        port,
        tail,
    })
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

pub fn away_from_page(url: &str, page_host: &str) -> String {
    let Some(p) = parts(url) else {
        return url.to_owned();
    };
    if p.host != bare(page_host) || !is_loopback(&p.host) {
        return url.to_owned();
    }
    let other = if p.host == "localhost" {
        "127.0.0.1"
    } else {
        "localhost"
    };
    format!("{}://{other}{}{}", p.scheme, p.port, p.tail)
}

pub fn loopback_port(url: &str) -> Option<u16> {
    let p = parts(url)?;
    if !matches!(p.scheme, "http" | "https") || !is_loopback(&p.host) {
        return None;
    }
    p.port.strip_prefix(':')?.parse().ok().filter(|&n| n != 0)
}

pub fn forwarded(url: &str, local: u16) -> String {
    match parts(url) {
        Some(p) if loopback_port(url).is_some() => {
            format!("{}://127.0.0.1:{local}{}", p.scheme, p.tail)
        }
        _ => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{away_from_page, forwarded, loopback_port};

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

    #[test]
    fn only_loopback_addresses_with_a_port_are_forwarded() {
        for (url, want) in [
            ("http://localhost:5173/", Some(5173)),
            ("http://127.0.0.1:51234/app?x=1#y", Some(51234)),
            ("https://LOCALHOST:8443", Some(8443)),
            ("http://[::1]:8000/", Some(8000)),
            ("http://localhost/", None),
            ("http://localhost:0/", None),
            ("http://localhost:99999/", None),
            ("http://127.0.0.2:5173/", None),
            ("http://hub.lan:5173/", None),
            ("http://a@localhost:5173/", None),
            ("ws://localhost:5173/", None),
            ("localhost:5173", None),
            ("", None),
        ] {
            assert_eq!(loopback_port(url), want, "{url}");
        }
    }

    #[test]
    fn forwarded_address_points_at_this_computer() {
        for (url, local, want) in [
            ("http://localhost:5173/", 52000, "http://127.0.0.1:52000/"),
            (
                "http://127.0.0.1:51234/app?x=1#y",
                52001,
                "http://127.0.0.1:52001/app?x=1#y",
            ),
            ("https://[::1]:8443", 52002, "https://127.0.0.1:52002"),
            ("http://hub.lan:5173/", 52003, "http://hub.lan:5173/"),
            ("not a url", 52004, "not a url"),
        ] {
            assert_eq!(forwarded(url, local), want, "{url}");
        }
        assert_eq!(
            away_from_page(&forwarded("http://localhost:5173/", 52000), "127.0.0.1"),
            "http://localhost:52000/"
        );
        assert_eq!(
            away_from_page(&forwarded("http://localhost:5173/", 52000), "localhost"),
            "http://127.0.0.1:52000/"
        );
    }
}
