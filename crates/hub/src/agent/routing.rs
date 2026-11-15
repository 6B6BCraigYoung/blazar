pub mod jev;
pub mod rank;

use std::path::{Path as FsPath, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use blazar_core_types::{EntryKind, NormalizedEntry, SessionId, WorkspaceId};
use blazar_runtime::DiscoveredAgent;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::Shared;
use crate::state::ServerEvent;

pub use jev::{Failure, Verdict};
pub use rank::{Candidate, Skip, Tier, Why};

pub const AUTO: &str = "auto";

const KV_KEY: &str = "routing";

const RUNTIMES: [&str; 4] = ["claude", "codex", "devin", "droid"];

const DEFAULT_ON: [&str; 3] = ["claude", "codex", "devin"];

const ACCOUNT: &str = "acc:";

const SAMPLE: &str =
    "Add a dark mode toggle to the settings page and remember the choice across restarts.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub channels: Vec<String>,
    pub first: String,
}

#[must_use]
pub fn settings_of(v: &Value) -> Settings {
    let mut channels: Vec<String> = Vec::new();
    for c in v["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !channels.iter().any(|x| x == c) {
            channels.push(c.to_owned());
        }
    }
    if channels.is_empty() {
        channels = DEFAULT_ON.iter().map(|c| (*c).to_owned()).collect();
    }
    let first = v["first"]
        .as_str()
        .filter(|f| channels.iter().any(|c| c == f))
        .map_or_else(|| channels[0].clone(), str::to_owned);
    Settings { channels, first }
}

pub async fn settings(st: &Shared) -> Settings {
    settings_of(&crate::office::kv_get(st, KV_KEY).await)
}

fn dir(st: &Shared) -> PathBuf {
    st.mesh_ctx
        .staging_dir
        .parent()
        .map_or_else(std::env::temp_dir, FsPath::to_path_buf)
        .join("routing")
}

#[derive(Debug, Clone, Serialize)]
pub struct Channel {
    pub id: String,
    pub label: String,
    pub runtime: String,
    pub account: Option<String>,
    pub metered: bool,
    pub images: bool,
    pub installed: Option<bool>,
}

async fn discovered(st: &Shared, node: &str) -> Vec<DiscoveredAgent> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT json_extract(capabilities, '$.agents') FROM nodes WHERE name = ?1",
    )
    .bind(node)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .flatten()
    .and_then(|c| serde_json::from_str(&c).ok())
    .unwrap_or_default()
}

fn label_of(runtime: &str) -> String {
    blazar_runtime::spec::find(runtime).map_or_else(|| runtime.to_owned(), |s| s.label.to_owned())
}

pub async fn channels(st: &Shared) -> Vec<Channel> {
    let found = discovered(st, "local").await;
    let installed = |rt: &str| found.iter().find(|a| a.id == rt).map(|a| a.path.is_some());
    let mut out: Vec<Channel> = RUNTIMES
        .iter()
        .map(|rt| Channel {
            id: (*rt).to_owned(),
            label: label_of(rt),
            runtime: (*rt).to_owned(),
            account: None,
            metered: false,
            images: !matches!(*rt, "droid" | "devin"),
            installed: installed(rt),
        })
        .collect();
    for a in crate::accounts::endpoints(st).await {
        out.push(Channel {
            id: format!("{ACCOUNT}{}", a.id),
            label: a.label.clone(),
            runtime: "claude".into(),
            images: a.endpoint.as_ref().is_none_or(|e| e.images),
            account: Some(a.id),
            metered: true,
            installed: installed("claude"),
        });
    }
    out
}

pub struct Ask<'a> {
    pub text: &'a str,
    pub images: bool,
    pub remote: bool,
    pub run_node: &'a str,
}

async fn assess(
    st: &Shared,
    ch: &Channel,
    ask: &Ask<'_>,
    found: &[DiscoveredAgent],
    line: f64,
    first: bool,
) -> Result<Candidate, Skip> {
    let skip = |why| Skip {
        channel: ch.id.clone(),
        label: ch.label.clone(),
        why,
    };
    if ask.remote && !blazar_runtime::supports_remote_hands(&ch.runtime) {
        return Err(skip(Why::LocalOnly));
    }
    if let Some(a) = found.iter().find(|a| a.id == ch.runtime) {
        if a.path.is_none() && !crate::remote_cli::installable(&ch.runtime) {
            return Err(skip(Why::Missing));
        }
        if a.authed == Some(false) && crate::accounts::QUOTA_ONLY.contains(&ch.runtime.as_str()) {
            return Err(skip(Why::LoggedOut));
        }
    }
    if ask.images && !ch.images {
        return Err(skip(Why::NoImages));
    }
    let peak = match &ch.account {
        Some(id) => {
            if !crate::accounts::get(st, id)
                .await
                .is_some_and(|a| a.usable())
            {
                return Err(skip(Why::LoggedOut));
            }
            None
        }
        None => {
            crate::accounts::outlook(st, &ch.runtime, ask.remote)
                .await
                .ok_or_else(|| skip(Why::LoggedOut))?
                .1
        }
    };
    if peak.is_some_and(|p| p >= line) {
        return Err(skip(Why::OverLine));
    }
    Ok(Candidate {
        channel: ch.id.clone(),
        runtime: ch.runtime.clone(),
        account: ch.account.clone(),
        label: ch.label.clone(),
        peak,
        strong: !ch.metered && ch.runtime != "droid",
        metered: ch.metered,
        first,
    })
}

fn fallback_of(s: &Settings, all: &[Channel], ask: &Ask<'_>) -> Candidate {
    let ch = all
        .iter()
        .find(|c| c.id == s.first)
        .filter(|c| !ask.remote || blazar_runtime::supports_remote_hands(&c.runtime));
    Candidate {
        channel: ch.map_or_else(|| "claude".to_owned(), |c| c.id.clone()),
        runtime: ch.map_or_else(|| "claude".to_owned(), |c| c.runtime.clone()),
        account: ch.and_then(|c| c.account.clone()),
        label: ch.map_or_else(|| label_of("claude"), |c| c.label.clone()),
        peak: None,
        strong: true,
        metered: ch.is_some_and(|c| c.metered),
        first: true,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub channel: String,
    pub runtime: String,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub tier: Tier,
    pub verdict: Option<Verdict>,
    pub jev_error: Option<String>,
    pub candidates: Vec<Candidate>,
    pub skipped: Vec<Skip>,
    pub note: String,
}

pub async fn decide(st: &Shared, ask: Ask<'_>) -> Decision {
    let s = settings(st).await;
    let line = crate::accounts::selection::threshold(st).await;
    let found = discovered(st, ask.run_node).await;
    let all = channels(st).await;
    let mut cands = Vec::new();
    let mut skipped = Vec::new();
    for id in &s.channels {
        let Some(ch) = all.iter().find(|c| &c.id == id) else {
            skipped.push(Skip {
                channel: id.clone(),
                label: id.clone(),
                why: Why::Gone,
            });
            continue;
        };
        match assess(st, ch, &ask, &found, line, *id == s.first).await {
            Ok(c) => cands.push(c),
            Err(k) => skipped.push(k),
        }
    }
    let worth = cands.len() > 1
        || cands
            .iter()
            .any(|c| matches!(c.channel.as_str(), "claude" | "codex"));
    let judged = match jev::key(st).await {
        Some(key) if worth && !ask.text.trim().is_empty() => Some(jev::judge(&key, ask.text).await),
        _ => None,
    };
    let verdict = judged.as_ref().and_then(|r| r.as_ref().ok());
    let tier = rank::tier_of(verdict);
    let picked = rank::best(&cands, tier, line).cloned();
    let fallback = picked.is_none();
    let chosen = picked.unwrap_or_else(|| fallback_of(&s, &all, &ask));
    let (model, effort) = rank::tuning(&chosen.channel, tier);
    let note = rank::note(&rank::Story {
        chosen: &chosen,
        model,
        effort,
        judged: judged.as_ref().map(Result::as_ref),
        first: cands.iter().find(|c| c.first),
        first_skip: skipped
            .iter()
            .find(|k| k.channel == s.first && k.channel != chosen.channel),
        fallback,
    });
    Decision {
        channel: chosen.channel.clone(),
        runtime: chosen.runtime.clone(),
        account: chosen.account.clone(),
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
        tier,
        verdict: verdict.cloned(),
        jev_error: judged
            .as_ref()
            .and_then(|r| r.as_ref().err())
            .map(Failure::en),
        candidates: cands,
        skipped,
        note,
    }
}

pub async fn announce(
    st: &Shared,
    sid: SessionId,
    ws: WorkspaceId,
    seq: u64,
    message: &str,
) -> anyhow::Result<()> {
    let entry = NormalizedEntry {
        seq,
        ts: Utc::now(),
        parent_tool_use_id: None,
        kind: EntryKind::Notice {
            message: message.to_owned(),
        },
    };
    st.db.append_event(sid, ws, &entry).await?;
    st.emit(ServerEvent::Entry {
        workspace_id: ws,
        session_id: sid,
        entry: Box::new(entry),
    });
    Ok(())
}

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

pub async fn get_settings(State(st): State<Shared>) -> Json<Value> {
    let s = settings(&st).await;
    Json(json!({
        "channels": s.channels,
        "first": s.first,
        "options": channels(&st).await,
        "jev": {
            "key": jev::key(&st).await.is_some(),
            "model": jev::MODEL,
            "timeout_secs": jev::TIMEOUT.as_secs(),
        },
    }))
}

#[derive(Deserialize)]
pub struct SettingsBody {
    pub channels: Vec<String>,
    pub first: String,
}

pub async fn put_settings(State(st): State<Shared>, Json(b): Json<SettingsBody>) -> Response {
    let known = channels(&st).await;
    let mut list: Vec<String> = Vec::new();
    for c in b.channels {
        if !known.iter().any(|k| k.id == c) {
            return fail(StatusCode::BAD_REQUEST, format!("没有这个渠道：{c}"));
        }
        if !list.contains(&c) {
            list.push(c);
        }
    }
    if list.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "至少要留一个渠道");
    }
    if !list.contains(&b.first) {
        return fail(StatusCode::BAD_REQUEST, "首选渠道要在勾选的渠道里");
    }
    match crate::office::kv_put(&st, KV_KEY, &json!({ "channels": list, "first": b.first })).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
pub struct KeyBody {
    pub key: String,
}

pub async fn put_key(State(st): State<Shared>, Json(b): Json<KeyBody>) -> Response {
    let key = b.key.trim();
    let key = key.strip_prefix("Bearer ").unwrap_or(key).trim();
    if key.len() < 8 || key.len() > 512 || !key.chars().all(|c| c.is_ascii_graphic()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "key 不对：要 8–512 个字符，中间不能有空格",
        );
    }
    match jev::save_key(&st, key) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("key 没存上：{e}"),
        ),
    }
}

pub async fn delete_key(State(st): State<Shared>) -> Response {
    match jev::delete_key(&st) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("key 没删掉：{e}"),
        ),
    }
}

#[derive(Deserialize, Default)]
pub struct TestBody {
    #[serde(default)]
    pub text: Option<String>,
}

pub async fn test(State(st): State<Shared>, Json(b): Json<TestBody>) -> Json<Value> {
    let Some(key) = jev::key(&st).await else {
        return Json(json!({ "ok": false, "error": Failure::NoKey.zh() }));
    };
    let text = b
        .text
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| SAMPLE.to_owned());
    Json(match jev::judge(&key, &text).await {
        Ok(v) => json!({
            "ok": true,
            "tier": rank::tier_of(Some(&v)),
            "task": rank::task_label(&v),
            "verdict": v,
        }),
        Err(f) => json!({ "ok": false, "error": f.zh() }),
    })
}
