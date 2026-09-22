use std::sync::Arc;

use blazar_transport::{ExecSpec, NodeTransport};
use serde::{Deserialize, Serialize};

use crate::{CliInvocation, MeshError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshConfig {
    pub hostname: String,

    pub instance_name: String,

    pub ipv4: String,

    pub network_name: String,
    pub network_secret: String,

    pub peers: Vec<String>,

    #[serde(default)]
    pub listeners: Vec<String>,

    #[serde(default)]
    pub mapped_listeners: Vec<String>,
}

impl MeshConfig {
    #[must_use]
    pub fn to_toml(&self) -> String {
        let list = |items: &[String]| {
            items
                .iter()
                .map(|s| format!("    \"{}\",", s.replace('"', "")))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let mut out = format!(
            "# 由 blazar 生成\nhostname = \"{}\"\ninstance_name = \"{}\"\nipv4 = \"{}\"\n",
            esc(&self.hostname),
            esc(&self.instance_name),
            esc(&self.ipv4),
        );
        if !self.listeners.is_empty() {
            out.push_str(&format!("\nlisteners = [\n{}\n]\n", list(&self.listeners)));
        }
        if !self.mapped_listeners.is_empty() {
            out.push_str("\n# 云上 EIP 是 NAT 映射，不声明公网入口会把内网地址广播出去\n");
            out.push_str(&format!(
                "mapped_listeners = [\n{}\n]\n",
                list(&self.mapped_listeners)
            ));
        }
        if !self.peers.is_empty() {
            for p in &self.peers {
                out.push_str(&format!("\n[[peer]]\nuri = \"{}\"\n", esc(p)));
            }
        }
        out.push_str(&format!(
            "\n[network_identity]\nnetwork_name = \"{}\"\nnetwork_secret = \"{}\"\n",
            esc(&self.network_name),
            esc(&self.network_secret),
        ));
        out
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteEntry {
    pub hostname: String,
    pub ipv4: String,

    pub next_hop: Option<String>,
    pub cost: Option<u32>,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub hostname: String,
    pub virtual_ipv4: String,
    pub public_ip: Option<String>,
    pub nat_type: Option<String>,
    pub version: Option<String>,

    pub peers: Vec<String>,

    #[serde(default)]
    pub network_name: Option<String>,

    #[serde(default)]
    pub listeners: Vec<String>,

    #[serde(default)]
    pub mapped_listeners: Vec<String>,
}

impl NodeStatus {
    #[must_use]
    pub fn entry_points(&self) -> Vec<String> {
        if !self.mapped_listeners.is_empty() {
            return self.mapped_listeners.clone();
        }
        let Some(public) = &self.public_ip else {
            return Vec::new();
        };
        self.listeners
            .iter()
            .filter_map(|l| {
                let (scheme, rest) = l.split_once("://")?;
                let port = rest.rsplit_once(':')?.1.trim_end_matches('/');

                matches!(scheme, "tcp" | "udp").then(|| format!("{scheme}://{public}:{port}"))
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct IssuedCredential {
    pub id: String,
    pub secret: String,
}

impl std::fmt::Debug for IssuedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedCredential")
            .field("id", &self.id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialInfo {
    pub credential_id: String,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub allow_relay: bool,
    #[serde(default)]
    pub expiry_unix: i64,
    #[serde(default)]
    pub reusable: Option<bool>,
}

fn unwrap_instance(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Array(mut items)
            if items.first().is_some_and(|i| i.get("result").is_some()) =>
        {
            items.swap_remove(0)["result"].take()
        }
        other => other,
    }
}

fn first_json(raw: &str) -> Result<serde_json::Value> {
    let start = raw.find(['{', '[']).unwrap_or(0);
    Ok(serde_json::from_str(&raw[start..])?)
}

pub(crate) fn parse_issued(raw: &str) -> Result<IssuedCredential> {
    let v = unwrap_instance(first_json(raw)?);
    let get = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match (get("credential_id"), get("credential_secret")) {
        (Some(id), Some(secret)) => Ok(IssuedCredential { id, secret }),
        _ => Err(MeshError::Engine(
            "管理节点没有返回凭据 —— 它需要 EasyTier ≥ 2.6 且持有 network_secret".into(),
        )),
    }
}

pub(crate) fn parse_credentials(raw: &str) -> Result<Vec<CredentialInfo>> {
    let v = unwrap_instance(first_json(raw)?);
    let list = v
        .get("credentials")
        .cloned()
        .unwrap_or(serde_json::json!([]));
    Ok(serde_json::from_value(list)?)
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub struct MeshAdmin {
    transport: Arc<dyn NodeTransport>,
    invocation: CliInvocation,
}

impl MeshAdmin {
    pub fn new(transport: Arc<dyn NodeTransport>, invocation: CliInvocation) -> Self {
        Self {
            transport,
            invocation,
        }
    }

    async fn cli(&self, args: &[&str]) -> Result<String> {
        let out = self.transport.exec(self.invocation.spec(args)).await?;
        Ok(out.stdout)
    }

    pub async fn status(&self) -> Result<NodeStatus> {
        let raw = self.cli(&["-o", "json", "node"]).await?;
        Ok(parse_status(&unwrap_instance(first_json(&raw)?)))
    }

    pub async fn routes(&self) -> Result<Vec<RouteEntry>> {
        let raw = self.cli(&["-o", "json", "route"]).await?;
        let json = raw.find('[').map_or(raw.as_str(), |i| &raw[i..]);
        let v: Vec<serde_json::Value> = serde_json::from_str(json).map_err(MeshError::Parse)?;
        Ok(v.iter().map(parse_route).collect())
    }

    pub async fn connectors(&self) -> Result<Vec<String>> {
        let raw = self.cli(&["connector", "list"]).await?;
        Ok(raw
            .lines()
            .map(str::trim)
            .filter(|l| l.contains("://"))
            .map(str::to_owned)
            .collect())
    }

    pub async fn add_connector(&self, uri: &str) -> Result<()> {
        self.cli(&["connector", "add", uri]).await?;
        Ok(())
    }

    pub async fn remove_connector(&self, uri: &str) -> Result<()> {
        self.cli(&["connector", "remove", uri]).await?;
        Ok(())
    }

    pub async fn issue_credential(&self, ttl_secs: i64) -> Result<IssuedCredential> {
        let ttl = ttl_secs.to_string();
        let raw = self
            .cli(&[
                "-o",
                "json",
                "credential",
                "generate",
                "--ttl",
                &ttl,
                "--reusable",
                "false",
                "--groups",
                "blazar-member",
            ])
            .await?;
        parse_issued(&raw)
    }

    pub async fn credentials(&self) -> Result<Vec<CredentialInfo>> {
        let raw = self.cli(&["-o", "json", "credential", "list"]).await?;
        parse_credentials(&raw)
    }

    pub async fn revoke_credential(&self, id: &str) -> Result<()> {
        self.cli(&["credential", "revoke", id]).await?;
        Ok(())
    }

    pub async fn stun(&self) -> Result<String> {
        self.cli(&["stun"]).await
    }

    pub async fn write_config(&self, cfg: &MeshConfig, path: &str) -> Result<String> {
        let toml = cfg.to_toml();

        let script = format!(
            "set -e\nmkdir -p \"$(dirname {p})\"\ncat > {p} <<'BLAZAR_EOF'\n{toml}\nBLAZAR_EOF\nchmod 600 {p}\necho {p}",
            p = q(path),
        );
        let out = self
            .transport
            .exec(ExecSpec::new("bash").arg("-lc").arg(script))
            .await?;
        if out.code != 0 {
            return Err(MeshError::Transport(
                blazar_transport::TransportError::Command {
                    code: out.code,
                    stderr: out.stderr,
                },
            ));
        }
        Ok(out.stdout.trim().to_owned())
    }
}

fn nat_name(v: &serde_json::Value) -> Option<String> {
    const NAMES: [&str; 10] = [
        "Unknown",
        "OpenInternet",
        "NoPAT",
        "FullCone",
        "Restricted",
        "PortRestricted",
        "Symmetric",
        "SymUdpFirewall",
        "SymmetricEasyInc",
        "SymmetricEasyDec",
    ];
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .and_then(|i| NAMES.get(usize::try_from(i).ok()?))
            .map(|s| (*s).to_owned()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn config_facts(config: &str) -> (Option<String>, Vec<String>, Vec<String>) {
    let Ok(t) = config.parse::<toml::Table>() else {
        return (None, Vec::new(), Vec::new());
    };
    let list = |k: &str| {
        t.get(k)
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let name = t
        .get("network_identity")
        .and_then(|n| n.get("network_name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    (name, list("listeners"), list("mapped_listeners"))
}

fn parse_status(v: &serde_json::Value) -> NodeStatus {
    let get = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let strings = |v: Option<&serde_json::Value>| -> Vec<String> {
        v.and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let stun = v.get("stun_info");
    let (network_name, cfg_listeners, mapped_listeners) = config_facts(
        v.get("config")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    );
    let listeners = match strings(v.get("listeners")) {
        l if l.is_empty() => cfg_listeners,
        l => l,
    };

    NodeStatus {
        hostname: get("hostname").unwrap_or_default(),
        virtual_ipv4: get("ipv4_addr")
            .or_else(|| get("virtual_ipv4"))
            .or_else(|| get("ipv4"))
            .unwrap_or_default(),
        public_ip: stun
            .and_then(|s| s.get("public_ip"))
            .and_then(|p| match p {
                serde_json::Value::Array(a) => a.first()?.as_str().map(str::to_owned),
                serde_json::Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .or_else(|| get("public_ip")),
        nat_type: stun
            .and_then(|s| s.get("udp_nat_type"))
            .and_then(nat_name)
            .or_else(|| get("nat_type")),
        version: get("version"),
        peers: strings(v.get("peers")),
        network_name,
        listeners,
        mapped_listeners,
    }
}

fn parse_route(v: &serde_json::Value) -> RouteEntry {
    let s = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let next = s("next_hop_hostname").filter(|x| !x.is_empty() && x != "-");
    RouteEntry {
        hostname: s("hostname").unwrap_or_default(),
        ipv4: s("ipv4").unwrap_or_default(),
        next_hop: next,
        cost: v
            .get("cost")
            .and_then(|c| c.as_u64().or_else(|| c.as_str()?.parse().ok()))
            .map(|c| c as u32),
        version: s("version").unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MeshConfig {
        MeshConfig {
            hostname: "gpu-new".into(),
            instance_name: "demo-mesh".into(),
            ipv4: "10.99.0.30".into(),
            network_name: "demo-mesh".into(),
            network_secret: "s3cr$t".into(),
            peers: vec!["tcp://203.0.113.10:11010".into()],
            listeners: vec![],
            mapped_listeners: vec![],
        }
    }

    #[test]
    fn config_renders_identity_and_peer() {
        let t = cfg().to_toml();
        assert!(t.contains(r#"ipv4 = "10.99.0.30""#));
        assert!(t.contains("[[peer]]"));
        assert!(t.contains(r#"uri = "tcp://203.0.113.10:11010""#));
        assert!(t.contains("[network_identity]"));
    }

    #[test]
    fn mapped_listeners_carry_the_nat_warning() {
        let mut c = cfg();
        c.listeners = vec!["tcp://0.0.0.0:11010".into()];
        c.mapped_listeners = vec!["tcp://1.2.3.4:11010".into()];
        let t = c.to_toml();
        assert!(t.contains("mapped_listeners"));

        assert!(t.contains("NAT 映射"));
    }

    #[test]
    fn secret_with_dollar_survives_heredoc() {
        let t = cfg().to_toml();
        assert!(t.contains("s3cr$t"), "密钥应原样保留: {t}");
    }

    const NODE_JSON: &str = r#"{
        "peer_id": 599746586, "ipv4_addr": "10.99.0.1/24", "proxy_cidrs": [],
        "hostname": "hub-host",
        "stun_info": {"udp_nat_type": 5, "tcp_nat_type": 0, "last_update_time": 0,
                      "public_ip": ["203.0.113.10"], "min_port": 0, "max_port": 0},
        "inst_id": "x", "listeners": ["tcp://0.0.0.0:11010", "udp://0.0.0.0:11010", "wg://0.0.0.0:11011"],
        "config": "hostname = \"hub-host\"\nmapped_listeners = [\"tcp://203.0.113.10:11010\"]\n[network_identity]\nnetwork_name = \"demo-mesh\"\nnetwork_secret = \"TOPSECRET\"\n",
        "version": "2.6.4"
    }"#;

    #[test]
    fn node_status_reads_2_6_fields_without_leaking_secret() {
        let st = parse_status(&serde_json::from_str(NODE_JSON).unwrap());
        assert_eq!(st.virtual_ipv4, "10.99.0.1/24");
        assert_eq!(st.public_ip.as_deref(), Some("203.0.113.10"));
        assert_eq!(st.nat_type.as_deref(), Some("PortRestricted"));
        assert_eq!(st.network_name.as_deref(), Some("demo-mesh"));
        assert_eq!(st.entry_points(), vec!["tcp://203.0.113.10:11010"]);

        assert!(!serde_json::to_string(&st).unwrap().contains("TOPSECRET"));
    }

    #[test]
    fn entry_points_fall_back_to_stun_ip() {
        let mut st = parse_status(&serde_json::from_str(NODE_JSON).unwrap());
        st.mapped_listeners.clear();
        assert_eq!(
            st.entry_points(),
            vec!["tcp://203.0.113.10:11010", "udp://203.0.113.10:11010"]
        );
    }

    #[test]
    fn issued_credential_parses_single_and_multi_instance() {
        let single = r#"{"credential_id": "c-1", "credential_secret": "S"}"#;
        let multi = r#"[{"instance_id":"x","instance_name":"demo-mesh",
            "result":{"credential_id":"c-2","credential_secret":"T"}}]"#;
        assert_eq!(parse_issued(single).unwrap().id, "c-1");
        assert_eq!(parse_issued(multi).unwrap().secret, "T");

        assert!(parse_issued("{}").is_err());
        let dbg = format!("{:?}", parse_issued(single).unwrap());
        assert!(!dbg.contains("\"S\""), "{dbg}");
    }

    #[test]
    fn credential_list_parses() {
        let raw = r#"warn: something
        {"credentials":[{"credential_id":"c-1","groups":["blazar-member"],
          "allow_relay":false,"expiry_unix":1900000000,"allowed_proxy_cidrs":[],"reusable":false}]}"#;
        let list = parse_credentials(raw).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].reusable, Some(false));
        assert!(
            parse_credentials(r#"{"credentials":[]}"#)
                .unwrap()
                .is_empty()
        );

        assert!(parse_credentials("{}").unwrap().is_empty());
    }

    #[test]
    fn route_parses_direct_and_relayed() {
        let direct = serde_json::json!({
            "hostname": "gpu1", "ipv4": "10.99.0.11",
            "next_hop_hostname": "-", "cost": "1", "version": "2.6.4"
        });
        let relayed = serde_json::json!({
            "hostname": "far", "ipv4": "10.99.0.99",
            "next_hop_hostname": "hub-host", "cost": 2, "version": "2.6.4"
        });
        assert_eq!(parse_route(&direct).next_hop, None, "'-' 应视为直连");
        assert_eq!(parse_route(&relayed).next_hop.as_deref(), Some("hub-host"));
        assert_eq!(parse_route(&direct).cost, Some(1), "cost 可能是字符串");
        assert_eq!(parse_route(&relayed).cost, Some(2), "cost 也可能是数字");
    }
}
