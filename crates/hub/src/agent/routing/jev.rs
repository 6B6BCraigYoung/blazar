use std::path::PathBuf;
use std::time::{Duration, Instant};

use reqwest::header::CONTENT_TYPE;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::api::Shared;

pub const URL: &str = "https://api.typesafe.ai/v1/systemone";

pub const MODEL: &str = "jev-latest";

pub const TIMEOUT: Duration = Duration::from_secs(3);

const MAX_CHARS: usize = 6000;

const PRICE_PER_TOKEN: f64 = 0.042 / 1_000_000.0;

const KEY_FILE: &str = "typesafe.key";

pub const LEVELS: [&str; 5] = [
    "Trivial: a quick answer, a lookup, or a one-line edit.",
    "Easy: a small, well-defined change in one or two files.",
    "Moderate: a feature or fix across several files that needs some investigation.",
    "Hard: a multi-step change across modules, tricky debugging, or design decisions.",
    "Very hard: a large refactor, a new subsystem, or open-ended investigation.",
];

pub const KINDS: [(&str, &str); 7] = [
    (
        "question",
        "Answer a question or explain something without changing code.",
    ),
    ("fix", "Fix a bug, an error, or a failing test."),
    (
        "feature",
        "Build new functionality or change existing behavior.",
    ),
    (
        "refactor",
        "Restructure or clean up code without changing behavior.",
    ),
    (
        "docs",
        "Write or edit documentation, comments, or other prose.",
    ),
    (
        "research",
        "Investigate, compare options, or look things up.",
    ),
    (
        "ops",
        "Run commands, builds, deployments, or system administration.",
    ),
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Verdict {
    pub difficulty: f64,
    pub difficulty_confidence: f64,
    pub kind: String,
    pub kind_confidence: f64,
    pub model: Option<String>,
    pub ms: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    NoKey,
    Rejected,
    Credits,
    Limited,
    Timeout,
    Network(String),
    Server(u16, Option<String>),
    Malformed,
}

impl Failure {
    #[must_use]
    pub fn en(&self) -> String {
        match self {
            Self::NoKey => "no key".into(),
            Self::Rejected => "key rejected".into(),
            Self::Credits => "out of credits".into(),
            Self::Limited => "rate limited".into(),
            Self::Timeout => "timed out".into(),
            Self::Network(_) => "unreachable".into(),
            Self::Server(code, _) => format!("HTTP {code}"),
            Self::Malformed => "unexpected reply".into(),
        }
    }

    #[must_use]
    pub fn zh(&self) -> String {
        match self {
            Self::NoKey => "还没填 TypeSafe 的 key".into(),
            Self::Rejected => "key 不对，或者已经失效".into(),
            Self::Credits => "TypeSafe 账户余额不足，充值后再试".into(),
            Self::Limited => "请求太频繁，被 TypeSafe 限流了，稍后再试".into(),
            Self::Timeout => format!("{} 秒内没有回应", TIMEOUT.as_secs()),
            Self::Network(e) => format!("连不上 TypeSafe：{e}"),
            Self::Server(code, Some(m)) => format!("TypeSafe 返回 HTTP {code}：{m}"),
            Self::Server(code, None) => format!("TypeSafe 返回 HTTP {code}"),
            Self::Malformed => "TypeSafe 的返回格式和预期不一样".into(),
        }
    }
}

#[must_use]
pub fn body(text: &str) -> Value {
    let state: String = text.trim().chars().take(MAX_CHARS).collect();
    let kinds: Map<String, Value> = KINDS
        .iter()
        .map(|(k, d)| ((*k).to_owned(), json!(d)))
        .collect();
    json!({
        "model": MODEL,
        "state": state,
        "questions": {
            "difficulty": {
                "type": "score",
                "instructions": "How hard is this request for an AI coding agent working in a software project?",
                "criteria": LEVELS,
            },
            "kind": {
                "type": "choice",
                "instructions": "What kind of work does this request ask for?",
                "criteria": kinds,
            },
        },
    })
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.trim().parse().ok())
        .filter(|n: &f64| n.is_finite())
}

#[must_use]
pub fn parse(v: &Value) -> Option<Verdict> {
    let d = &v["answers"]["difficulty"];
    let k = &v["answers"]["kind"];
    #[allow(clippy::cast_precision_loss)]
    let top = (LEVELS.len() - 1) as f64;
    let difficulty = num(&d["score"])?.clamp(0.0, top);
    let kind = k["choice"]
        .as_str()
        .filter(|c| KINDS.iter().any(|(n, _)| n == c))?
        .to_owned();
    #[allow(clippy::cast_precision_loss)]
    let estimate = v["usage"]["input_tokens"]
        .as_u64()
        .map(|t| t as f64 * PRICE_PER_TOKEN);
    Some(Verdict {
        difficulty,
        difficulty_confidence: num(&d["confidence"]).unwrap_or(1.0).clamp(0.0, 1.0),
        kind,
        kind_confidence: num(&k["confidence"]).unwrap_or(1.0).clamp(0.0, 1.0),
        model: v["model"].as_str().map(str::to_owned),
        ms: 0,
        cost_usd: num(&v["usage"]["cost"])
            .or_else(|| num(&v["meta"]["usd_spent"]))
            .or(estimate),
    })
}

#[must_use]
pub fn failure(code: u16, v: &Value) -> Failure {
    let kind = v["detail"]["error_type"]
        .as_str()
        .or_else(|| v["error"]["type"].as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let message = [
        &v["detail"]["message"],
        &v["detail"],
        &v["error"]["message"],
        &v["error"],
        &v["message"],
    ]
    .into_iter()
    .find_map(Value::as_str)
    .map(|m| m.chars().take(160).collect::<String>());
    if ["credit", "billing", "balance", "payment", "funds"]
        .iter()
        .any(|w| kind.contains(w))
    {
        return Failure::Credits;
    }
    if kind.contains("auth") || kind.contains("permission") {
        return Failure::Rejected;
    }
    if kind.contains("rate") {
        return Failure::Limited;
    }
    match code {
        401 | 403 => Failure::Rejected,
        402 => Failure::Credits,
        429 => Failure::Limited,
        _ => Failure::Server(code, message),
    }
}

pub async fn judge(key: &str, text: &str) -> Result<Verdict, Failure> {
    judge_at(URL, key, text).await
}

pub async fn judge_at(url: &str, key: &str, text: &str) -> Result<Verdict, Failure> {
    let client = reqwest::Client::builder()
        .connect_timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Failure::Network(e.to_string()))?;
    let started = Instant::now();
    let r = client
        .post(url)
        .bearer_auth(key)
        .header(CONTENT_TYPE, "application/json")
        .body(body(text).to_string())
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                Failure::Timeout
            } else {
                Failure::Network(e.without_url().to_string())
            }
        })?;
    let code = r.status().as_u16();
    let v = match r.bytes().await {
        Ok(b) => serde_json::from_slice::<Value>(&b).unwrap_or_default(),
        Err(e) if e.is_timeout() => return Err(Failure::Timeout),
        Err(_) => Value::Null,
    };
    if code != 200 {
        return Err(failure(code, &v));
    }
    let mut verdict = parse(&v).ok_or(Failure::Malformed)?;
    verdict.ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(verdict)
}

fn key_path(st: &Shared) -> PathBuf {
    super::dir(st).join(KEY_FILE)
}

pub async fn key(st: &Shared) -> Option<String> {
    tokio::fs::read_to_string(key_path(st))
        .await
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

pub fn save_key(st: &Shared, key: &str) -> std::io::Result<()> {
    let path = key_path(st);
    if let Some(dir) = path.parent() {
        blazar_db::ensure_private_dir(dir)?;
    }
    blazar_core_types::private_storage::write(&path, key.as_bytes())
}

pub fn delete_key(st: &Shared) -> std::io::Result<()> {
    match std::fs::remove_file(key_path(st)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "jev_tests.rs"]
mod tests;
