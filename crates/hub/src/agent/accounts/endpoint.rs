use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::model_choice::valid_model;

use super::{Account, fail};

pub const KEY_FILE: &str = ".blazar-endpoint-key";

pub(super) const KEY_ENV: &str = "ANTHROPIC_AUTH_TOKEN";

pub const CLEARED_ENV: [&str; 4] = [
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

const MAX_MODELS: usize = 24;

const MAIN_ALIASES: [&str; 3] = ["FABLE", "OPUS", "SONNET"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub base_url: String,
    pub models: Vec<String>,
    pub default_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

impl Endpoint {
    #[must_use]
    pub fn host(&self) -> String {
        reqwest::Url::parse(&self.base_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| self.base_url.clone())
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub models: &'static [&'static str],
    pub default_model: &'static str,
    pub small_model: &'static str,
    #[serde(skip)]
    pub models_url: &'static str,
}

pub const PRESETS: &[Preset] = &[Preset {
    id: "deepseek",
    label: "DeepSeek",
    base_url: "https://api.deepseek.com/anthropic",
    models: &["deepseek-v4-pro", "deepseek-flash"],
    default_model: "deepseek-v4-pro",
    small_model: "deepseek-flash",
    models_url: "https://api.deepseek.com/models",
}];

fn preset(id: Option<&str>) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| Some(p.id) == id)
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct EndpointInput {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
}

pub fn valid_base_url(raw: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| "接口地址要写完整，例如 https://api.deepseek.com/anthropic".to_owned())?;
    let host = url.host_str().unwrap_or_default();
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "[::1]");
    match url.scheme() {
        "https" if !host.is_empty() => {}
        "http" if loopback => {}
        _ => return Err("接口地址要用 https://，只有本机地址可以用 http://".into()),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("地址里不要带用户名和密码，密钥单独填".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("地址里不要带 ? 或 # 后面的部分".into());
    }
    let mut s = url.as_str().trim_end_matches('/').to_owned();
    for tail in ["/v1/messages", "/v1"] {
        if let Some(head) = s.strip_suffix(tail) {
            s = head.to_owned();
            break;
        }
    }
    Ok(s)
}

pub fn valid_key(raw: &str) -> Result<String, String> {
    let k = raw.trim();
    let k = k.strip_prefix("Bearer ").unwrap_or(k).trim();
    if k.starts_with("sk-ant-oat") {
        return Err("这是 Claude 的订阅 token，接入方式请选「粘贴长期 token」".into());
    }
    if k.len() < 8 || k.len() > 512 || !k.chars().all(|c| c.is_ascii_graphic()) {
        return Err("密钥不对：要 8–512 个字符，中间不能有空格".into());
    }
    Ok(k.to_owned())
}

fn target(input: &EndpointInput) -> Result<(String, Option<&'static Preset>), String> {
    let base_url = valid_base_url(&input.base_url)?;
    let wanted = input.preset.as_deref().filter(|p| !p.is_empty());
    let chosen = match wanted {
        Some(id) => Some(preset(Some(id)).ok_or_else(|| format!("未知的接口类型 {id}"))?),
        None => None,
    };
    Ok((
        base_url.clone(),
        chosen.filter(|p| p.base_url.trim_end_matches('/') == base_url),
    ))
}

pub fn valid_endpoint(input: &EndpointInput) -> Result<Endpoint, String> {
    let (base_url, preset) = target(input)?;
    let mut models: Vec<String> = Vec::new();
    for m in input.models.iter().map(|m| m.trim()) {
        if m.is_empty() || models.iter().any(|x| x == m) {
            continue;
        }
        if !valid_model(m) {
            return Err(format!("型号名「{m}」里有不支持的字符"));
        }
        models.push(m.to_owned());
    }
    if models.is_empty() {
        return Err("至少填一个型号".into());
    }
    if models.len() > MAX_MODELS {
        return Err(format!("型号最多 {MAX_MODELS} 个"));
    }
    let default_model = match input
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        Some(m) if models.iter().any(|x| x == m) => m.to_owned(),
        Some(m) => return Err(format!("默认型号「{m}」不在型号列表里")),
        None => models[0].clone(),
    };
    Ok(Endpoint {
        small_model: preset
            .map(|p| p.small_model.to_owned())
            .filter(|s| models.contains(s)),
        preset: preset.map(|p| p.id.to_owned()),
        base_url,
        models,
        default_model,
    })
}

#[must_use]
pub fn model_for(e: &Endpoint, requested: Option<&str>) -> String {
    requested
        .map(str::trim)
        .filter(|m| e.models.iter().any(|x| x == m))
        .map_or_else(|| e.default_model.clone(), str::to_owned)
}

pub async fn base_of(st: &crate::api::Shared, account: Option<&str>) -> Option<String> {
    let a = super::get(st, account?).await?;
    a.endpoint
        .filter(|_| a.kind == "endpoint")
        .map(|e| e.base_url)
}

pub async fn endpoint_only(st: &crate::api::Shared, model: &str) -> bool {
    let m = model.trim();
    let family = super::model_family(m);
    if m.is_empty()
        || m.starts_with("claude")
        || ["fable", "opus", "sonnet", "haiku", "default", "best"].contains(&family.as_str())
    {
        return false;
    }
    super::all(st, Some("claude"))
        .await
        .iter()
        .filter_map(|a| a.endpoint.as_ref().filter(|_| a.kind == "endpoint"))
        .any(|e| e.models.iter().any(|x| x == m))
}

#[must_use]
pub fn run_env(e: &Endpoint, model: &str) -> Vec<(String, String)> {
    let mut out = vec![
        ("ANTHROPIC_BASE_URL".to_owned(), e.base_url.clone()),
        ("API_TIMEOUT_MS".to_owned(), "600000".to_owned()),
        (
            "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_owned(),
            e.small_model.clone().unwrap_or_else(|| model.to_owned()),
        ),
    ];
    out.extend(
        MAIN_ALIASES
            .iter()
            .map(|f| (format!("ANTHROPIC_DEFAULT_{f}_MODEL"), model.to_owned())),
    );
    out
}

pub fn apply(
    env: &mut BTreeMap<String, String>,
    unset: &mut BTreeSet<String>,
    e: &Endpoint,
    model: &str,
) {
    for k in CLEARED_ENV.iter().chain(std::iter::once(&KEY_ENV)) {
        env.remove(*k);
    }
    unset.extend(CLEARED_ENV.iter().map(|k| (*k).to_owned()));
    env.extend(run_env(e, model));
}

#[must_use]
pub fn rows(e: &Endpoint) -> Vec<Value> {
    let mut out = vec![serde_json::json!({
        "id": "",
        "label": "Default",
        "desc": format!("Uses {}", e.default_model),
        "efforts": [],
    })];
    out.extend(
        e.models
            .iter()
            .map(|m| serde_json::json!({ "id": m, "label": m, "desc": "", "efforts": [] })),
    );
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Reach {
    pub status: String,
    pub models: Vec<String>,
    pub error: Option<String>,
}

impl Reach {
    fn unknown(why: impl Into<String>) -> Self {
        Self {
            status: "unknown".into(),
            models: Vec::new(),
            error: Some(why.into()),
        }
    }
}

#[must_use]
pub fn parse_models(body: &Value) -> Vec<String> {
    body["data"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|m| m["id"].as_str())
                .filter(|id| !id.is_empty() && valid_model(id))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[must_use]
pub fn judge(code: u16, body: &Value) -> Reach {
    match code {
        200..=299 => Reach {
            status: "ok".into(),
            models: parse_models(body),
            error: None,
        },
        401 | 403 => Reach {
            status: "logged_out".into(),
            models: Vec::new(),
            error: Some(format!("密钥无效（接口返回 {code}）")),
        },
        404 | 405 => Reach {
            status: "ok".into(),
            ..Reach::default()
        },
        _ => Reach::unknown(format!("接口返回 {code}")),
    }
}

async fn reach_with(base_url: &str, preset: Option<&Preset>, key: &str) -> Reach {
    let url = preset.map_or_else(
        || format!("{base_url}/v1/models"),
        |p| p.models_url.to_owned(),
    );
    let client = match crate::remote_cli::http() {
        Ok(c) => c,
        Err(e) => return Reach::unknown(e),
    };
    let sent = client
        .get(&url)
        .bearer_auth(key)
        .header("anthropic-version", "2023-06-01")
        .timeout(CHECK_TIMEOUT)
        .send()
        .await;
    match sent {
        Ok(r) => {
            let code = r.status().as_u16();
            let body = r
                .bytes()
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .unwrap_or_default();
            judge(code, &body)
        }
        Err(e) => Reach::unknown(format!(
            "连不上 {}：{}",
            reqwest::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default(),
            e.without_url()
        )),
    }
}

pub async fn reach(e: &Endpoint, key: &str) -> Reach {
    reach_with(&e.base_url, preset(e.preset.as_deref()), key).await
}

pub(super) async fn probe(a: &Account) -> Reach {
    let (Some(e), Some((_, path))) = (a.endpoint.as_ref(), a.secret_file()) else {
        return Reach::unknown("账号的接口配置不全");
    };
    match tokio::fs::read_to_string(&path).await {
        Ok(key) => reach(e, key.trim()).await,
        Err(_) => Reach {
            status: "logged_out".into(),
            models: Vec::new(),
            error: Some("没找到保存的密钥".into()),
        },
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct TestRequest {
    #[serde(default)]
    pub endpoint: EndpointInput,
    #[serde(default)]
    pub key: String,
}

pub async fn test(Json(req): Json<TestRequest>) -> Response {
    let (base_url, preset) = match target(&req.endpoint) {
        Ok(t) => t,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    let key = match valid_key(&req.key) {
        Ok(k) => k,
        Err(e) => return fail(StatusCode::BAD_REQUEST, e),
    };
    Json(reach_with(&base_url, preset, &key).await).into_response()
}

#[cfg(test)]
#[path = "endpoint_tests.rs"]
mod tests;
