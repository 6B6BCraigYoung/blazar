use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Serialize;

use crate::invite::{self, INSTANCE_NAME, InviteError};

type Result<T> = std::result::Result<T, InviteError>;

fn bad(field: &'static str, reason: impl Into<String>) -> InviteError {
    InviteError::Field {
        field,
        reason: reason.into(),
    }
}

const KNOWN_KEYS: &[&str] = &[
    "netns",
    "hostname",
    "instance_name",
    "instance_id",
    "ipv4",
    "ipv6",
    "ipv6_public_addr_provider",
    "ipv6_public_addr_auto",
    "ipv6_public_addr_prefix",
    "dhcp",
    "network_identity",
    "listeners",
    "mapped_listeners",
    "exit_nodes",
    "peer",
    "proxy_network",
    "vpn_portal_config",
    "routes",
    "socks5_proxy",
    "port_forward",
    "secure_mode",
    "flags",
    "acl",
    "tcp_whitelist",
    "udp_whitelist",
    "stun_servers",
    "stun_servers_v6",
    "credential_file",
    "source",
];

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    NetworkSecret,

    Credential,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSummary {
    pub network_name: String,
    pub auth: AuthMode,
    pub hostname: Option<String>,

    pub ipv4: Option<String>,
    pub dhcp: bool,
    pub peers: Vec<String>,
    pub listeners: Vec<String>,

    pub features: Vec<String>,

    pub warnings: Vec<String>,
}

#[derive(Clone)]
pub struct ImportedConfig {
    pub summary: ConfigSummary,

    toml: String,
}

impl std::fmt::Debug for ImportedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedConfig")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

fn str_of<'a>(t: &'a toml::Table, k: &str) -> Option<&'a str> {
    t.get(k).and_then(toml::Value::as_str)
}

fn str_list(t: &toml::Table, k: &str, field: &'static str) -> Result<Vec<String>> {
    match t.get(k) {
        None => Ok(Vec::new()),
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| bad(field, "应为字符串数组"))
            })
            .collect(),
        Some(_) => Err(bad(field, "应为数组")),
    }
}

fn count(t: &toml::Table, k: &str) -> usize {
    t.get(k).and_then(toml::Value::as_array).map_or(0, Vec::len)
}

impl ImportedConfig {
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 256 * 1024 {
            return Err(InviteError::Format("配置文件过大".into()));
        }
        let mut t: toml::Table =
            text.trim_start_matches('\u{feff}')
                .parse()
                .map_err(|e: toml::de::Error| {
                    InviteError::Format(format!("不是合法的 TOML：{}", e.message()))
                })?;

        let unknown: Vec<&str> = t
            .keys()
            .map(String::as_str)
            .filter(|k| !KNOWN_KEYS.contains(k))
            .collect();
        if !unknown.is_empty() {
            return Err(InviteError::Format(format!(
                "有 EasyTier 不认识的配置项：{}（拼写错误的项会被静默忽略，请修正后再导入）",
                unknown.join("、")
            )));
        }

        let ident = t
            .get("network_identity")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| bad("network_identity", "缺少 [network_identity]"))?;
        let network_name = str_of(ident, "network_name")
            .ok_or_else(|| bad("network_identity", "缺少 network_name"))?
            .to_owned();
        check_name("network_name", &network_name)?;
        let has_secret = str_of(ident, "network_secret").is_some_and(|s| !s.is_empty())
            || ident.contains_key("network_secret_digest");

        let secure = t.get("secure_mode").and_then(toml::Value::as_table);
        let private_key = secure
            .filter(|s| s.get("enabled").and_then(toml::Value::as_bool) == Some(true))
            .and_then(|s| str_of(s, "local_private_key"))
            .map(str::to_owned);
        let auth = match (has_secret, &private_key) {
            (true, _) => AuthMode::NetworkSecret,
            (false, Some(_)) => AuthMode::Credential,
            (false, None) => {
                return Err(bad(
                    "network_identity",
                    "既没有 network_secret，也没有启用 secure_mode 的凭据私钥，无法入网",
                ));
            }
        };

        if let Some(sk) = &private_key {
            let raw = B64
                .decode(sk.trim())
                .map_err(|_| bad("secure_mode", "local_private_key 不是 base64"))?;
            let bytes = <[u8; 32]>::try_from(raw.as_slice())
                .map_err(|_| bad("secure_mode", "local_private_key 长度应为 32 字节"))?;
            let public = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(bytes));
            let derived = B64.encode(public.as_bytes());
            let sm = t
                .get_mut("secure_mode")
                .and_then(toml::Value::as_table_mut)
                .expect("上面已确认存在");
            match sm.get("local_public_key").and_then(toml::Value::as_str) {
                Some(pk) if pk != derived => {
                    return Err(bad("secure_mode", "local_public_key 与私钥不匹配"));
                }
                Some(_) => {}
                None => {
                    sm.insert("local_public_key".into(), derived.into());
                }
            }
        }

        let hostname = str_of(&t, "hostname").map(str::to_owned);
        if let Some(h) = &hostname {
            check_name("hostname", h)?;
        }
        let ipv4 = str_of(&t, "ipv4").map(str::to_owned);
        if let Some(ip) = &ipv4 {
            invite::parse_ipv4_cidr(ip)?;
        }
        let dhcp = t
            .get("dhcp")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);

        let mut peers = Vec::new();
        if let Some(list) = t.get("peer") {
            let arr = list
                .as_array()
                .ok_or_else(|| bad("peer", "应为 [[peer]] 数组"))?;
            for p in arr {
                let uri = p
                    .get("uri")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| bad("peer", "每个 [[peer]] 都要有 uri"))?;
                invite::validate_peer_uri(uri)?;
                peers.push(uri.to_owned());
            }
        }
        let listeners = str_list(&t, "listeners", "listeners")?;
        for l in listeners
            .iter()
            .chain(&str_list(&t, "mapped_listeners", "mapped_listeners")?)
        {
            invite::validate_peer_uri(l)?;
        }

        let mut warnings = Vec::new();
        if peers.is_empty() && listeners.is_empty() {
            warnings.push("没有 [[peer]] 也没有 listeners，入不了网。".into());
        }
        if auth == AuthMode::NetworkSecret {
            warnings.push(
                "含整个网络的 network_secret，适合枢纽 / 管理员；给同事请用邀请文件。".into(),
            );
        }
        if ipv4.is_none() && !dhcp {
            warnings.push("没有 ipv4 也没开 dhcp，本机将没有虚拟地址。".into());
        }
        if t.contains_key("netns") {
            warnings.push("指定了 netns，虚拟网卡在默认命名空间里看不到。".into());
        }

        let mut features = Vec::new();
        let mut feat = |n: usize, name: &str| {
            if n > 0 {
                features.push(format!("{name} ×{n}"));
            }
        };
        feat(count(&t, "proxy_network"), "子网代理");
        feat(count(&t, "port_forward"), "端口转发");
        feat(count(&t, "exit_nodes"), "出口节点");
        feat(count(&t, "routes"), "手动路由");
        feat(
            count(&t, "tcp_whitelist") + count(&t, "udp_whitelist"),
            "端口白名单",
        );
        for (k, name) in [
            ("acl", "ACL"),
            ("vpn_portal_config", "WireGuard 门户"),
            ("socks5_proxy", "SOCKS5 代理"),
            ("credential_file", "凭据签发（credential_file）"),
        ] {
            if t.contains_key(k) {
                features.push(name.to_owned());
            }
        }
        if let Some(flags) = t.get("flags").and_then(toml::Value::as_table)
            && !flags.is_empty()
        {
            let mut keys: Vec<&str> = flags.keys().map(String::as_str).collect();
            keys.sort_unstable();
            features.push(format!("flags：{}", keys.join(", ")));
        }

        t.entry("instance_name")
            .or_insert_with(|| INSTANCE_NAME.into());

        let toml = format!(
            "# 由 Blazar 导入。含组网密钥，仅 root 可读，请勿外传。\n{}",
            toml::to_string(&t).map_err(|e| InviteError::Format(e.to_string()))?
        );
        Ok(Self {
            summary: ConfigSummary {
                network_name,
                auth,
                hostname,
                ipv4,
                dhcp,
                peers,
                listeners,
                features,
                warnings,
            },
            toml,
        })
    }

    #[must_use]
    pub fn engine_config(&self) -> &str {
        &self.toml
    }
}

fn check_name(field: &'static str, s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(bad(field, "只能包含字母、数字、- _ .，长度 1..=64"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUB: &str = r#"
hostname = "hub-host"
ipv4 = "10.99.0.1"
listeners = ["tcp://0.0.0.0:11010", "udp://0.0.0.0:11010"]
mapped_listeners = ["tcp://203.0.113.10:11010"]
credential_file = "/etc/easytier/credentials.json"

[network_identity]
network_name = "demo-mesh"
network_secret = "TOPSECRET"

[[proxy_network]]
cidr = "192.168.1.0/24"

[flags]
latency_first = true
"#;

    #[test]
    fn full_easytier_config_is_accepted_and_summarised() {
        let c = ImportedConfig::parse(HUB).unwrap();
        let s = &c.summary;
        assert_eq!(s.network_name, "demo-mesh");
        assert_eq!(s.auth, AuthMode::NetworkSecret);
        assert_eq!(s.ipv4.as_deref(), Some("10.99.0.1"));
        assert!(s.features.iter().any(|f| f.starts_with("子网代理")));
        assert!(s.features.iter().any(|f| f.contains("latency_first")));
        assert!(s.warnings.iter().any(|w| w.contains("network_secret")));

        assert!(c.engine_config().contains("192.168.1.0/24"));
        assert!(c.engine_config().contains("instance_name = \"blazar\""));

        assert!(!serde_json::to_string(s).unwrap().contains("TOPSECRET"));
        assert!(!format!("{c:?}").contains("TOPSECRET"));
    }

    #[test]
    fn typo_keys_are_rejected_not_silently_ignored() {
        let e =
            ImportedConfig::parse(&format!("listener = [\"tcp://0.0.0.0:1\"]\n{HUB}")).unwrap_err();
        assert!(e.to_string().contains("listener"), "{e}");
    }

    #[test]
    fn credential_config_gets_its_public_key_filled_in() {
        let t = r#"
hostname = "zhang-mbp"
dhcp = true
[network_identity]
network_name = "demo-mesh"
[[peer]]
uri = "tcp://203.0.113.10:11010"
[secure_mode]
enabled = true
local_private_key = "dwdtCnMYpX08FsFyUbJmRd9ML4frwJkqsXf7pR25LCo="
"#;
        let c = ImportedConfig::parse(t).unwrap();
        assert_eq!(c.summary.auth, AuthMode::Credential);
        assert!(
            c.engine_config()
                .contains("hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo=")
        );
        let mismatched = t.replace(
            "enabled = true",
            "enabled = true\nlocal_public_key = \"AAAA\"",
        );
        assert!(ImportedConfig::parse(&mismatched).is_err());
    }

    #[test]
    fn configs_that_cannot_join_are_rejected() {
        let no_auth =
            "[network_identity]\nnetwork_name = \"x\"\n[[peer]]\nuri = \"tcp://1.2.3.4:1\"\n";
        assert!(ImportedConfig::parse(no_auth).is_err());
        assert!(ImportedConfig::parse("hostname = 1 = 2").is_err());
        let bad_peer = HUB.replace("credential_file", "[[peer]]\nuri = \"tcp://h:1\\\"; x\"\n#");
        assert!(ImportedConfig::parse(&bad_peer).is_err());
        let bad_host = HUB.replace("hub-host", "a b");
        assert!(ImportedConfig::parse(&bad_host).is_err());
    }
}
