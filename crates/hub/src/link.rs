use blazar_core_types::api::{ClaudeLink, ClaudeLinkState};
use blazar_transport::ExecSpec;
use serde_json::Value;

use crate::api::Shared;
use crate::state::ServerEvent;

const SLOW_MS: u32 = 2000;

const PROBE: &str = r#"PX="${https_proxy:-${HTTPS_PROXY:-${ALL_PROXY:-${all_proxy:-}}}}"
echo "proxy=$PX"
if ! command -v curl >/dev/null 2>&1; then echo nocurl; exit 0; fi
if [ -n "$PX" ]; then
  if curl -s -o /dev/null --max-time 5 "$PX" 2>/dev/null; then echo proxyok=1; else echo proxyok=0; fi
fi
for i in 1 2 3 4 5; do
  echo "s=$(curl -s -o /dev/null -w '%{http_code}:%{time_total}' --max-time 10 https://api.anthropic.com/v1/models 2>/dev/null || echo 000:0)"
done
"#;

fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', r"'\''"))
}

fn host_only(proxy: &str) -> String {
    let after_scheme = proxy.split_once("://").map_or(proxy, |(_, rest)| rest);
    let no_creds = after_scheme.rsplit('@').next().unwrap_or(after_scheme);
    no_creds
        .split('/')
        .next()
        .unwrap_or(no_creds)
        .trim()
        .to_owned()
}

fn median(mut ms: Vec<u32>) -> Option<u32> {
    if ms.is_empty() {
        return None;
    }
    ms.sort_unstable();
    Some(ms[ms.len() / 2])
}

fn code_ok(code: u16) -> bool {
    (200..=299).contains(&code) || code == 401
}

/// Turn the probe's stdout into a link verdict. `ssh_ok` is whether the shell ran at all, so a
/// machine we cannot even reach reads as the machine being down rather than the API.
fn parse(raw: &str, ssh_ok: bool) -> ClaudeLink {
    if !ssh_ok {
        return ClaudeLink {
            state: ClaudeLinkState::MachineDown,
            route: "连不上这台机器".into(),
            latency_ms: None,
            loss: 1.0,
            checked_at: None,
            detail: Some("SSH 连不上，先看机器和组网".into()),
        };
    }
    let mut proxy = String::new();
    let mut proxy_ok: Option<bool> = None;
    let mut no_curl = false;
    let mut samples: Vec<(u16, u32)> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("proxy=") {
            proxy = v.trim().to_owned();
        } else if let Some(v) = line.strip_prefix("proxyok=") {
            proxy_ok = Some(v.trim() == "1");
        } else if line == "nocurl" {
            no_curl = true;
        } else if let Some(v) = line.strip_prefix("s=")
            && let Some((code, time)) = v.split_once(':')
        {
            let code = code.trim().parse::<u16>().unwrap_or(0);
            let ms = time.trim().parse::<f64>().unwrap_or(0.0).max(0.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            samples.push((code, (ms * 1000.0) as u32));
        }
    }
    let route = if proxy.is_empty() {
        "直连".to_owned()
    } else {
        format!("经代理 {}", host_only(&proxy))
    };
    if no_curl {
        return ClaudeLink {
            state: ClaudeLinkState::Unknown,
            route,
            latency_ms: None,
            loss: 0.0,
            checked_at: None,
            detail: Some("这台机器上没有 curl，测不了".into()),
        };
    }
    let total = samples.len();
    if total == 0 {
        return ClaudeLink {
            state: ClaudeLinkState::Unknown,
            route,
            latency_ms: None,
            loss: 0.0,
            checked_at: None,
            detail: Some("没测到结果".into()),
        };
    }
    let oks: Vec<u32> = samples
        .iter()
        .filter(|(c, _)| code_ok(*c))
        .map(|(_, ms)| *ms)
        .collect();
    let ok = oks.len();
    #[allow(clippy::cast_precision_loss)]
    let loss = (total - ok) as f64 / total as f64;
    let latency_ms = median(oks);
    let proxied = !proxy.is_empty();
    let (state, detail) = if ok == 0 {
        if proxied && proxy_ok == Some(false) {
            (
                ClaudeLinkState::ProxyDown,
                Some(format!("代理 {} 连不上", host_only(&proxy))),
            )
        } else {
            (
                ClaudeLinkState::EgressDown,
                Some("连不上 Anthropic（机器和代理都正常时多半是出口被挡）".into()),
            )
        }
    } else if ok < total {
        (
            ClaudeLinkState::Degraded,
            Some(format!("{total} 次里丢了 {}", total - ok)),
        )
    } else if latency_ms.is_some_and(|m| m > SLOW_MS) {
        (ClaudeLinkState::Slow, None)
    } else {
        (ClaudeLinkState::Ok, None)
    };
    ClaudeLink {
        state,
        route,
        latency_ms,
        loss,
        checked_at: Some(chrono::Utc::now().to_rfc3339()),
        detail,
    }
}

async fn run(st: &Shared, node: &str) -> ClaudeLink {
    let env = if node == "local" {
        Vec::new()
    } else {
        crate::proxy::node_net_env(st, node).await
    };
    let exports: String = env
        .iter()
        .map(|(k, v)| format!("export {k}={}\n", quote(v)))
        .collect();
    let script = format!("{exports}{PROBE}");
    match st
        .transport(node)
        .exec(ExecSpec::new("bash").arg("-lc").arg(script))
        .await
    {
        Ok(out) => parse(&out.stdout, true),
        Err(_) => parse("", false),
    }
}

async fn store(st: &Shared, node: &str, link: &ClaudeLink) {
    let Ok(value) = serde_json::to_value(link) else {
        return;
    };
    let existing: Option<String> =
        sqlx::query_scalar("SELECT capabilities FROM nodes WHERE name = ?1")
            .bind(node)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten();
    let mut caps: Value = existing
        .as_deref()
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or(Value::Null);
    if !caps.is_object() {
        caps = serde_json::json!({});
    }
    caps["claude_link"] = value;
    let _ = sqlx::query("UPDATE nodes SET capabilities = ?1 WHERE name = ?2")
        .bind(caps.to_string())
        .bind(node)
        .execute(st.db.pool())
        .await;
    st.emit(ServerEvent::NodesChanged);
}

pub fn stored(caps: &Value) -> Option<ClaudeLink> {
    serde_json::from_value(caps.get("claude_link")?.clone()).ok()
}

pub async fn check(st: &Shared, node: &str) -> ClaudeLink {
    let link = run(st, node).await;
    store(st, node, &link).await;
    link
}

/// Re-check a node's link after a network/login failure, so the machines page reflects why a run
/// just stopped without waiting for the next scheduled sweep.
pub fn check_after_failure(st: &Shared, node: &str) {
    let services = st.services;
    let (st, node) = (st.clone(), node.to_owned());
    services.spawn(async move {
        let _ = check(&st, &node).await;
    });
}

const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

/// Keep the link reading fresh for machines with a running session, so a slow or broken path
/// shows up on the machines page within a few minutes.
pub async fn sweep(st: Shared) {
    loop {
        tokio::time::sleep(SWEEP_EVERY).await;
        let nodes: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT COALESCE(s.node, 'local') FROM sessions s WHERE s.status = 'running'",
        )
        .fetch_all(st.db.pool())
        .await
        .unwrap_or_default();
        for node in nodes {
            let _ = check(&st, &node).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(lines: &[&str]) -> ClaudeLink {
        parse(&lines.join("\n"), true)
    }

    #[test]
    fn a_fast_clean_path_reads_as_ok_with_the_median_latency() {
        let link = probe(&[
            "proxy=",
            "s=200:0.20",
            "s=401:0.30",
            "s=200:0.25",
            "s=200:0.90",
            "s=401:0.28",
        ]);
        assert_eq!(link.state, ClaudeLinkState::Ok);
        assert_eq!(link.route, "直连");
        assert_eq!(link.latency_ms, Some(280));
        assert_eq!(link.loss, 0.0);
    }

    #[test]
    fn a_slow_but_working_path_reads_as_slow() {
        let link = probe(&["proxy=", "s=200:2.5", "s=200:2.6", "s=200:2.7"]);
        assert_eq!(link.state, ClaudeLinkState::Slow);
        assert_eq!(link.latency_ms, Some(2600));
    }

    #[test]
    fn some_drops_read_as_degraded_with_the_loss() {
        let link = probe(&["proxy=", "s=200:0.3", "s=000:0", "s=200:0.3", "s=000:0"]);
        assert_eq!(link.state, ClaudeLinkState::Degraded);
        assert!((link.loss - 0.5).abs() < 1e-9);
        assert_eq!(link.latency_ms, Some(300));
    }

    #[test]
    fn a_dead_proxy_is_named_so_it_is_not_blamed_on_the_api() {
        let link = probe(&[
            "proxy=http://10.0.0.2:7897",
            "proxyok=0",
            "s=000:0",
            "s=000:0",
        ]);
        assert_eq!(link.state, ClaudeLinkState::ProxyDown);
        assert_eq!(link.route, "经代理 10.0.0.2:7897");
        assert!(link.detail.unwrap().contains("10.0.0.2:7897"));
    }

    #[test]
    fn a_reachable_proxy_that_still_cannot_get_out_reads_as_egress() {
        let link = probe(&[
            "proxy=http://user:pass@10.0.0.2:7897",
            "proxyok=1",
            "s=000:0",
            "s=000:0",
        ]);
        assert_eq!(link.state, ClaudeLinkState::EgressDown);
        assert_eq!(link.route, "经代理 10.0.0.2:7897", "凭据不进路由标签");
    }

    #[test]
    fn an_unreachable_machine_blames_the_machine_not_the_api() {
        let link = parse("", false);
        assert_eq!(link.state, ClaudeLinkState::MachineDown);
        assert_eq!(link.loss, 1.0);
    }

    #[test]
    fn a_machine_without_curl_is_unknown_not_broken() {
        let link = probe(&["proxy=", "nocurl"]);
        assert_eq!(link.state, ClaudeLinkState::Unknown);
    }

    #[test]
    fn states_map_to_a_dot_tone() {
        assert_eq!(ClaudeLinkState::Ok.tone(), "ok");
        assert_eq!(ClaudeLinkState::Slow.tone(), "attention");
        assert_eq!(ClaudeLinkState::EgressDown.tone(), "bad");
        assert!(ClaudeLinkState::Slow.healthy());
        assert!(!ClaudeLinkState::Degraded.healthy());
    }
}
