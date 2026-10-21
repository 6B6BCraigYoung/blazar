use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "hub.json";
pub const DEFAULT_PORT: u16 = 7777;
pub const URL_ENV: &str = "BLAZAR_REMOTE_HUB_URL";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum HubMode {
    #[default]
    Embedded,
    Remote {
        ssh: String,
        #[serde(default = "default_port")]
        port: u16,
    },
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Ssh { alias: String, port: u16 },
    Direct(String),
}

impl Target {
    pub fn label(&self) -> &str {
        match self {
            Self::Ssh { alias, .. } => alias,
            Self::Direct(url) => url,
        }
    }
}

impl HubMode {
    pub fn remote(ssh: &str, port: u16) -> Result<Self, String> {
        let mode = Self::Remote {
            ssh: ssh.trim().to_owned(),
            port,
        };
        mode.validate()?;
        Ok(mode)
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Embedded => Ok(()),
            Self::Remote { ssh, port } => {
                blazar_transport::validate_ssh_target(ssh)
                    .map_err(|_| format!("SSH 主机别名「{ssh}」不合法"))?;
                if *port == 0 {
                    return Err("端口要在 1 到 65535 之间".into());
                }
                Ok(())
            }
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mode: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        mode.validate()?;
        Ok(mode)
    }

    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::Embedded,
            Err(e) => {
                tracing::warn!("读不了 {}，改用本机 hub：{e}", path.display());
                return Self::Embedded;
            }
        };
        Self::parse(&text).unwrap_or_else(|e| {
            tracing::warn!("{} 写得不对，改用本机 hub：{e}", path.display());
            Self::Embedded
        })
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        self.validate()?;
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(FILE), text + "\n").map_err(|e| format!("写不了 {FILE}：{e}"))
    }

    pub fn target(&self, url_override: Option<&str>) -> Option<Target> {
        if let Some(raw) = url_override.map(str::trim).filter(|u| !u.is_empty()) {
            match loopback_url(raw) {
                Some(url) => return Some(Target::Direct(url)),
                None => tracing::warn!("{URL_ENV}={raw} 不是带端口的本机 http 地址，已忽略"),
            }
        }
        match self {
            Self::Embedded => None,
            Self::Remote { ssh, port } => Some(Target::Ssh {
                alias: ssh.clone(),
                port: *port,
            }),
        }
    }
}

pub fn loopback_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    let host = match url.host()? {
        url::Host::Domain(d) if d.eq_ignore_ascii_case("localhost") => "localhost".to_owned(),
        url::Host::Ipv4(ip) if ip == std::net::Ipv4Addr::LOCALHOST => ip.to_string(),
        url::Host::Ipv6(ip) if ip == std::net::Ipv6Addr::LOCALHOST => format!("[{ip}]"),
        _ => return None,
    };
    let port = url.port()?;
    let plain = url.scheme() == "http"
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    plain.then(|| format!("http://{host}:{port}"))
}

pub fn ssh_args(alias: &str, local_port: u16, dest: &str, remote_port: u16) -> Vec<String> {
    let mut args: Vec<String> = [
        "-N",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-L",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.push(format!("127.0.0.1:{local_port}:{dest}:{remote_port}"));
    args.push("--".into());
    args.push(alias.to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_mode_fields_default_to_embedded_or_port() {
        assert_eq!(
            HubMode::parse(r#"{"mode":"embedded"}"#),
            Ok(HubMode::Embedded)
        );
        assert_eq!(
            HubMode::parse(r#"{"mode":"remote","ssh":"hub-host"}"#),
            Ok(HubMode::Remote {
                ssh: "hub-host".into(),
                port: 7777
            })
        );
        assert_eq!(
            HubMode::parse(r#"{"mode":"remote","ssh":"me@hub.lan","port":9000}"#),
            Ok(HubMode::Remote {
                ssh: "me@hub.lan".into(),
                port: 9000
            })
        );
    }

    #[test]
    fn bad_settings_are_rejected() {
        for text in [
            "",
            "{}",
            r#"{"mode":"cloud"}"#,
            r#"{"mode":"remote"}"#,
            r#"{"mode":"remote","ssh":"-oProxyCommand=x"}"#,
            r#"{"mode":"remote","ssh":"a b"}"#,
            r#"{"mode":"remote","ssh":"hub","port":0}"#,
            r#"{"mode":"remote","ssh":"hub","port":70000}"#,
            r#"{"mode":"remote","ssh":"hub","port":"7777"}"#,
        ] {
            assert!(HubMode::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn load_falls_back_to_embedded() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(HubMode::load(dir.path()), HubMode::Embedded);
        std::fs::write(dir.path().join(FILE), "not json").unwrap();
        assert_eq!(HubMode::load(dir.path()), HubMode::Embedded);
        let remote = HubMode::remote(" hub-host ", 7000).unwrap();
        remote.save(dir.path()).unwrap();
        assert_eq!(HubMode::load(dir.path()), remote);
        HubMode::Embedded.save(dir.path()).unwrap();
        assert_eq!(HubMode::load(dir.path()), HubMode::Embedded);
    }

    #[test]
    fn url_override_wins_over_the_file() {
        let remote = HubMode::remote("hub-host", 7777).unwrap();
        assert_eq!(
            HubMode::Embedded.target(Some("http://127.0.0.1:7777/")),
            Some(Target::Direct("http://127.0.0.1:7777".into()))
        );
        assert_eq!(HubMode::Embedded.target(Some(" ")), None);
        assert_eq!(
            remote.target(None),
            Some(Target::Ssh {
                alias: "hub-host".into(),
                port: 7777
            })
        );
    }

    #[test]
    fn url_override_must_be_a_loopback_http_address_with_a_port() {
        for (raw, want) in [
            ("http://127.0.0.1:7777", Some("http://127.0.0.1:7777")),
            ("http://localhost:7777/", Some("http://localhost:7777")),
            ("http://LOCALHOST:8080", Some("http://localhost:8080")),
            ("http://[::1]:9000", Some("http://[::1]:9000")),
            ("http://127.0.0.1", None),
            ("https://127.0.0.1:7777", None),
            ("http://127.0.0.2:7777", None),
            ("http://10.0.0.5:7777", None),
            ("http://hub.lan:7777", None),
            ("http://localhost.evil.com:7777", None),
            ("http://a@127.0.0.1:7777", None),
            ("http://127.0.0.1:7777/x", None),
            ("http://127.0.0.1:7777/?a=1", None),
            ("127.0.0.1:7777", None),
        ] {
            assert_eq!(loopback_url(raw).as_deref(), want, "{raw}");
        }
        assert_eq!(HubMode::Embedded.target(Some("http://10.0.0.5:7777")), None);
    }

    #[test]
    fn ssh_forwards_one_loopback_port() {
        assert_eq!(
            ssh_args("hub-host", 51000, "127.0.0.1", 7777),
            [
                "-N",
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-L",
                "127.0.0.1:51000:127.0.0.1:7777",
                "--",
                "hub-host",
            ]
        );
        assert!(
            ssh_args("hub-host", 52000, "localhost", 5173)
                .contains(&"127.0.0.1:52000:localhost:5173".to_owned())
        );
    }
}
