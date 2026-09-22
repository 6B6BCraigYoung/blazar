use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeCapability {
    pub name: String,

    pub labels: Vec<String>,
    pub cpus: u32,
    pub mem_gb: u32,
    pub disk_free_gb: u32,

    pub gpu_vram_mb: Vec<u64>,

    pub load1: f64,

    pub has_ai_egress: bool,

    pub latency_ms: Option<f64>,

    pub running: u32,

    pub max_concurrent: u32,

    pub probed: bool,
}

impl NodeCapability {
    #[must_use]
    pub fn free_slots(&self) -> u32 {
        self.max_concurrent.saturating_sub(self.running)
    }

    #[must_use]
    pub fn max_vram_mb(&self) -> u64 {
        self.gpu_vram_mb.iter().copied().max().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Requirements {
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub min_cpus: u32,
    #[serde(default)]
    pub min_mem_gb: u32,
    #[serde(default)]
    pub min_disk_free_gb: u32,

    #[serde(default)]
    pub min_vram_mb: u64,

    #[serde(default)]
    pub needs_ai_egress: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    MissingLabel(String),
    NotEnoughCpus { need: u32, have: u32 },
    NotEnoughMemory { need: u32, have: u32 },
    NotEnoughDisk { need: u32, have: u32 },
    NotEnoughVram { need: u64, have: u64 },
    NoAiEgress,
    NoFreeSlot,
    NotProbed,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingLabel(l) => write!(f, "缺少标签 {l}"),
            Self::NotEnoughCpus { need, have } => write!(f, "CPU 不足（需 {need}，有 {have}）"),
            Self::NotEnoughMemory { need, have } => write!(f, "内存不足（需 {need}G，有 {have}G）"),
            Self::NotEnoughDisk { need, have } => write!(f, "磁盘不足（需 {need}G，有 {have}G）"),
            Self::NotEnoughVram { need, have } => {
                write!(f, "显存不足（需 {need}MB，单卡最大 {have}MB）")
            }
            Self::NoAiEgress => write!(f, "无法直连 AI API（需出口代理）"),
            Self::NoFreeSlot => write!(f, "并发已满"),
            Self::NotProbed => write!(f, "尚未体检，能力未知"),
        }
    }
}

impl Requirements {
    #[must_use]
    pub fn check(&self, n: &NodeCapability) -> Option<RejectReason> {
        if !n.probed {
            return Some(RejectReason::NotProbed);
        }
        for l in &self.labels {
            if !n.labels.iter().any(|x| x == l) {
                return Some(RejectReason::MissingLabel(l.clone()));
            }
        }
        if n.cpus < self.min_cpus {
            return Some(RejectReason::NotEnoughCpus {
                need: self.min_cpus,
                have: n.cpus,
            });
        }
        if n.mem_gb < self.min_mem_gb {
            return Some(RejectReason::NotEnoughMemory {
                need: self.min_mem_gb,
                have: n.mem_gb,
            });
        }
        if n.disk_free_gb < self.min_disk_free_gb {
            return Some(RejectReason::NotEnoughDisk {
                need: self.min_disk_free_gb,
                have: n.disk_free_gb,
            });
        }
        if n.max_vram_mb() < self.min_vram_mb {
            return Some(RejectReason::NotEnoughVram {
                need: self.min_vram_mb,
                have: n.max_vram_mb(),
            });
        }
        if self.needs_ai_egress && !n.has_ai_egress {
            return Some(RejectReason::NoAiEgress);
        }
        if n.free_slots() == 0 {
            return Some(RejectReason::NoFreeSlot);
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub chosen: Vec<String>,

    pub rejected: BTreeMap<String, String>,

    pub requested: usize,
}

impl Plan {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.chosen.len() == self.requested
    }
}

fn score(n: &NodeCapability) -> f64 {
    let per_core = if n.cpus > 0 {
        n.load1 / f64::from(n.cpus)
    } else {
        n.load1
    };
    let slot_pressure = if n.max_concurrent > 0 {
        f64::from(n.running) / f64::from(n.max_concurrent)
    } else {
        1.0
    };

    let lat = n.latency_ms.unwrap_or(0.0).min(200.0) / 200.0;
    per_core * 10.0 + slot_pressure * 4.0 + lat
}

#[must_use]
pub fn plan(nodes: &[NodeCapability], req: &Requirements, fanout: usize) -> Plan {
    let mut eligible: Vec<&NodeCapability> = Vec::new();
    let mut rejected = BTreeMap::new();

    for n in nodes {
        match req.check(n) {
            None => eligible.push(n),
            Some(reason) => {
                rejected.insert(n.name.clone(), reason.to_string());
            }
        }
    }

    eligible.sort_by(|a, b| {
        score(a)
            .partial_cmp(&score(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });

    Plan {
        chosen: eligible
            .iter()
            .take(fanout)
            .map(|n| n.name.clone())
            .collect(),
        rejected,
        requested: fanout,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    pub task_id: String,
    pub node: String,
    pub token: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl Lease {
    #[must_use]
    pub fn issue(task_id: impl Into<String>, node: impl Into<String>, ttl_secs: i64) -> Self {
        Self {
            task_id: task_id.into(),
            node: node.into(),
            token: uuid::Uuid::now_v7().to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(ttl_secs),
        }
    }

    #[must_use]
    pub fn is_expired(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        now >= self.expires_at
    }

    #[must_use]
    pub fn renew(&self, token: &str, ttl_secs: i64) -> Option<Self> {
        (token == self.token).then(|| Self {
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(ttl_secs),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        name: &str,
        cpus: u32,
        load: f64,
        vram: u64,
        slots: u32,
        running: u32,
    ) -> NodeCapability {
        NodeCapability {
            name: name.into(),
            labels: if vram > 0 { vec!["gpu".into()] } else { vec![] },
            cpus,
            mem_gb: 64,
            disk_free_gb: 200,
            gpu_vram_mb: if vram > 0 { vec![vram, vram] } else { vec![] },
            load1: load,
            has_ai_egress: false,
            latency_ms: Some(30.0),
            running,
            max_concurrent: slots,
            probed: true,
        }
    }

    #[test]
    fn unprobed_node_is_rejected_with_actionable_reason() {
        let mut n = node("fresh", 0, 0.0, 0, 2, 0);
        n.probed = false;
        let req = Requirements {
            labels: vec!["gpu".into()],
            ..Default::default()
        };
        let p = plan(&[n], &req, 1);
        assert_eq!(p.rejected["fresh"], "尚未体检，能力未知");
    }

    #[test]
    fn picks_least_loaded_first() {
        let nodes = vec![
            node("busy", 64, 60.0, 32000, 4, 3),
            node("idle", 64, 1.0, 32000, 4, 0),
            node("mid", 64, 20.0, 32000, 4, 1),
        ];
        let p = plan(&nodes, &Requirements::default(), 2);
        assert_eq!(p.chosen, vec!["idle", "mid"]);
        assert!(p.is_complete());
    }

    #[test]
    fn load_is_normalised_per_core() {
        let nodes = vec![
            node("big", 128, 60.0, 0, 4, 0),
            node("small", 8, 10.0, 0, 4, 0),
        ];
        let p = plan(&nodes, &Requirements::default(), 1);
        assert_eq!(p.chosen, vec!["big"]);
    }

    #[test]
    fn vram_requirement_uses_largest_single_card() {
        let nodes = vec![node("dual32", 64, 0.0, 32_000, 4, 0)];
        let req = Requirements {
            min_vram_mb: 48_000,
            ..Default::default()
        };
        let p = plan(&nodes, &req, 1);
        assert!(p.chosen.is_empty());
        assert!(p.rejected["dual32"].contains("显存不足"));
    }

    #[test]
    fn full_node_is_rejected_with_reason() {
        let nodes = vec![node("full", 64, 0.0, 0, 2, 2)];
        let p = plan(&nodes, &Requirements::default(), 1);
        assert!(p.chosen.is_empty());
        assert_eq!(p.rejected["full"], "并发已满");
    }

    #[test]
    fn egress_requirement_filters_mainland_nodes() {
        let mut inland = node("gpu-cn", 64, 0.0, 32_000, 4, 0);
        inland.has_ai_egress = false;
        let mut overseas = node("vps-us", 8, 0.0, 0, 4, 0);
        overseas.has_ai_egress = true;
        let req = Requirements {
            needs_ai_egress: true,
            ..Default::default()
        };
        let p = plan(&[inland, overseas], &req, 2);
        assert_eq!(p.chosen, vec!["vps-us"]);
        assert!(p.rejected["gpu-cn"].contains("出口代理"));
    }

    #[test]
    fn fanout_never_picks_same_node_twice() {
        let nodes = vec![node("only", 64, 0.0, 32_000, 8, 0)];
        let p = plan(&nodes, &Requirements::default(), 3);
        assert_eq!(p.chosen.len(), 1);
        assert!(!p.is_complete(), "排不满时必须能看出来");
    }

    #[test]
    fn plan_is_deterministic() {
        let nodes = vec![node("b", 64, 1.0, 0, 4, 0), node("a", 64, 1.0, 0, 4, 0)];
        assert_eq!(
            plan(&nodes, &Requirements::default(), 2).chosen,
            vec!["a", "b"]
        );
    }

    #[test]
    fn lease_renew_requires_matching_token() {
        let l = Lease::issue("t1", "gpu1", 60);
        assert!(l.renew("wrong-token", 60).is_none(), "错误 token 不得续租");
        assert!(l.renew(&l.token, 60).is_some());
    }

    #[test]
    fn expired_lease_is_detected() {
        let l = Lease::issue("t1", "gpu1", -1);
        assert!(l.is_expired(chrono::Utc::now()));
    }
}
