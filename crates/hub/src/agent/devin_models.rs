use std::process::Stdio;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::api::Shared;

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

const FRESH: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub images: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing {
    pub models: Vec<Model>,
    pub current: Option<String>,
    pub version: Option<String>,
}

static CACHE: LazyLock<Mutex<Option<(Instant, Listing)>>> = LazyLock::new(Default::default);

#[must_use]
pub fn parse_options(options: &Value) -> Option<Listing> {
    let model = options.as_array()?.iter().find(|o| o["id"] == "model")?;
    let models: Vec<Model> = model["options"]
        .as_array()?
        .iter()
        .filter_map(|o| {
            let id = o["value"].as_str().filter(|v| !v.is_empty())?;
            Some(Model {
                id: id.to_owned(),
                label: o["name"].as_str().unwrap_or(id).to_owned(),
                images: o["_meta"]["cognition.ai/supportsImages"].as_bool(),
            })
        })
        .collect();
    (!models.is_empty()).then(|| Listing {
        models,
        current: model["currentValue"].as_str().map(str::to_owned),
        version: None,
    })
}

pub fn remember(line: &str) {
    if !line.contains("\"configOptions\"") {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let fresh = v.pointer("/result/sessionId").is_some();
    let Some(found) = v
        .pointer("/result/configOptions")
        .or_else(|| v.pointer("/params/update/configOptions"))
        .and_then(parse_options)
    else {
        return;
    };
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let old = cache
        .as_ref()
        .map(|(_, old)| old.clone())
        .unwrap_or_default();
    *cache = Some((
        Instant::now(),
        Listing {
            models: found.models,
            current: if fresh { found.current } else { old.current },
            version: old.version,
        },
    ));
}

#[must_use]
pub fn rows(list: &Listing) -> Vec<Value> {
    let default = list
        .current
        .as_deref()
        .and_then(|c| list.models.iter().find(|m| m.id == c));
    let mut out = vec![json!({
        "id": "",
        "label": "Default",
        "desc": default.map_or_else(|| "Uses Devin's default model".to_owned(), |m| format!("Uses {}", m.label)),
        "efforts": [],
        "images": default.and_then(|m| m.images),
    })];
    out.extend(list.models.iter().map(
        |m| json!({ "id": m.id, "label": m.label, "desc": "", "efforts": [], "images": m.images }),
    ));
    out
}

fn request(id: &str, method: &str, params: &Value) -> String {
    format!(
        "{}\n",
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    )
}

async fn version(program: &str) -> Option<String> {
    let script = format!(
        "{}\n{} --version 2>/dev/null",
        blazar_transport::PATH_PRELUDE,
        blazar_transport::shell_quote(program)
    );
    let out = tokio::time::timeout(
        CLOSE_TIMEOUT,
        tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(script)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.'))
        .map(str::to_owned)
}

async fn probe(program: &str) -> Result<Listing, String> {
    let dir = std::env::temp_dir().join("blazar-devin-models");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Couldn't prepare a folder for Devin: {e}"))?;
    let script = format!(
        "{}\nexec {} acp",
        blazar_transport::PATH_PRELUDE,
        blazar_transport::shell_quote(program)
    );
    let mut child = tokio::process::Command::new("bash")
        .arg("-lc")
        .arg(script)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Devin isn't installed on this Mac".to_owned())?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("Couldn't talk to Devin".into());
    };
    let mut lines = BufReader::new(stdout).lines();
    let cwd = dir.display().to_string();
    let opening = request(
        blazar_runtime::acp::INIT_ID,
        "initialize",
        &json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            "clientInfo": { "name": "blazar", "title": "Blazar", "version": env!("CARGO_PKG_VERSION") },
        }),
    ) + &request(
        blazar_runtime::acp::NEW_ID,
        "session/new",
        &json!({ "cwd": cwd, "mcpServers": [] }),
    );
    stdin
        .write_all(opening.as_bytes())
        .await
        .map_err(|_| "Devin isn't installed on this Mac".to_owned())?;
    let wait = async {
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v["id"] == blazar_runtime::acp::NEW_ID && v.get("method").is_none() {
                return Ok(v);
            }
            if v["id"] == blazar_runtime::acp::INIT_ID && v.get("error").is_some() {
                return Err(v["error"]["message"]
                    .as_str()
                    .unwrap_or("Devin couldn't start")
                    .to_owned());
            }
        }
        Err("Devin isn't installed on this Mac".to_owned())
    };
    let opened = tokio::time::timeout(PROBE_TIMEOUT, wait)
        .await
        .map_err(|_| "Devin didn't answer in time".to_owned())??;
    if let Some(e) = opened.get("error") {
        return Err(e["data"]
            .as_str()
            .or_else(|| e["message"].as_str())
            .unwrap_or("Devin couldn't open a session")
            .to_owned());
    }
    let listing = parse_options(&opened["result"]["configOptions"]);
    if let Some(session) = opened["result"]["sessionId"].as_str() {
        let gone = request(
            "blazar-delete",
            "session/delete",
            &json!({ "sessionId": session }),
        );
        if stdin.write_all(gone.as_bytes()).await.is_ok() {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.contains("\"blazar-delete\"") {
                        break;
                    }
                }
            })
            .await;
        }
    }
    drop(stdin);
    if tokio::time::timeout(CLOSE_TIMEOUT, child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
    let mut listing = listing.ok_or_else(|| "Devin didn't list any models".to_owned())?;
    listing.version = version(program).await;
    Ok(listing)
}

fn reply(list: &Listing, stale: bool, error: Option<String>) -> Value {
    json!({
        "runtime": "devin", "source": "cli", "node": "local", "cli_version": list.version,
        "models": rows(list), "stale": stale, "error": error,
    })
}

pub async fn models(st: &Shared, run_node: &str, refresh: bool) -> Value {
    if run_node != "local" {
        return json!({
            "runtime": "devin", "source": "cli", "node": run_node, "models": [],
            "stale": false, "error": "Devin only runs on this Mac for now",
        });
    }
    let program = crate::api::local_program(st, "devin").await;
    let cached = CACHE.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Some((at, mut list)) = cached
        .clone()
        .filter(|(at, _)| !refresh && at.elapsed() < FRESH)
    {
        if list.version.is_none() {
            list.version = version(&program).await;
            *CACHE.lock().unwrap_or_else(PoisonError::into_inner) = Some((at, list.clone()));
        }
        return reply(&list, false, None);
    }
    match probe(&program).await {
        Ok(list) => {
            *CACHE.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((Instant::now(), list.clone()));
            reply(&list, false, None)
        }
        Err(error) => match cached {
            Some((_, list)) => reply(&list, true, Some(error)),
            None => json!({
                "runtime": "devin", "source": "cli", "node": "local", "models": [],
                "stale": false, "error": error,
            }),
        },
    }
}

#[cfg(test)]
#[path = "devin_models_tests.rs"]
mod tests;
