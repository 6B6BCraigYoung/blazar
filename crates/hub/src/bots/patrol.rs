use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use blazar_transport::ExecSpec;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::api::Shared;

pub const SERVICE_EVERY: Duration = Duration::from_secs(120);
pub const DISK_EVERY: Duration = Duration::from_secs(30 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
pub const DISK_FREE_GB: f64 = 20.0;
const DF: &str = "d=/; [ -d /System/Volumes/Data ] && d=/System/Volumes/Data; df -Pk \"$d\" | awk 'NR==2{print $3, $4}'";
pub const DISK_FRESH_SECS: i64 = 2 * 3600;

#[derive(Default)]
pub struct PatrolState {
    services: Mutex<HashMap<String, Check>>,
    ready: AtomicBool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub up: bool,
    pub at: DateTime<Utc>,
    pub why: String,
}

impl PatrolState {
    #[must_use]
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn check(&self, key: &str) -> Option<Check> {
        self.services.lock().ok()?.get(key).cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Port { host: String, port: u16 },
    Url(String),
    Process(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub key: String,
    pub label: String,
    pub node: String,
    pub probe: Probe,
}

impl Service {
    #[must_use]
    pub fn how(&self) -> String {
        match &self.probe {
            Probe::Port { host, port } => format!("{host}:{port} 端口"),
            Probe::Url(u) => u.clone(),
            Probe::Process(p) => format!("进程「{p}」"),
        }
    }
}

fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

pub fn parse(key: &str, value: &Value) -> Result<Service, String> {
    let node = value["node"].as_str().unwrap_or("local").trim().to_owned();
    let label = value["label"]
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(key)
        .to_owned();
    let probe = if let Some(port) = value["port"].as_u64() {
        let port = u16::try_from(port)
            .ok()
            .filter(|p| *p > 0)
            .ok_or("port must be 1-65535")?;
        let host = value["host"].as_str().unwrap_or("127.0.0.1").to_owned();
        if !valid_host(&host) {
            return Err("host may only contain letters, digits, dots, dashes and colons".into());
        }
        Probe::Port { host, port }
    } else if let Some(url) = value["url"].as_str() {
        if !(url.starts_with("http://") || url.starts_with("https://")) || url.len() > 500 {
            return Err("url must start with http:// or https://".into());
        }
        Probe::Url(url.to_owned())
    } else if let Some(p) = value["process"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Probe::Process(p.chars().take(200).collect())
    } else {
        return Err("a service needs one of port, url or process".into());
    };
    if node.is_empty() {
        return Err("node can't be empty".into());
    }
    Ok(Service {
        key: key.to_owned(),
        label,
        node,
        probe,
    })
}

#[must_use]
pub fn self_safe(pattern: &str) -> String {
    let mut chars = pattern.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => format!("[{c}]{}", chars.as_str()),
        _ => pattern.to_owned(),
    }
}

fn spec(probe: &Probe) -> ExecSpec {
    match probe {
        Probe::Port { host, port } => ExecSpec::new("bash")
            .arg("-c")
            .arg(format!(": </dev/tcp/{host}/{port}")),
        Probe::Url(u) => ExecSpec::new("curl").args(["-fsS", "-m", "8", "-o", "/dev/null", u]),
        Probe::Process(p) => ExecSpec::new("pgrep").args(["-f".to_owned(), self_safe(p)]),
    }
}

pub async fn services(st: &Shared) -> Vec<Service> {
    super::facts::list(st, "", 1000)
        .await
        .into_iter()
        .filter(|f| f.kind == "service")
        .filter_map(|f| parse(&f.key, &f.value).ok())
        .collect()
}

async fn online(st: &Shared, node: &str) -> bool {
    node == "local"
        || sqlx::query_scalar::<_, String>("SELECT status FROM nodes WHERE name = ?1")
            .bind(node)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .as_deref()
            == Some("online")
}

async fn check_one(st: &Shared, s: &Service) -> Option<Check> {
    if !online(st, &s.node).await {
        return None;
    }
    let t = st.transport(&s.node);
    let out = tokio::time::timeout(CHECK_TIMEOUT, t.exec(spec(&s.probe)))
        .await
        .ok()?;
    let out = out.ok()?;
    let why = if out.code == 0 {
        String::new()
    } else {
        let err = out.stderr.trim();
        match &s.probe {
            Probe::Port { .. } => "端口连不上".to_owned(),
            Probe::Process(_) => "没有找到进程".to_owned(),
            Probe::Url(_) if err.is_empty() => format!("请求失败（退出码 {}）", out.code),
            Probe::Url(_) => err.chars().take(160).collect(),
        }
    };
    Some(Check {
        up: out.code == 0,
        at: Utc::now(),
        why,
    })
}

pub async fn sweep_services(st: &Shared) {
    let list = services(st).await;
    let checks = futures::future::join_all(list.iter().map(|s| check_one(st, s))).await;
    if let Ok(mut map) = st.bots.patrol.services.lock() {
        map.retain(|k, _| list.iter().any(|s| &s.key == k));
        for (s, c) in list.iter().zip(checks) {
            match c {
                Some(c) => {
                    map.insert(s.key.clone(), c);
                }
                None => {
                    map.remove(&s.key);
                }
            }
        }
    }
    st.bots.patrol.ready.store(true, Ordering::SeqCst);
}

#[must_use]
pub fn parse_df(out: &str) -> Option<(f64, f64)> {
    let mut it = out.split_whitespace();
    let used: f64 = it.next()?.parse().ok()?;
    let avail: f64 = it.next()?.parse().ok()?;
    let total = used + avail;
    (total > 0.0).then(|| {
        let pct = (used / total * 1000.0).round() / 10.0;
        let free_gb = (avail / 1_048_576.0 * 10.0).round() / 10.0;
        (pct, free_gb)
    })
}

pub async fn sweep_disks(st: &Shared) {
    let nodes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM nodes WHERE transport = 'local' OR (transport = 'ssh' AND status = 'online')",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default();
    let probes = nodes.iter().map(|n| async move {
        let t = st.transport(n);
        let spec = ExecSpec::new("sh").arg("-c").arg(DF);
        let out = tokio::time::timeout(CHECK_TIMEOUT, t.exec(spec))
            .await
            .ok()?
            .ok()?;
        (out.code == 0).then_some(())?;
        parse_df(&out.stdout).map(|d| (n.clone(), d))
    });
    let now = Utc::now().to_rfc3339();
    let mut changed = false;
    for (name, (pct, free_gb)) in futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
    {
        changed |= sqlx::query(
            "UPDATE nodes SET capabilities = json_set(
                 CASE WHEN json_valid(capabilities) THEN capabilities ELSE '{}' END,
                 '$.disk_used_pct', ?1, '$.disk_free_gb', ?2, '$.disk_checked_at', ?3)
             WHERE name = ?4",
        )
        .bind(pct)
        .bind(free_gb)
        .bind(&now)
        .bind(&name)
        .execute(st.db.pool())
        .await
        .is_ok_and(|r| r.rows_affected() > 0);
    }
    if changed {
        st.emit(crate::state::ServerEvent::NodesChanged);
    }
}

pub async fn watch(st: Shared) {
    let mut disks: Option<Instant> = None;
    loop {
        sweep_services(&st).await;
        if disks.is_none_or(|t| t.elapsed() >= DISK_EVERY) {
            disks = Some(Instant::now());
            sweep_disks(&st).await;
        }
        tokio::time::sleep(SERVICE_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_service_is_checked_by_port_url_or_process() {
        let s = parse(
            "demo-ingest",
            &json!({ "node": "ali", "port": 8787, "label": "demo 入库" }),
        )
        .unwrap();
        assert_eq!(
            s.probe,
            Probe::Port {
                host: "127.0.0.1".into(),
                port: 8787
            }
        );
        assert_eq!((s.node.as_str(), s.label.as_str()), ("ali", "demo 入库"));
        assert!(matches!(
            parse("k", &json!({ "url": "https://example.com/health" }))
                .unwrap()
                .probe,
            Probe::Url(_)
        ));
        assert!(parse("k", &json!({ "url": "file:///etc/passwd" })).is_err());
        assert!(parse("k", &json!({ "port": 70000 })).is_err());
        assert!(parse("k", &json!({ "port": 80, "host": "a;rm -rf /" })).is_err());
        assert!(parse("k", &json!({ "node": "ali" })).is_err());
    }

    #[test]
    fn a_process_pattern_never_matches_the_check_itself() {
        assert_eq!(self_safe("uvicorn app:main"), "[u]vicorn app:main");
        assert_eq!(self_safe("^python"), "^python");
    }

    #[test]
    fn df_reports_used_percent_and_free_gigabytes() {
        assert_eq!(parse_df("1717986918 65011712\n"), Some((96.4, 62.0)));
        assert_eq!(parse_df(""), None);
        assert_eq!(parse_df("0 0"), None);
    }
}
