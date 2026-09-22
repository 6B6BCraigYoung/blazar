use std::io::Write;

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

mod tools;

const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Parser)]
#[command(name = "blazar-mcp", version, about = "Blazar 的 MCP server（stdio）")]
struct Cli {
    #[arg(long, env = "BLAZAR_HUB", default_value = "http://127.0.0.1:7777")]
    hub: String,

    #[arg(long, requires = "remote_root")]
    remote_node: Option<String>,

    #[arg(long, requires = "remote_node")]
    remote_root: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blazar=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    if let (Some(node), Some(root)) = (cli.remote_node.clone(), cli.remote_root.clone()) {
        return blazar_mcp::remote::serve(blazar_mcp::remote::RemoteTarget { node, root }).await;
    }
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            tracing::warn!("无法解析的输入行，已跳过");
            continue;
        };
        if let Some(resp) = handle(&cli.hub, &req).await {
            let mut out = std::io::stdout().lock();
            writeln!(out, "{resp}")?;
            out.flush()?;
        }
    }
    Ok(())
}

async fn handle(hub: &str, req: &Value) -> Option<Value> {
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = req.get("id").cloned();

    id.as_ref()?;

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "blazar", "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => Ok(tools::manifest()),
        "tools/call" => call_tool(hub, req.get("params")).await,
        "ping" => Ok(json!({})),
        other => Err(anyhow::anyhow!("不支持的方法: {other}")),
    };

    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": -32603, "message": e.to_string() }
        }),
    })
}

async fn call_tool(hub: &str, params: Option<&Value>) -> Result<Value> {
    let params = params.context("tools/call 缺少 params")?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .context("缺少工具名")?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let tool = tools::find(name).with_context(|| format!("未知工具: {name}"))?;
    let (method, path, body) = (tool.build)(&args)?;
    let text = request(hub, method, &path, body).await?;

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    }))
}

async fn request(hub: &str, method: &str, path: &str, body: Option<Value>) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let base = hub.trim_end_matches('/');
    let addr = base
        .strip_prefix("http://")
        .context("hub 地址必须以 http:// 开头（MCP 只连本机或内网）")?;
    let (host_port, _) = addr.split_once('/').unwrap_or((addr, ""));

    let mut stream = tokio::net::TcpStream::connect(host_port)
        .await
        .with_context(|| format!("连接 hub {host_port} 失败 —— 它在跑吗？"))?;

    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    if !payload.is_empty() {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            payload.len()
        ));
    } else if method == "POST" {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");
    req.push_str(&payload);

    stream.write_all(req.as_bytes()).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;

    let text = String::from_utf8_lossy(&raw);
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .context("hub 返回的不是合法 HTTP 响应")?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let out = decode_body(head, rest);
    if !(200..300).contains(&status) {
        anyhow::bail!("hub 返回 {status}: {}", out.trim());
    }
    Ok(out)
}

fn decode_body(head: &str, rest: &str) -> String {
    if !head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return rest.to_owned();
    }
    let mut out = String::new();
    let mut s = rest;
    loop {
        let Some((size_line, tail)) = s.split_once("\r\n") else {
            break;
        };
        let Ok(n) = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
        else {
            break;
        };
        if n == 0 || tail.len() < n {
            break;
        }
        out.push_str(&tail[..n]);
        s = tail[n..].trim_start_matches("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notifications_get_no_response() {
        let r = handle("http://127.0.0.1:1", &json!({ "method": "initialized" })).await;
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn initialize_reports_tools_capability() {
        let r = handle(
            "http://127.0.0.1:1",
            &json!({ "id": 1, "method": "initialize" }),
        )
        .await
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(r["result"]["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn tools_list_returns_all() {
        let r = handle(
            "http://127.0.0.1:1",
            &json!({ "id": 2, "method": "tools/list" }),
        )
        .await
        .unwrap();
        assert_eq!(
            r["result"]["tools"].as_array().unwrap().len(),
            tools::TOOLS.len()
        );
    }

    #[tokio::test]
    async fn unknown_method_errors_with_id_preserved() {
        let r = handle(
            "http://127.0.0.1:1",
            &json!({ "id": 7, "method": "no/such" }),
        )
        .await
        .unwrap();
        assert_eq!(r["id"], 7);
        assert_eq!(r["error"]["code"], -32603);
    }

    #[tokio::test]
    async fn unreachable_hub_gives_actionable_error() {
        let r = handle(
            "http://127.0.0.1:1",
            &json!({ "id": 3, "method": "tools/call",
                     "params": { "name": "list_workspaces", "arguments": {} } }),
        )
        .await
        .unwrap();
        let msg = r["error"]["message"].as_str().unwrap();
        assert!(msg.contains("它在跑吗"), "错误信息应可操作，实得: {msg}");
    }

    #[test]
    fn chunked_body_is_decoded() {
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked";
        let body = "5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
        assert_eq!(decode_body(head, body), "helloworld");
    }

    #[test]
    fn plain_body_passes_through() {
        assert_eq!(decode_body("HTTP/1.1 200 OK", "{\"a\":1}"), "{\"a\":1}");
    }
}
