use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::api::Shared;

const TTL: Duration = Duration::from_secs(600);

static CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);

fn pick(resp: &Value) -> Value {
    let list = |k: &str| resp.get(k).cloned().unwrap_or(json!([]));
    json!({
        "commands": list("commands"),
        "models": list("models"),
        "agents": list("agents"),
        "output_styles": list("available_output_styles"),
        "output_style": resp.get("output_style").cloned().unwrap_or(Value::Null),
    })
}

async fn fetch(program: &str) -> anyhow::Result<Value> {
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, Into::into);
    let mut child = tokio::process::Command::new(program)
        .args([
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json",
        ])
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("无法写入 claude"))?;
    let req = json!({ "type": "control_request", "request_id": "blazar-init", "request": { "subtype": "initialize" } });
    stdin.write_all(format!("{req}\n").as_bytes()).await?;
    stdin.flush().await?;
    let mut lines = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("无法读取 claude"))?,
    )
    .lines();

    async fn answer(
        lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        id: &str,
    ) -> anyhow::Result<Value> {
        while let Some(line) = lines.next_line().await? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("type").and_then(Value::as_str) == Some("control_response") {
                let r = v.get("response").cloned().unwrap_or(Value::Null);
                if r.get("request_id").and_then(Value::as_str) != Some(id) {
                    continue;
                }
                return Ok(r.get("response").cloned().unwrap_or(r));
            }
        }
        anyhow::bail!("claude 没有应答")
    }
    let resp = tokio::time::timeout(Duration::from_secs(20), answer(&mut lines, "blazar-init"))
        .await
        .map_err(|_| anyhow::anyhow!("20 秒内没拿到 claude 的能力清单"))??;
    let mut out = pick(&resp);

    let started = Instant::now();
    let mut mcp = Vec::new();
    for i in 0.. {
        let id = format!("blazar-mcp-{i}");
        let req = json!({ "type": "control_request", "request_id": id, "request": { "subtype": "mcp_status" } });
        if stdin
            .write_all(format!("{req}\n").as_bytes())
            .await
            .is_err()
            || stdin.flush().await.is_err()
        {
            break;
        }
        let Ok(Ok(r)) = tokio::time::timeout(Duration::from_secs(5), answer(&mut lines, &id)).await
        else {
            break;
        };
        mcp = blazar_core_types::McpServerStatus::list_from(r.get("mcpServers"));
        if !mcp.iter().any(|m| m.status == "pending") || started.elapsed() > Duration::from_secs(8)
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    out["mcp_servers"] = serde_json::to_value(&mcp).unwrap_or_else(|_| json!([]));
    drop(stdin);
    let _ = child.kill().await;
    Ok(out)
}

pub async fn claude(st: &Shared, refresh: bool) -> Option<Value> {
    if !refresh
        && let Ok(g) = CACHE.lock()
        && let Some((at, v)) = g.as_ref()
        && at.elapsed() < TTL
    {
        return Some(v.clone());
    }
    let program = crate::api::local_program(st, "claude").await;
    match fetch(&program).await {
        Ok(v) => {
            if let Ok(mut g) = CACHE.lock() {
                *g = Some((Instant::now(), v.clone()));
            }
            Some(v)
        }
        Err(e) => {
            tracing::warn!(target: "blazar::catalog", "取 claude 能力清单失败: {e:#}");
            None
        }
    }
}

#[derive(Deserialize)]
pub struct CatalogQuery {
    #[serde(default)]
    refresh: bool,
}

pub async fn claude_catalog(
    State(st): State<Shared>,
    Query(q): Query<CatalogQuery>,
) -> Json<Value> {
    Json(claude(&st, q.refresh).await.unwrap_or_else(|| {
        json!({ "commands": [], "models": [], "agents": [], "output_styles": [], "mcp_servers": [], "unavailable": true })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_info_is_dropped() {
        let resp = json!({
            "commands": [{ "name": "review", "description": "d", "argumentHint": "" }],
            "models": [], "agents": [], "available_output_styles": ["default"],
            "account": { "email": "someone@example.com" }, "pid": 1,
        });
        let v = pick(&resp);
        assert!(
            !v.to_string().contains("someone@example.com"),
            "账号信息不能经 API 流出"
        );
        assert_eq!(v["commands"][0]["name"], "review");
    }

    #[test]
    fn mcp_config_is_dropped() {
        let raw = json!([{ "name": "github", "status": "connected", "source": "user",
            "config": { "type": "http", "url": "https://x", "headers": { "Authorization": "Bearer sekret" } } }]);
        let v = serde_json::to_string(&blazar_core_types::McpServerStatus::list_from(Some(&raw)))
            .unwrap();
        assert!(!v.contains("sekret") && !v.contains("https://x"), "{v}");
        assert!(v.contains("github") && v.contains("connected"));
    }
}
