use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub url: String,
    pub token: String,
}

pub fn profile_name(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or("default")
        .to_owned()
}

pub fn data_root() -> io::Result<PathBuf> {
    directories::ProjectDirs::from("ai", "blazar", "Blazar")
        .map(|p| p.data_dir().to_owned())
        .ok_or_else(|| io::Error::other("无法确定 Blazar 数据目录"))
}

pub fn resolve(requested: &str) -> io::Result<Endpoint> {
    let root = data_root()?;
    let profile = profile_name(std::env::var("BLAZAR_PROFILE").ok().as_deref());
    let paths = match std::env::var_os("BLAZAR_HUB_SESSION") {
        Some(path) => vec![PathBuf::from(path)],
        None => vec![
            root.join("profiles").join(profile).join("hub.session"),
            root.join("hub/hub.session"),
        ],
    };
    let token = std::env::var("BLAZAR_HUB_TOKEN").ok();
    resolve_from(&paths, requested, token.as_deref())
}

pub fn resolve_from(
    paths: &[PathBuf],
    requested: &str,
    token: Option<&str>,
) -> io::Result<Endpoint> {
    let requested = requested.trim_end_matches('/');
    let explicit = requested != "auto" && !requested.is_empty();
    if explicit && !valid_url(requested) {
        return Err(io::Error::other(
            "hub 地址必须是 http://主机:端口，不能带路径或凭据",
        ));
    }
    let discovered = paths
        .iter()
        .filter_map(|path| read(path).ok())
        .find(|e| !explicit || e.url == requested);
    let endpoint = match discovered {
        Some(mut endpoint) => {
            if let Some(token) = token {
                endpoint.token = token.to_owned();
            }
            endpoint
        }
        None if explicit => Endpoint {
            url: requested.to_owned(),
            token: token.unwrap_or_default().to_owned(),
        },
        None => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "找不到运行中的 Blazar；启动桌面应用，或用 --hub 和 BLAZAR_HUB_TOKEN 指定连接",
            ));
        }
    };
    if endpoint.token.bytes().any(|b| b.is_ascii_control()) {
        return Err(io::Error::other("hub 会话令牌包含非法字符"));
    }
    Ok(endpoint)
}

fn valid_url(url: &str) -> bool {
    url.strip_prefix("http://").is_some_and(|authority| {
        !authority.is_empty()
            && !authority.contains(['/', '?', '#', '@'])
            && !authority
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
    })
}

pub fn read(path: &Path) -> io::Result<Endpoint> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err(io::Error::other("hub 会话文件不是普通的小型文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other("hub 会话文件权限过宽"));
        }
    }
    #[cfg(windows)]
    crate::private_storage::verify_file(path)?;
    let endpoint: Endpoint = serde_json::from_slice(&std::fs::read(path)?)?;
    if !valid_url(&endpoint.url)
        || endpoint.token.len() != 64
        || !endpoint.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(io::Error::other("hub 会话文件无效"));
    }
    Ok(endpoint)
}

pub fn write(path: &Path, endpoint: &Endpoint) -> io::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = crate::private_storage::open(&temporary, &options)?;
        file.write_all(&serde_json::to_vec(endpoint)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_addresses_never_receive_an_unrelated_discovered_token() {
        let dir = std::env::temp_dir().join(format!("blazar-connection-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        crate::private_storage::protect_directory(&dir).unwrap();
        let path = dir.join("hub.session");
        let endpoint = Endpoint {
            url: "http://127.0.0.1:45678".into(),
            token: "a".repeat(64),
        };
        write(&path, &endpoint).unwrap();
        let found = resolve_from(std::slice::from_ref(&path), "auto", None).unwrap();
        assert_eq!(found.url, endpoint.url);
        assert_eq!(found.token, endpoint.token);
        let other = resolve_from(
            std::slice::from_ref(&path),
            "http://203.0.113.10:7777",
            None,
        )
        .unwrap();
        assert!(other.token.is_empty());
        let explicit = resolve_from(&[path], "http://127.0.0.1:45678/", None).unwrap();
        assert_eq!(explicit.token, endpoint.token);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reject_header_injection_and_profile_traversal() {
        assert!(resolve_from(&[], "http://hub-host:7777\r\nx: y", None).is_err());
        assert!(resolve_from(&[], "http://hub-host:7777", Some("x\r\ny")).is_err());
        assert_eq!(profile_name(Some("../../other")), "default");
        assert_eq!(profile_name(Some("lab-1")), "lab-1");
        assert!(resolve_from(&[], "auto", None).is_err());
    }
}
