use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use blazar_transport::{ExecSpec, LocalTransport, NodeTransport};
use serde_json::{Value, json};

use crate::api::Shared;

const HELP_TIMEOUT: Duration = Duration::from_secs(30);

const RECENT: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
    pub default: bool,
}

type Listing = (String, Vec<Model>);

static CACHE: LazyLock<Mutex<Option<Listing>>> = LazyLock::new(Default::default);

fn efforts_of(spec: &str) -> Option<Vec<String>> {
    let list = spec.split("supported: [").nth(1)?.split(']').next()?;
    Some(
        list.split(',')
            .map(str::trim)
            .filter(|e| blazar_runtime::cli_runtime::EFFORTS.contains(e))
            .map(str::to_owned)
            .collect(),
    )
}

#[must_use]
pub fn parse_help(help: &str) -> Vec<Model> {
    let mut models: Vec<Model> = Vec::new();
    let mut efforts: HashMap<String, Vec<String>> = HashMap::new();
    let mut listing = false;
    for line in help.lines() {
        if line.starts_with("Available Models:") {
            listing = true;
            continue;
        }
        if listing {
            if line.starts_with("  ") && !line.trim().is_empty() {
                if let Some((id, label)) = line.trim().split_once(char::is_whitespace) {
                    let label = label.trim();
                    if !label.contains("[Deprecated]") {
                        let default = label.ends_with("(default)");
                        models.push(Model {
                            id: id.to_owned(),
                            label: label.trim_end_matches("(default)").trim().to_owned(),
                            efforts: Vec::new(),
                            default,
                        });
                    }
                }
                continue;
            }
            listing = false;
        }
        if let Some((label, spec)) = line
            .trim()
            .strip_prefix("- ")
            .and_then(|rest| rest.split_once(": supports reasoning:"))
            && let Some(list) = efforts_of(spec)
        {
            efforts.insert(label.trim().to_owned(), list);
        }
    }
    for m in &mut models {
        if let Some(e) = efforts.get(&m.label) {
            m.efforts.clone_from(e);
        }
    }
    models
}

#[must_use]
pub fn recent(home: &FsPath) -> Vec<String> {
    let mut found: Vec<(SystemTime, String)> = Vec::new();
    let Ok(dirs) = std::fs::read_dir(home.join(".factory").join("sessions")) else {
        return Vec::new();
    };
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".settings.json"))
            {
                continue;
            }
            let at = f
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if let Some(model) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["model"].as_str().map(str::to_owned))
                .filter(|m| !m.is_empty())
            {
                found.push((at, model));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<String> = Vec::new();
    for (_, m) in found {
        if !out.contains(&m) {
            out.push(m);
        }
        if out.len() == RECENT {
            break;
        }
    }
    out
}

fn custom_label(rest: &str) -> String {
    let name = match rest.rsplit_once('-') {
        Some((head, tail)) if !head.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => rest,
    };
    name.replace('-', " ")
}

fn provider_label(provider: &str) -> String {
    match provider {
        "chatgpt" => "ChatGPT".to_owned(),
        other => other.to_owned(),
    }
}

#[must_use]
pub fn rows(models: &[Model], recent: &[String]) -> Vec<Value> {
    let find = |id: &str| models.iter().find(|m| m.id == id);
    let default = models.iter().find(|m| m.default);
    let mut out = vec![json!({
        "id": "",
        "label": "Default",
        "desc": default.map_or_else(|| "Uses Droid's default model".to_owned(), |m| format!("Uses {}", m.label)),
        "efforts": default.map(|m| m.efforts.clone()).unwrap_or_default(),
    })];
    for id in recent {
        let (base, provider) = match id.strip_prefix("custom:").and_then(|r| r.split_once('/')) {
            Some((provider, base)) => (base, Some(provider)),
            None => (id.as_str(), None),
        };
        let known = find(base);
        let name = match (known, provider, id.strip_prefix("custom:")) {
            (Some(m), _, _) => m.label.clone(),
            (None, None, Some(rest)) => custom_label(rest),
            _ => base.to_owned(),
        };
        out.push(json!({
            "id": id,
            "label": provider.map_or(name.clone(), |p| format!("{name} · {}", provider_label(p))),
            "desc": "Used recently on this Mac",
            "efforts": known.map(|m| m.efforts.clone()).unwrap_or_default(),
        }));
    }
    out.extend(
        models
            .iter()
            .filter(|m| !recent.contains(&m.id))
            .map(|m| json!({ "id": m.id, "label": m.label, "desc": "", "efforts": m.efforts })),
    );
    out
}

async fn shell(program: &str, args: &str) -> Option<String> {
    let script = format!(
        "{}\n{} {args} 2>&1",
        blazar_transport::PATH_PRELUDE,
        blazar_transport::shell_quote(program)
    );
    let out = tokio::time::timeout(
        HELP_TIMEOUT,
        LocalTransport.exec(ExecSpec::new("bash").arg("-lc").arg(script)),
    )
    .await
    .ok()?
    .ok()?;
    (out.code == 0).then_some(out.stdout)
}

async fn listed(program: &str) -> Result<Listing, String> {
    let version = shell(program, "--version")
        .await
        .map(|v| v.lines().next().unwrap_or_default().trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or("Factory Droid isn't installed on this Mac")?;
    if let Some((v, models)) = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .filter(|(v, _)| *v == version)
    {
        return Ok((v, models));
    }
    let help = shell(program, "exec --help")
        .await
        .ok_or("Couldn't read Droid's model list")?;
    let models = parse_help(&help);
    *CACHE.lock().unwrap_or_else(PoisonError::into_inner) = Some((version.clone(), models.clone()));
    Ok((version, models))
}

pub async fn models(st: &Shared, run_node: &str) -> Value {
    if run_node != "local" {
        return json!({
            "runtime": "droid", "source": "cli", "node": run_node, "models": [],
            "stale": false, "error": "Factory Droid only runs on this Mac for now",
        });
    }
    let program = crate::api::local_program(st, "droid").await;
    match listed(&program).await {
        Ok((version, list)) => {
            let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
            let recent = home.as_deref().map(recent).unwrap_or_default();
            json!({
                "runtime": "droid", "source": "cli", "node": "local", "cli_version": version,
                "models": rows(&list, &recent), "stale": false, "error": null,
            })
        }
        Err(error) => json!({
            "runtime": "droid", "source": "cli", "node": "local", "models": [],
            "stale": false, "error": error,
        }),
    }
}

#[cfg(test)]
#[path = "droid_models_tests.rs"]
mod tests;
