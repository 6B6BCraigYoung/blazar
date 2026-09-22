use std::sync::Arc;

use blazar_transport::{ExecSpec, NodeTransport};
use serde::{Deserialize, Serialize};

pub mod config_import;
pub mod engine;
pub mod invite;
pub mod manage;
pub use invite::{Invite, InviteDraft, InviteError, InviteSummary};
pub use manage::{CredentialInfo, IssuedCredential, MeshAdmin, MeshConfig, NodeStatus, RouteEntry};

#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error("执行 easytier-cli 失败: {0}")]
    Transport(#[from] blazar_transport::TransportError),

    #[error("解析 easytier-cli 输出失败: {0}")]
    Parse(#[from] serde_json::Error),

    #[error(transparent)]
    Invite(#[from] InviteError),

    #[error("{0}")]
    Engine(String),
}

pub type Result<T> = std::result::Result<T, MeshError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshPeer {
    pub hostname: String,
    pub ipv4: String,

    pub cost: String,

    pub latency_ms: Option<f64>,
    pub loss_rate: Option<f64>,
    pub nat_type: String,
    pub tunnel_proto: String,
    pub version: String,
    pub peer_id: String,
}

impl MeshPeer {
    #[must_use]
    pub fn is_self(&self) -> bool {
        self.cost.eq_ignore_ascii_case("local")
    }

    #[must_use]
    pub fn is_direct(&self) -> bool {
        self.cost.eq_ignore_ascii_case("p2p")
    }

    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.is_self() || (self.latency_ms.is_some() && self.loss_rate.unwrap_or(1.0) < 0.2)
    }
}

#[derive(Debug, Deserialize)]
struct RawPeer {
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    ipv4: String,
    #[serde(default)]
    cost: String,
    #[serde(default)]
    lat_ms: String,
    #[serde(default)]
    loss_rate: String,
    #[serde(default)]
    nat_type: String,
    #[serde(default)]
    tunnel_proto: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    id: String,
}

fn parse_dash_number(raw: &str) -> Option<f64> {
    let t = raw.trim().trim_end_matches('%');
    if t.is_empty() || t == "-" {
        return None;
    }
    t.parse().ok()
}

fn to_peer(raw: RawPeer) -> MeshPeer {
    MeshPeer {
        hostname: raw.hostname,
        ipv4: raw.ipv4,
        cost: raw.cost,
        latency_ms: parse_dash_number(&raw.lat_ms),

        loss_rate: parse_dash_number(&raw.loss_rate).map(|v| v / 100.0),
        nat_type: raw.nat_type,
        tunnel_proto: raw.tunnel_proto,
        version: raw.version,
        peer_id: raw.id,
    }
}

pub fn parse_peers(json: &str) -> Result<Vec<MeshPeer>> {
    let raw: Vec<RawPeer> = serde_json::from_str(json)?;
    Ok(raw.into_iter().map(to_peer).collect())
}

#[derive(Debug, Clone)]
pub enum CliInvocation {
    Direct {
        program: String,
        rpc: Option<String>,
    },

    Docker {
        container: String,
        program: String,
    },
}

impl CliInvocation {
    #[must_use]
    pub fn direct() -> Self {
        Self::Direct {
            program: "easytier-cli".to_owned(),
            rpc: None,
        }
    }

    #[must_use]
    pub fn docker(container: impl Into<String>) -> Self {
        Self::Docker {
            container: container.into(),
            program: "easytier-cli".to_owned(),
        }
    }

    pub fn spec(&self, args: &[&str]) -> ExecSpec {
        match self {
            Self::Direct { program, rpc } => {
                let mut spec = ExecSpec::new(program);
                if let Some(rpc) = rpc {
                    spec = spec.arg("-p").arg(rpc);
                }
                spec.args(args.iter().copied())
            }
            Self::Docker { container, program } => ExecSpec::new("docker")
                .arg("exec")
                .arg(container)
                .arg(program)
                .args(args.iter().copied()),
        }
    }
}

pub struct EasyTierMesh {
    transport: Arc<dyn NodeTransport>,
    invocation: CliInvocation,
}

impl EasyTierMesh {
    pub fn new(transport: Arc<dyn NodeTransport>, invocation: CliInvocation) -> Self {
        Self {
            transport,
            invocation,
        }
    }

    pub async fn peers(&self) -> Result<Vec<MeshPeer>> {
        let spec = self.invocation.spec(&["-o", "json", "peer"]);
        let out = self.transport.exec(spec).await?;

        let json = out
            .stdout
            .find('[')
            .map_or(out.stdout.as_str(), |i| &out.stdout[i..]);
        parse_peers(json)
    }

    pub async fn usable_peers(&self) -> Result<Vec<MeshPeer>> {
        Ok(self
            .peers()
            .await?
            .into_iter()
            .filter(|p| !p.is_self() && p.is_healthy())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"cidr":"10.99.0.1/24","ipv4":"10.99.0.1","hostname":"relay-1",
       "cost":"Local","lat_ms":"-","loss_rate":"-","rx_bytes":"-","tx_bytes":"-",
       "tunnel_proto":"-","nat_type":"PortRestricted","id":"599746586","version":"2.6.4"},
      {"cidr":"10.99.0.11/24","ipv4":"10.99.0.11","hostname":"gpu-1",
       "cost":"p2p","lat_ms":"34.65","loss_rate":"0.0%","rx_bytes":"519.96 MB",
       "tx_bytes":"1.20 GB","tunnel_proto":"tcp","nat_type":"Symmetric",
       "id":"3421826093","version":"2.6.4"},
      {"cidr":"10.99.0.12/24","ipv4":"10.99.0.12","hostname":"gpu-2",
       "cost":"relay","lat_ms":"42.17","loss_rate":"35.0%","rx_bytes":"1.19 GB",
       "tx_bytes":"4.80 GB","tunnel_proto":"tcp","nat_type":"Symmetric",
       "id":"1","version":"2.6.4"}
    ]"#;

    #[test]
    fn parses_real_easytier_output() {
        let peers = parse_peers(SAMPLE).unwrap();
        assert_eq!(peers.len(), 3);
        assert_eq!(peers[0].hostname, "relay-1");
        assert_eq!(peers[1].ipv4, "10.99.0.11");
    }

    #[test]
    fn dash_means_absent_not_zero() {
        let peers = parse_peers(SAMPLE).unwrap();
        assert!(peers[0].is_self());
        assert_eq!(peers[0].latency_ms, None);
        assert_eq!(peers[0].loss_rate, None);
    }

    #[test]
    fn loss_rate_normalised_from_percent() {
        let peers = parse_peers(SAMPLE).unwrap();
        assert_eq!(peers[1].loss_rate, Some(0.0));
        assert_eq!(peers[2].loss_rate, Some(0.35));
    }

    #[test]
    fn unhealthy_peer_is_excluded() {
        let peers = parse_peers(SAMPLE).unwrap();

        assert!(peers[1].is_healthy(), "p2p 0% 丢包应健康");
        assert!(!peers[2].is_healthy(), "35% 丢包应判为不健康");
    }

    #[test]
    fn direct_vs_relay_is_distinguished() {
        let peers = parse_peers(SAMPLE).unwrap();
        assert!(peers[1].is_direct());
        assert!(!peers[2].is_direct());
    }

    #[test]
    fn direct_invocation_targets_our_rpc_port() {
        let inv = CliInvocation::Direct {
            program: "/opt/blazar/mesh/bin/easytier-cli".into(),
            rpc: Some("127.0.0.1:15898".into()),
        };
        let spec = inv.spec(&["-o", "json", "node"]);
        assert_eq!(
            spec.args,
            vec!["-p", "127.0.0.1:15898", "-o", "json", "node"]
        );
    }

    #[test]
    fn docker_invocation_wraps_cli() {
        let spec = CliInvocation::docker("easytier").spec(&["-o", "json", "peer"]);
        assert_eq!(spec.program, "docker");
        assert_eq!(
            spec.args,
            vec!["exec", "easytier", "easytier-cli", "-o", "json", "peer"]
        );
    }
}
