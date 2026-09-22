use anyhow::{Context, Result};
use serde_json::{Value, json};

pub struct Reply {
    pub status: u16,
    pub body: Value,
}

pub async fn request(hub: &str, method: &str, path: &str, body: Option<Value>) -> Result<Reply> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = hub
        .trim_end_matches('/')
        .strip_prefix("http://")
        .context("hub 地址必须以 http:// 开头")?;
    let host_port = addr.split('/').next().unwrap_or(addr);
    let mut stream = tokio::net::TcpStream::connect(host_port)
        .await
        .with_context(|| format!("连不上 hub {host_port} —— Blazar 在跑吗？（桌面端默认 127.0.0.1:61528，可用 --hub 或 BLAZAR_HUB 指定）"))?;
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    if !payload.is_empty() {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            payload.len()
        ));
    } else if method != "GET" {
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
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = decode(head, rest);
    Ok(Reply {
        status,
        body: serde_json::from_str(&body).unwrap_or(Value::String(body)),
    })
}

fn decode(head: &str, rest: &str) -> String {
    if !head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return rest.to_owned();
    }
    let (mut out, mut s) = (String::new(), rest);
    while let Some((size, tail)) = s.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16) else {
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

fn enc(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn fail_of(r: &Reply) -> anyhow::Error {
    anyhow::anyhow!(
        "hub 返回 {}：{}",
        r.status,
        r.body["error"]
            .as_str()
            .map_or_else(|| r.body.to_string(), str::to_owned)
    )
}

async fn resolve_workspace(hub: &str, name_or_id: &str) -> Result<String> {
    let r = request(hub, "GET", "/api/state", None).await?;
    let list = r.body["workspaces"].as_array().cloned().unwrap_or_default();
    if list.iter().any(|w| w["id"] == name_or_id) {
        return Ok(name_or_id.to_owned());
    }
    let hits: Vec<&Value> = list.iter().filter(|w| w["name"] == name_or_id).collect();
    match hits.as_slice() {
        [one] => Ok(one["id"].as_str().unwrap_or_default().to_owned()),
        [] => anyhow::bail!("没有叫「{name_or_id}」的工作区"),
        many => anyhow::bail!(
            "有 {} 个工作区都叫「{name_or_id}」，请用 id：{}",
            many.len(),
            many.iter()
                .map(|w| format!(
                    "{}@{}",
                    w["id"].as_str().unwrap_or_default(),
                    w["node"].as_str().unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join("，")
        ),
    }
}

pub async fn wait(hub: &str, workspace: &str, until: &str, timeout: u64) -> Result<i32> {
    let id = resolve_workspace(hub, workspace).await?;
    let until = if until == "blocked" {
        "blocked"
    } else {
        "idle"
    };
    let r = request(
        hub,
        "GET",
        &format!(
            "/api/workspaces/{}/wait?until={until}&timeout={timeout}",
            enc(&id)
        ),
        None,
    )
    .await?;
    if r.body["reached"] == json!(true) {
        if let Some(t) = r.body["last_reply"].as_str() {
            println!("{t}");
        }
        eprintln!("[{}]", r.body["activity"].as_str().unwrap_or_default());
        return Ok(0);
    }
    eprintln!("{}", r.body["error"].as_str().unwrap_or("没等到"));
    Ok(match r.body["error_code"].as_str() {
        Some("agent_prompt_stalled") => 2,
        Some("wait_timeout") => 3,
        _ => 1,
    })
}

pub struct PromptOpts<'a> {
    pub workspace: &'a str,
    pub text: &'a str,
    pub agent: Option<&'a str>,
    pub model: Option<&'a str>,
    pub permission_mode: Option<&'a str>,
    pub new_chat: bool,
    pub wait: bool,
    pub until: &'a str,
    pub timeout: u64,
}

pub async fn prompt(hub: &str, o: PromptOpts<'_>) -> Result<i32> {
    let id = resolve_workspace(hub, o.workspace).await?;
    let body = json!({
        "text": o.text, "resume": !o.new_chat, "agent": o.agent, "model": o.model,
        "permission_mode": o.permission_mode, "brain": "local", "wait_secs": 20,
    });
    let r = request(
        hub,
        "POST",
        &format!("/api/workspaces/{}/prompt", enc(&id)),
        Some(body),
    )
    .await?;
    if !(200..300).contains(&r.status) {
        return Err(fail_of(&r));
    }
    if r.body["admitted"] == json!(false) {
        anyhow::bail!(
            "{}。{}",
            r.body["reason"].as_str().unwrap_or("工作区正忙"),
            r.body["hint"].as_str().unwrap_or_default()
        );
    }
    if r.body["activity"]["started"] == json!(false) {
        anyhow::bail!(
            "agent 没能启动：{}",
            r.body["activity"]["reason"].as_str().unwrap_or_default()
        );
    }
    eprintln!(
        "会话 {} 已开工",
        r.body["session_id"].as_str().unwrap_or_default()
    );
    if o.wait {
        return wait(hub, &id, o.until, o.timeout).await;
    }
    Ok(0)
}

pub async fn tasks(
    hub: &str,
    action: &str,
    arg: Option<&str>,
    workspace: Option<&str>,
) -> Result<i32> {
    match action {
        "list" => {
            let r = request(hub, "GET", "/api/tasks", None).await?;
            for t in r.body.as_array().into_iter().flatten() {
                println!(
                    "{:<8} {:<12} {}  {}",
                    t["key"].as_str().unwrap_or_default(),
                    t["status"].as_str().unwrap_or_default(),
                    t["title"].as_str().unwrap_or_default(),
                    t["workspace_name"]
                        .as_str()
                        .map(|w| format!("@{w}"))
                        .unwrap_or_default()
                );
            }
        }
        "create" => {
            let title = arg.context("用法：blazar task create <标题> [--workspace 名字或 id]")?;
            let ws = match workspace {
                Some(w) => Some(resolve_workspace(hub, w).await?),
                None => None,
            };
            let r = request(
                hub,
                "POST",
                "/api/tasks",
                Some(json!({ "title": title, "workspace_id": ws, "runtime": "claude" })),
            )
            .await?;
            if !(200..300).contains(&r.status) {
                return Err(fail_of(&r));
            }
            println!(
                "{} {}",
                r.body["key"].as_str().unwrap_or_default(),
                r.body["id"].as_str().unwrap_or_default()
            );
        }
        "start" => {
            let key = arg.context("用法：blazar task start <BLZ-n 或任务 id>")?;
            let all = request(hub, "GET", "/api/tasks", None).await?;
            let id = all
                .body
                .as_array()
                .into_iter()
                .flatten()
                .find(|t| t["key"] == key || t["id"] == key)
                .and_then(|t| t["id"].as_str())
                .with_context(|| format!("没有任务 {key}"))?
                .to_owned();
            let r = request(hub, "POST", &format!("/api/tasks/{}/start", enc(&id)), None).await?;
            if !(200..300).contains(&r.status) {
                return Err(fail_of(&r));
            }
            println!(
                "已开始：会话 {}",
                r.body["session_id"].as_str().unwrap_or_default()
            );
        }
        _ => anyhow::bail!("task 的动作只有 list / create / start"),
    }
    Ok(0)
}

pub async fn autopilots(hub: &str, action: &str, arg: Option<&str>) -> Result<i32> {
    let all = request(hub, "GET", "/api/autopilots", None).await?;
    match action {
        "list" => {
            for a in all.body.as_array().into_iter().flatten() {
                println!(
                    "{:<8} {:<24} {:<16} next={}",
                    a["status"].as_str().unwrap_or_default(),
                    a["name"].as_str().unwrap_or_default(),
                    a["cron"].as_str().unwrap_or("-"),
                    a["next_run_at"].as_str().unwrap_or("-")
                );
            }
        }
        "run" => {
            let name = arg.context("用法：blazar autopilot run <名字或 id>")?;
            let id = all
                .body
                .as_array()
                .into_iter()
                .flatten()
                .find(|a| a["name"] == name || a["id"] == name)
                .and_then(|a| a["id"].as_str())
                .with_context(|| format!("没有自动化 {name}"))?
                .to_owned();
            let r = request(
                hub,
                "POST",
                &format!("/api/autopilots/{}/run", enc(&id)),
                None,
            )
            .await?;
            println!(
                "{} {}",
                r.body["status"].as_str().unwrap_or_default(),
                r.body["reason"].as_str().unwrap_or_default()
            );
        }
        _ => anyhow::bail!("autopilot 的动作只有 list / run"),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_bodies_are_reassembled() {
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked";
        assert_eq!(
            decode(head, "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"),
            "hello world"
        );
        assert_eq!(decode("HTTP/1.1 200 OK", "plain"), "plain");
    }

    #[test]
    fn path_segments_are_escaped() {
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
        assert_eq!(enc("01a0-ba2f"), "01a0-ba2f");
    }
}
