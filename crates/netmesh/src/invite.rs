use std::net::Ipv4Addr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

pub const INVITE_EXT: &str = "blazar";

pub const INVITE_VERSION: u32 = 1;

pub const MAX_INVITE_BYTES: usize = 16 * 1024;

const README: &str = "用 Blazar 打开此文件（双击，或拖进 Blazar 窗口）即可加入团队组网。\
文件内含你个人的组网凭据，请勿转发；丢失或泄露请联系管理员吊销后重新签发。";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InviteError {
    #[error("不是 Blazar 邀请文件：{0}")]
    Format(String),
    #[error("邀请文件版本 {0} 不受支持，请升级 Blazar")]
    Version(u32),
    #[error("邀请文件字段 `{field}` 无效：{reason}")]
    Field { field: &'static str, reason: String },
    #[error("邀请已于 {0} 过期，请找管理员重新签发")]
    Expired(String),
}

type Result<T> = std::result::Result<T, InviteError>;

fn bad(field: &'static str, reason: impl Into<String>) -> InviteError {
    InviteError::Field {
        field,
        reason: reason.into(),
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Invite {
    #[serde(rename = "_readme", default)]
    pub readme: String,

    pub blazar_invite: u32,
    pub network_name: String,

    pub credential_id: String,

    pub credential: String,

    pub peers: Vec<String>,

    pub hostname: String,

    #[serde(default)]
    pub ipv4: Option<String>,

    #[serde(default)]
    pub issued_by: String,
    pub issued_at: i64,
    pub expires_at: i64,
    #[serde(default)]
    pub note: String,
}

impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invite")
            .field("network_name", &self.network_name)
            .field("credential_id", &self.credential_id)
            .field("credential", &"<redacted>")
            .field("peers", &self.peers)
            .field("hostname", &self.hostname)
            .field("ipv4", &self.ipv4)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InviteSummary {
    pub network_name: String,
    pub credential_id: String,
    pub peers: Vec<String>,
    pub hostname: String,
    pub ipv4: Option<String>,
    pub issued_by: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct InviteDraft {
    pub network_name: String,
    pub peers: Vec<String>,
    pub hostname: String,
    pub ipv4: Option<String>,
    pub issued_by: String,
    pub note: String,
}

impl InviteDraft {
    pub fn seal(
        self,
        credential_id: String,
        credential: String,
        issued_at: i64,
        expires_at: i64,
    ) -> Result<Invite> {
        let inv = Invite {
            readme: README.to_owned(),
            blazar_invite: INVITE_VERSION,
            network_name: self.network_name,
            credential_id,
            credential,
            peers: self.peers,
            hostname: self.hostname,
            ipv4: self.ipv4,
            issued_by: self.issued_by,
            issued_at,
            expires_at,
            note: self.note,
        };
        inv.validate(issued_at)?;
        Ok(inv)
    }
}

impl Invite {
    pub fn parse(text: &str, now: i64) -> Result<Self> {
        if text.len() > MAX_INVITE_BYTES {
            return Err(InviteError::Format("文件过大".into()));
        }
        let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| InviteError::Format(format!("不是合法 JSON（{e}）")))?;
        let version = v
            .get("blazar_invite")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| InviteError::Format("缺少 blazar_invite 标识".into()))?;
        if version != u64::from(INVITE_VERSION) {
            return Err(InviteError::Version(
                u32::try_from(version).unwrap_or(u32::MAX),
            ));
        }
        let inv: Invite =
            serde_json::from_value(v).map_err(|e| InviteError::Format(e.to_string()))?;
        inv.validate(now)?;
        Ok(inv)
    }

    fn validate(&self, now: i64) -> Result<()> {
        check_name("network_name", &self.network_name, 64)?;
        check_name("hostname", &self.hostname, 63)?;
        if self.credential_id.is_empty()
            || self.credential_id.len() > 64
            || !self
                .credential_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(bad("credential_id", "应为 UUID"));
        }
        self.private_key()?;
        if self.peers.is_empty() {
            return Err(bad("peers", "至少需要一个入网地址"));
        }
        if self.peers.len() > 16 {
            return Err(bad("peers", "入网地址过多"));
        }
        for p in &self.peers {
            check_peer_uri(p)?;
        }
        if let Some(ip) = &self.ipv4 {
            parse_ipv4_cidr(ip)?;
        }
        for (field, s, max) in [
            ("issued_by", &self.issued_by, 64),
            ("note", &self.note, 500),
        ] {
            if s.chars().count() > max || s.chars().any(char::is_control) {
                return Err(bad(field, "过长或含控制字符"));
            }
        }
        if self.expires_at <= now {
            let when = chrono::DateTime::from_timestamp(self.expires_at, 0)
                .map_or_else(|| self.expires_at.to_string(), |t| t.to_rfc3339());
            return Err(InviteError::Expired(when));
        }
        Ok(())
    }

    fn private_key(&self) -> Result<[u8; 32]> {
        let raw = B64
            .decode(self.credential.trim())
            .map_err(|_| bad("credential", "不是 base64"))?;
        <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| bad("credential", "长度应为 32 字节"))
    }

    pub fn public_key(&self) -> Result<String> {
        let secret = x25519_dalek::StaticSecret::from(self.private_key()?);
        Ok(B64.encode(x25519_dalek::PublicKey::from(&secret).as_bytes()))
    }

    #[must_use]
    pub fn summary(&self) -> InviteSummary {
        InviteSummary {
            network_name: self.network_name.clone(),
            credential_id: self.credential_id.clone(),
            peers: self.peers.clone(),
            hostname: self.hostname.clone(),
            ipv4: self.ipv4.clone(),
            issued_by: self.issued_by.clone(),
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            note: self.note.clone(),
        }
    }

    #[must_use]
    pub fn to_file(&self) -> String {
        serde_json::to_string_pretty(self).expect("邀请结构总能序列化")
    }

    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.{INVITE_EXT}", self.hostname)
    }

    pub fn engine_config(&self) -> Result<String> {
        let mut out = String::from(
            "# 由 Blazar 根据邀请文件生成。含本机的组网凭据，仅 root 可读，请勿外传。\n",
        );
        out.push_str(&format!("hostname = {}\n", toml_str(&self.hostname)));
        out.push_str(&format!("instance_name = {}\n", toml_str(INSTANCE_NAME)));
        match &self.ipv4 {
            Some(ip) => {
                out.push_str(&format!("ipv4 = {}\n", toml_str(ip)));
                out.push_str("dhcp = false\n");
            }
            None => out.push_str("dhcp = true\n"),
        }
        out.push_str(&format!(
            "\n[network_identity]\nnetwork_name = {}\n",
            toml_str(&self.network_name)
        ));
        for p in &self.peers {
            out.push_str(&format!("\n[[peer]]\nuri = {}\n", toml_str(p)));
        }
        out.push_str(&format!(
            "\n[secure_mode]\nenabled = true\nlocal_private_key = {}\nlocal_public_key = {}\n",
            toml_str(self.credential.trim()),
            toml_str(&self.public_key()?),
        ));
        Ok(out)
    }
}

pub const INSTANCE_NAME: &str = "blazar";

fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn check_name(field: &'static str, s: &str, max: usize) -> Result<()> {
    if s.is_empty() || s.len() > max {
        return Err(bad(field, format!("长度应为 1..={max}")));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(bad(field, "只能包含字母、数字、- _ ."));
    }
    Ok(())
}

const PEER_SCHEMES: &[&str] = &["tcp", "udp", "quic", "wg", "ws", "wss", "faketcp"];

pub fn validate_peer_uri(uri: &str) -> Result<()> {
    check_peer_uri(uri)
}

fn check_peer_uri(uri: &str) -> Result<()> {
    let (scheme, rest) = uri
        .split_once("://")
        .ok_or_else(|| bad("peers", format!("`{uri}` 不是 协议://主机:端口")))?;
    if !PEER_SCHEMES.contains(&scheme) {
        return Err(bad("peers", format!("不支持的协议 `{scheme}`")));
    }

    let hostport = rest.split('/').next().unwrap_or_default();
    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| bad("peers", format!("`{uri}` 缺少端口")))?;
    if port.parse::<u16>().map_or(true, |p| p == 0) {
        return Err(bad("peers", format!("`{uri}` 端口无效")));
    }
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':'));
    let rest_ok = rest
        .chars()
        .all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '\'' | '\\' | '`' | '$'));
    if !host_ok || !rest_ok {
        return Err(bad("peers", format!("`{uri}` 含非法字符")));
    }
    Ok(())
}

pub fn parse_ipv4_cidr(s: &str) -> Result<(Ipv4Addr, u8)> {
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (
            a,
            p.parse::<u8>()
                .map_err(|_| bad("ipv4", format!("`{s}` 前缀无效")))?,
        ),
        None => (s, 24),
    };
    let addr: Ipv4Addr = addr
        .parse()
        .map_err(|_| bad("ipv4", format!("`{s}` 不是 IPv4 地址")))?;
    if !(8..=30).contains(&prefix) {
        return Err(bad("ipv4", format!("`{s}` 前缀应在 8..=30")));
    }
    Ok((addr, prefix))
}

#[must_use]
pub fn allocate_ipv4(subnet_of: Ipv4Addr, prefix: u8, taken: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    if !(8..=30).contains(&prefix) {
        return None;
    }
    let mask = u32::MAX << (32 - u32::from(prefix));
    let base = u32::from(subnet_of) & mask;
    let size = 1u32 << (32 - u32::from(prefix));
    (30..size - 1)
        .map(|off| Ipv4Addr::from(base + off))
        .find(|ip| !taken.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    fn draft() -> InviteDraft {
        InviteDraft {
            network_name: "demo-mesh".into(),
            peers: vec!["tcp://203.0.113.10:11010".into()],
            hostname: "zhang-mbp".into(),
            ipv4: Some("10.99.0.40/24".into()),
            issued_by: "admin".into(),
            note: String::new(),
        }
    }

    fn invite() -> Invite {
        draft()
            .seal(
                "0b6f3a8e-1c2d-4e5f-8a9b-0c1d2e3f4a5b".into(),
                KEY.into(),
                NOW,
                NOW + 86_400,
            )
            .unwrap()
    }

    #[test]
    fn round_trips_through_file() {
        let text = invite().to_file();
        let back = Invite::parse(&text, NOW).unwrap();
        assert_eq!(back.summary(), invite().summary());
        assert_eq!(invite().file_name(), "zhang-mbp.blazar");
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let dbg = format!("{:?}", invite());
        assert!(!dbg.contains(KEY), "{dbg}");
        assert!(
            !serde_json::to_string(&invite().summary())
                .unwrap()
                .contains(KEY)
        );
    }

    #[test]
    fn expired_invite_is_rejected() {
        let text = invite().to_file();
        assert!(matches!(
            Invite::parse(&text, NOW + 86_400),
            Err(InviteError::Expired(_))
        ));
    }

    #[test]
    fn rejects_non_invites_and_future_versions() {
        assert!(matches!(
            Invite::parse("{}", NOW),
            Err(InviteError::Format(_))
        ));
        assert!(matches!(
            Invite::parse("not json", NOW),
            Err(InviteError::Format(_))
        ));
        let mut v: serde_json::Value = serde_json::from_str(&invite().to_file()).unwrap();
        v["blazar_invite"] = 2.into();
        assert_eq!(
            Invite::parse(&v.to_string(), NOW).unwrap_err(),
            InviteError::Version(2)
        );
    }

    #[test]
    fn hostile_fields_cannot_reach_the_root_config() {
        let base: serde_json::Value = serde_json::from_str(&invite().to_file()).unwrap();
        let cases: &[(&str, serde_json::Value)] = &[
            ("hostname", "a\"\n[secure_mode]".into()),
            ("hostname", "../../etc".into()),
            ("network_name", "x y".into()),
            (
                "peers",
                serde_json::json!(["tcp://h:1\"\nlisteners=[\"tcp://0.0.0.0:1\"]"]),
            ),
            ("peers", serde_json::json!(["file:///etc/passwd:1"])),
            ("peers", serde_json::json!(["tcp://host:0"])),
            ("peers", serde_json::json!(["tcp://$(id):1"])),
            ("peers", serde_json::json!([])),
            ("ipv4", "10.0.0.1/33".into()),
            ("ipv4", "not-ip".into()),
            ("credential", "c2hvcnQ=".into()),
            ("credential_id", "a b".into()),
            ("note", "line\u{0007}bell".into()),
        ];
        for (field, val) in cases {
            let mut v = base.clone();
            v[*field] = val.clone();
            assert!(
                Invite::parse(&v.to_string(), NOW).is_err(),
                "{field} = {val} 应被拒绝"
            );
        }
    }

    #[test]
    fn engine_config_is_credential_mode() {
        let toml = invite().engine_config().unwrap();

        assert!(toml.contains("[network_identity]\nnetwork_name = \"demo-mesh\"\n"));
        assert!(!toml.contains("network_secret"));
        assert!(toml.contains("[secure_mode]\nenabled = true"));
        assert!(toml.contains(&format!("local_private_key = \"{KEY}\"")));
        assert!(toml.contains("local_public_key = \""));
        assert!(toml.contains("ipv4 = \"10.99.0.40/24\"\ndhcp = false"));
        assert!(toml.contains("uri = \"tcp://203.0.113.10:11010\""));
        assert!(!toml.contains("listeners"), "成员节点不应监听端口");
    }

    #[test]
    fn missing_ipv4_means_dhcp() {
        let mut d = draft();
        d.ipv4 = None;
        let inv = d.seal("id-1".into(), KEY.into(), NOW, NOW + 10).unwrap();
        assert!(inv.engine_config().unwrap().contains("dhcp = true"));
    }

    #[test]
    fn public_key_matches_x25519_derivation() {
        let sk = "dwdtCnMYpX08FsFyUbJmRd9ML4frwJkqsXf7pR25LCo=";
        let pk = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo=";
        let mut inv = invite();
        inv.credential = sk.into();
        assert_eq!(inv.public_key().unwrap(), pk);
    }

    #[test]
    fn allocation_skips_taken_and_infrastructure_range() {
        let net: Ipv4Addr = "10.99.0.1".parse().unwrap();
        let taken: Vec<Ipv4Addr> = ["10.99.0.30", "10.99.0.31"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(
            allocate_ipv4(net, 24, &taken),
            Some("10.99.0.32".parse().unwrap())
        );

        let all: Vec<Ipv4Addr> = (0..=255u8).map(|i| Ipv4Addr::new(10, 99, 0, i)).collect();
        assert_eq!(allocate_ipv4(net, 24, &all), None);
    }
}
