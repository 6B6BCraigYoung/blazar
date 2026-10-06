use std::io::Write;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::tools;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub const SUBCOMMAND: &str = "__mcp-fleet";

pub const SERVER_NAME: &str = "blazar-fleet";

#[must_use]
pub fn hub_from_args(argv: &[String]) -> Option<String> {
    argv.windows(2)
        .find(|w| w[0] == "--hub")
        .map(|w| w[1].clone())
}

pub async fn serve(hub: &str) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            tracing::warn!("无法解析的输入行，已跳过");
            continue;
        };
        if let Some(resp) = handle(hub, &req).await {
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

    let endpoint = blazar_core_types::connection::resolve(hub)?;
    let base = endpoint.url.trim_end_matches('/');
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
    if !endpoint.token.is_empty() {
        req.push_str(&format!("Authorization: Bearer {}\r\n", endpoint.token));
    }
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
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<String> {
    let response = blazar_core_types::http::decode_response(raw)?;
    let status = response.status;
    let out = String::from_utf8(response.body).context("hub 返回的 HTTP 正文不是合法 UTF-8")?;
    if !(200..300).contains(&status) {
        anyhow::bail!("hub 返回 {status}: {}", out.trim());
    }
    Ok(out)
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
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap(), "helloworld");
    }

    #[test]
    fn chunked_response_preserves_split_utf8() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n\xe4\r\n2\r\n\xb8\xad\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap(), "中");
    }

    #[test]
    fn truncated_chunked_response_is_an_error() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhi";
        assert!(parse_response(raw).is_err());
    }

    #[test]
    fn plain_body_passes_through() {
        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\n\r\n{\"a\":1}").unwrap(),
            "{\"a\":1}"
        );
    }
}
