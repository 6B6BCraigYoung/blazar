pub mod jev;
pub mod options;
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

use crate::accounts::model_family;
use crate::api::Shared;
use crate::state::ServerEvent;

pub use jev::{Failure, Verdict};
pub use options::{Choice, Seat};
pub use rank::{Candidate, Skip, Tier, Why};

pub const AUTO: &str = "auto";

const KV_KEY: &str = "routing";

const RUNTIMES: [&str; 5] = ["claude", "codex", "devin", "droid", "grok"];

const DEFAULT_ON: [&str; 3] = ["claude", "codex", "devin"];

const ACCOUNT: &str = "acc:";

const OPENING_CHARS: usize = 1500;

const MESSAGE_CHARS: usize = 4000;

const REPLY_CHARS: usize = 1500;

const SAMPLE: &str =
    "Add a dark mode toggle to the settings page and remember the choice across restarts.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub enabled: bool,
    pub escalate: bool,
    pub channels: Vec<String>,
    pub first: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jev_url: Option<String>,
}

fn local_or_https(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://127.0.0.1:")
        || url.starts_with("http://localhost:")
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
    Settings {
        enabled: v["enabled"].as_bool().unwrap_or(true),
        escalate: v["escalate"].as_bool().unwrap_or(true),
        channels,
        first,
        jev_url: v["jev_url"]
            .as_str()
            .filter(|u| local_or_https(u))
            .map(str::to_owned),
    }
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
            images: !matches!(*rt, "droid" | "devin" | "grok"),
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

fn screen(ch: &Channel, ask: &Ask<'_>, found: &[DiscoveredAgent]) -> Option<Why> {
    if ask.remote && !blazar_runtime::supports_remote_hands(&ch.runtime) {
        return Some(Why::LocalOnly);
    }
    if let Some(a) = found.iter().find(|a| a.id == ch.runtime) {
        if a.path.is_none() && !crate::remote_cli::installable(&ch.runtime) {
            return Some(Why::Missing);
        }
        let own_login = ch.account.is_none() && crate::accounts::env_key(&ch.runtime).is_none();
        if own_login && a.authed == Some(false) {
            return Some(Why::LoggedOut);
        }
    }
    if ask.images && !ch.images {
        return Some(Why::NoImages);
    }
    None
}

struct Field {
    choices: Vec<Choice>,
    skipped: Vec<Skip>,
}

async fn field(
    st: &Shared,
    s: &Settings,
    all: &[Channel],
    ask: &Ask<'_>,
    found: &[DiscoveredAgent],
    line: f64,
) -> Field {
    let mut choices = Vec::new();
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
        let skip = |why| Skip {
            channel: ch.id.clone(),
            label: ch.label.clone(),
            why,
        };
        if let Some(why) = screen(ch, ask, found) {
            skipped.push(skip(why));
            continue;
        }
        let mut list = options::of_channel(st, ch, ask.remote, *id == s.first).await;
        if list.is_empty() {
            skipped.push(skip(Why::LoggedOut));
            continue;
        }
        for c in &mut list {
            c.seats.retain(|seat| seat.peak.is_none_or(|p| p < line));
        }
        list.retain(|c| !c.seats.is_empty());
        if list.is_empty() {
            skipped.push(skip(Why::OverLine));
            continue;
        }
        choices.extend(list);
    }
    number(&mut choices);
    Field { choices, skipped }
}

fn number(choices: &mut [Choice]) {
    for (i, c) in choices.iter_mut().enumerate() {
        c.key = format!("m{i}");
    }
}

fn seat_for<'a>(c: &'a Choice, line: f64, keep: Option<&str>) -> Option<&'a Seat> {
    if let Some(k) = keep
        && let Some(s) = c.seats.iter().find(|s| s.account.as_deref() == Some(k))
    {
        return Some(s);
    }
    let mut best: Option<&Seat> = None;
    for s in &c.seats {
        if best.is_none_or(|b| rank::load(s.peak, line) < rank::load(b.peak, line) - 1e-9) {
            best = Some(s);
        }
    }
    best
}

fn channel_view(choices: &[Choice], all: &[Channel], line: f64) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for c in choices {
        let peak = seat_for(c, line, None).and_then(|s| s.peak);
        match out.iter_mut().find(|x| x.channel == c.channel) {
            Some(x) => {
                if rank::load(peak, line) < rank::load(x.peak, line) {
                    x.peak = peak;
                }
            }
            None => out.push(Candidate {
                channel: c.channel.clone(),
                runtime: c.runtime.clone(),
                account: None,
                label: all
                    .iter()
                    .find(|ch| ch.id == c.channel)
                    .map_or_else(|| c.channel.clone(), |ch| ch.label.clone()),
                peak,
                strong: c.strong,
                metered: c.metered,
                first: c.first,
            }),
        }
    }
    out
}

fn plain(choices: &[Choice], channel: &Candidate) -> Option<Choice> {
    let mut c = choices
        .iter()
        .find(|c| c.channel == channel.channel)?
        .clone();
    let mut seats: Vec<Seat> = choices
        .iter()
        .filter(|x| x.channel == channel.channel)
        .flat_map(|x| x.seats.iter().cloned())
        .collect();
    seats.dedup_by(|a, b| a.account == b.account);
    c.seats = seats;
    c.model = None;
    c.effort = None;
    c.label = channel.label.clone();
    c.rank = 2;
    Some(c)
}

fn fallback_of(s: &Settings, all: &[Channel], ask: &Ask<'_>) -> Choice {
    let ch = all
        .iter()
        .find(|c| c.id == s.first)
        .filter(|c| !ask.remote || blazar_runtime::supports_remote_hands(&c.runtime));
    Choice {
        key: String::new(),
        channel: ch.map_or_else(|| "claude".to_owned(), |c| c.id.clone()),
        runtime: ch.map_or_else(|| "claude".to_owned(), |c| c.runtime.clone()),
        label: ch.map_or_else(|| label_of("claude"), |c| c.label.clone()),
        model: None,
        effort: None,
        about: String::new(),
        seats: ch
            .and_then(|c| c.account.clone())
            .map(|a| Seat {
                account: Some(a),
                label: String::new(),
                peak: None,
                pass: true,
                blocked: Vec::new(),
            })
            .into_iter()
            .collect(),
        metered: ch.is_some_and(|c| c.metered),
        strong: true,
        first: true,
        rank: 2,
    }
}

async fn judge(
    st: &Shared,
    s: &Settings,
    state: &str,
    choices: &[Choice],
    unhappy: bool,
) -> Option<Result<Verdict, Failure>> {
    if state.trim().is_empty() || (choices.len() < 2 && !unhappy) {
        return None;
    }
    let key = jev::key(st).await?;
    let pairs: Vec<(String, String)> = choices
        .iter()
        .map(|c| (c.key.clone(), c.about.clone()))
        .collect();
    Some(
        jev::judge_at(
            s.jev_url.as_deref().unwrap_or(jev::URL),
            &key,
            state,
            &pairs,
            unhappy,
        )
        .await,
    )
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Decision {
    pub channel: String,
    pub runtime: String,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub label: String,
    pub seat: String,
    pub tier: Option<Tier>,
    pub verdict: Option<Verdict>,
    pub jev_error: Option<String>,
    pub skipped: Vec<Skip>,
    pub kept: bool,
    pub note: String,
}

fn shown_seat(c: &Choice, seat: Option<&Seat>) -> Option<String> {
    let distinct = c
        .seats
        .iter()
        .filter_map(|s| s.account.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    seat.filter(|s| s.pass && distinct > 1 && !s.label.is_empty())
        .map(|s| s.label.clone())
}

pub async fn decide(st: &Shared, ask: Ask<'_>) -> Decision {
    let s = settings(st).await;
    let line = crate::accounts::selection::threshold(st).await;
    let found = discovered(st, ask.run_node).await;
    let all = channels(st).await;
    let f = field(st, &s, &all, &ask, &found, line).await;
    let judged = judge(st, &s, ask.text, &f.choices, false).await;
    let verdict = judged.as_ref().and_then(|r| r.as_ref().ok());
    let tier = rank::tier_of(verdict);
    let view = channel_view(&f.choices, &all, line);
    let picked = match verdict.filter(|v| !v.probs.is_empty()) {
        Some(v) => {
            let mut top: Option<(&Choice, f64)> = None;
            for c in &f.choices {
                let peak = seat_for(c, line, None).and_then(|x| x.peak);
                let w = rank::worth(v.prob(&c.key), peak, c.first, c.metered, line);
                if top.is_none_or(|(_, t)| w > t + 1e-9) {
                    top = Some((c, w));
                }
            }
            top.map(|(c, _)| c.clone())
        }
        None => rank::best(&view, tier, line).and_then(|ch| plain(&f.choices, ch)),
    };
    let fallback = picked.is_none();
    let chosen = picked.unwrap_or_else(|| fallback_of(&s, &all, &ask));
    let seat = seat_for(&chosen, line, None);
    let seat_label = shown_seat(&chosen, seat);
    let note = rank::note(&rank::Story {
        label: &chosen.label,
        channel: &chosen.channel,
        seat: seat_label.as_deref(),
        judged: judged.as_ref().map(Result::as_ref),
        first: view.iter().find(|c| c.first),
        first_skip: f
            .skipped
            .iter()
            .find(|k| k.channel == s.first && k.channel != chosen.channel),
        fallback,
    });
    Decision {
        account: seat.filter(|x| x.pass).and_then(|x| x.account.clone()),
        seat: seat.map(|x| x.label.clone()).unwrap_or_default(),
        channel: chosen.channel.clone(),
        runtime: chosen.runtime.clone(),
        model: chosen.model.clone(),
        effort: chosen.effort.clone(),
        label: chosen.label.clone(),
        tier: Some(tier),
        verdict: verdict.cloned(),
        jev_error: judged
            .as_ref()
            .and_then(|r| r.as_ref().err())
            .map(Failure::en),
        skipped: f.skipped,
        kept: false,
        note,
    }
}

#[derive(Debug, Clone, Default)]
pub struct Current {
    pub runtime: String,
    pub channel: String,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub actual: Option<String>,
    pub label: String,
    pub opening: String,
    pub last_reply: String,
}

pub async fn current(st: &Shared, thread: &str, runtime: &str, account: Option<&str>) -> Current {
    let (route, request): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT route, request FROM sessions WHERE COALESCE(thread_id, id) = ?1
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    let parse = |raw: Option<String>| {
        raw.and_then(|r| serde_json::from_str::<Value>(&r).ok())
            .unwrap_or(Value::Null)
    };
    let route = parse(route);
    let route = if route.is_null() {
        parse(request)
    } else {
        route
    };
    let route = match route["runtime"]
        .as_str()
        .or_else(|| route["agent"].as_str())
    {
        Some(r) if r != runtime => Value::Null,
        _ => route,
    };
    let actual: Option<String> = sqlx::query_scalar::<_, String>(
        "SELECT e.payload FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE COALESCE(s.thread_id, s.id) = ?1 AND e.payload LIKE '{\"type\":\"session_started\"%'
         ORDER BY s.created_at DESC, e.seq DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| v["model"].as_str().map(str::to_owned));
    let opening: String = sqlx::query_scalar::<_, String>(
        "SELECT payload FROM events WHERE session_id = ?1
           AND payload LIKE '{\"type\":\"user_message\"%' ORDER BY seq LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| v["text"].as_str().map(str::to_owned))
    .unwrap_or_default();
    let last_reply: String = sqlx::query_scalar::<_, String>(
        "SELECT e.payload FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE COALESCE(s.thread_id, s.id) = ?1 AND e.payload LIKE '{\"type\":\"assistant_message\"%'
         ORDER BY s.created_at DESC, e.seq DESC LIMIT 1",
    )
    .bind(thread)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .and_then(|p| serde_json::from_str::<Value>(&p).ok())
    .and_then(|v| v["text"].as_str().map(str::to_owned))
    .unwrap_or_default();
    let text = |k: &str| {
        route[k]
            .as_str()
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
    };
    let account = account.map(str::to_owned).or_else(|| text("account"));
    let endpoint = match account.as_deref() {
        Some(a) => crate::accounts::get(st, a)
            .await
            .filter(|x| x.kind == "endpoint")
            .map(|x| format!("{ACCOUNT}{}", x.id)),
        None => None,
    };
    Current {
        runtime: runtime.to_owned(),
        channel: endpoint.unwrap_or_else(|| runtime.to_owned()),
        account,
        model: text("model"),
        effort: text("effort"),
        actual,
        label: text("label").unwrap_or_else(|| label_of(runtime)),
        opening,
        last_reply,
    }
}

fn same(c: &Choice, cur: &Current, single: bool) -> bool {
    if single {
        return true;
    }
    if c.model == cur.model && c.effort == cur.effort && (c.model.is_some() || c.effort.is_some()) {
        return true;
    }
    c.runtime == "claude"
        && cur.model.is_none()
        && c.model.as_deref().map(model_family).is_some_and(|f| {
            cur.actual
                .as_deref()
                .map(model_family)
                .is_some_and(|a| a == f)
        })
}

fn follow_state(cur: &Current, text: &str) -> String {
    let opening: String = cur.opening.trim().chars().take(OPENING_CHARS).collect();
    let message: String = text.trim().chars().take(MESSAGE_CHARS).collect();
    let reply = cur.last_reply.trim();
    let skip = reply.chars().count().saturating_sub(REPLY_CHARS);
    let reply: String = reply.chars().skip(skip).collect();
    let earlier = if reply.is_empty() {
        String::new()
    } else {
        format!("The agent's last reply ended with: {reply}\n\n")
    };
    format!(
        "The conversation started with: {opening}\n\n{earlier}It is currently handled by: {}.\n\nThe new message is: {message}",
        cur.label
    )
}

#[must_use]
pub fn step_up(open: &[Choice], from: u8, verdict: Option<&Verdict>) -> Option<Choice> {
    let target = open
        .iter()
        .filter(|c| c.rank > from)
        .map(|c| c.rank)
        .min()?;
    let prob = |c: &Choice| verdict.map_or(0.0, |v| v.prob(&c.key));
    let mut best: Option<&Choice> = None;
    for c in open.iter().filter(|c| c.rank == target) {
        if best.is_none_or(|b| prob(c) > prob(b) + 1e-9) {
            best = Some(c);
        }
    }
    best.cloned()
}

#[must_use]
pub fn command_for(c: &Choice) -> String {
    match (c.model.as_deref(), c.effort.as_deref()) {
        (None, Some(e)) => format!("/model {} {e}", c.runtime),
        (Some(m), _) => format!("/model {m}"),
        (None, None) => format!("/model {}", c.runtime),
    }
}

async fn try_next(
    st: &Shared,
    s: &Settings,
    all: &[Channel],
    here: &str,
    ask: &Ask<'_>,
    line: f64,
) -> Option<String> {
    let found = discovered(st, ask.run_node).await;
    let order = std::iter::once(&s.first).chain(s.channels.iter().filter(|c| **c != s.first));
    for id in order {
        if id == here {
            continue;
        }
        let Some(ch) = all.iter().find(|c| &c.id == id) else {
            continue;
        };
        if screen(ch, ask, &found).is_some() {
            continue;
        }
        let mut list = options::of_channel(st, ch, ask.remote, false).await;
        for c in &mut list {
            c.seats.retain(|x| x.peak.is_none_or(|p| p < line));
        }
        let mut best: Option<&Choice> = None;
        for c in list.iter().filter(|c| !c.seats.is_empty()) {
            if best.is_none_or(|b| c.rank > b.rank) {
                best = Some(c);
            }
        }
        if let Some(b) = best {
            return Some(command_for(b));
        }
    }
    None
}

fn kept(cur: &Current, verdict: Option<Verdict>, jev_error: Option<String>) -> Decision {
    Decision {
        channel: cur.channel.clone(),
        runtime: cur.runtime.clone(),
        account: cur.account.clone(),
        model: cur.model.clone(),
        effort: cur.effort.clone(),
        label: cur.label.clone(),
        seat: String::new(),
        tier: None,
        verdict,
        jev_error,
        skipped: Vec::new(),
        kept: true,
        note: String::new(),
    }
}

pub async fn follow(st: &Shared, ask: Ask<'_>, cur: &Current) -> Decision {
    let s = settings(st).await;
    let line = crate::accounts::selection::threshold(st).await;
    let all = channels(st).await;
    let Some(ch) = all.iter().find(|c| c.id == cur.channel) else {
        return kept(cur, None, None);
    };
    let mut every = options::of_channel(st, ch, ask.remote, false).await;
    number(&mut every);
    let single = every.len() == 1;
    let here = every.iter().find(|c| same(c, cur, single)).cloned();
    let here_seat = every
        .iter()
        .flat_map(|c| c.seats.iter())
        .find(|x| x.account == cur.account && x.account.is_some())
        .cloned();
    let over = cur.account.is_some()
        && here_seat
            .as_ref()
            .is_none_or(|x| x.peak.is_some_and(|p| p >= line));
    let mut open = every.clone();
    for c in &mut open {
        c.seats.retain(|x| x.peak.is_none_or(|p| p < line));
    }
    open.retain(|c| !c.seats.is_empty());
    let judged = judge(st, &s, &follow_state(cur, ask.text), &open, s.escalate).await;
    let verdict = judged.as_ref().and_then(|r| r.as_ref().ok()).cloned();
    let jev_error = judged
        .as_ref()
        .and_then(|r| r.as_ref().err())
        .map(Failure::en);
    let unhappy = s.escalate
        && verdict
            .as_ref()
            .and_then(|v| v.unhappy)
            .is_some_and(|p| p >= rank::UNHAPPY);
    let here_rank = here.as_ref().map_or_else(
        || {
            options::strength(
                &cur.runtime,
                cur.model.as_deref().or(cur.actual.as_deref()),
                cur.effort.as_deref(),
                None,
            )
        },
        |h| h.rank,
    );
    let up = if unhappy {
        step_up(&open, here_rank, verdict.as_ref())
    } else {
        None
    };
    let hint = if unhappy && up.is_none() {
        let next = try_next(st, &s, &all, &ch.id, &ask, line).await;
        Some(rank::top_note(&ch.label, next.as_deref()))
    } else {
        None
    };
    let stay = |verdict: Option<Verdict>, jev_error: Option<String>| {
        let mut d = kept(cur, verdict, jev_error);
        if let Some(h) = &hint {
            d.note.clone_from(h);
        }
        d
    };
    let top = verdict
        .as_ref()
        .filter(|v| !v.probs.is_empty())
        .and_then(|v| {
            let mut best: Option<&Choice> = None;
            for c in &open {
                if best.is_none_or(|b| v.prob(&c.key) > v.prob(&b.key) + 1e-9) {
                    best = Some(c);
                }
            }
            best.cloned()
        });
    let here_open = here
        .as_ref()
        .and_then(|h| open.iter().find(|c| c.key == h.key).cloned());
    let switch_to = match (&up, &top, &verdict) {
        (Some(u), _, _) => Some(u.clone()),
        (None, Some(t), Some(v)) if !unhappy => {
            let current_p = here.as_ref().map_or(0.0, |h| v.prob(&h.key));
            (here.as_ref().is_none_or(|h| h.key != t.key)
                && rank::should_switch(v.prob(&t.key), current_p, v.pick_confidence))
            .then(|| t.clone())
        }
        _ => None,
    };
    let escalated = up.is_some();
    let next = match (switch_to, here_open) {
        (Some(t), _) => Some(t),
        (None, Some(h)) => Some(h),
        (None, None) if over => top.or_else(|| open.first().cloned()),
        (None, None) => None,
    };
    let Some(next) = next else {
        return stay(verdict, jev_error);
    };
    let keep = (!over).then_some(cur.account.as_deref()).flatten();
    let Some(seat) = seat_for(&next, line, keep) else {
        return stay(verdict, jev_error);
    };
    let model_changed = here.as_ref().is_none_or(|h| h.key != next.key);
    let account_changed = seat.pass && seat.account != cur.account;
    if !model_changed && !account_changed {
        return stay(verdict, jev_error);
    }
    let seat_label = (account_changed || shown_seat(&next, Some(seat)).is_some())
        .then(|| seat.label.clone())
        .filter(|l| !l.is_empty());
    let left = here_seat
        .filter(|_| over && account_changed)
        .and_then(|x| x.peak.map(|p| (x.label, p)));
    let note = rank::shift_note(&rank::Shift {
        label: &next.label,
        seat: seat_label.as_deref(),
        judged: judged.as_ref().map(Result::as_ref),
        left: left.as_ref().map(|(l, p)| (l.as_str(), *p)),
        model_changed,
        unhappy: escalated && model_changed,
    });
    Decision {
        channel: next.channel.clone(),
        runtime: next.runtime.clone(),
        account: seat
            .pass
            .then(|| seat.account.clone())
            .flatten()
            .or_else(|| cur.account.clone()),
        model: next.model.clone(),
        effort: next.effort.clone(),
        label: next.label.clone(),
        seat: seat.label.clone(),
        tier: verdict.as_ref().map(|v| rank::tier_of(Some(v))),
        verdict,
        jev_error,
        skipped: Vec::new(),
        kept: false,
        note,
    }
}

pub async fn quiet_runtime(st: &Shared) -> String {
    let s = settings(st).await;
    channels(st)
        .await
        .into_iter()
        .find(|c| c.id == s.first)
        .map_or_else(|| "claude".to_owned(), |c| c.runtime)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Resolved {
    pub agent: String,
    pub runtime: String,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub label: String,
}

fn normalize(token: &str) -> String {
    match token {
        "低" | "低推理" => "low".into(),
        "中" | "中等" => "medium".into(),
        "高" | "高推理" => "high".into(),
        t => t.to_lowercase(),
    }
}

#[must_use]
pub fn matching(choices: &[Choice], all: &[Channel], query: &str) -> Option<Resolved> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(normalize)
        .filter(|t| !t.is_empty() && t != "effort" && t != "推理")
        .collect();
    if tokens.is_empty() {
        return None;
    }
    if tokens.len() == 1 && matches!(tokens[0].as_str(), "auto" | "自动") {
        return Some(Resolved {
            agent: format!("r:{AUTO}"),
            runtime: AUTO.into(),
            account: None,
            model: None,
            effort: None,
            label: "Auto".into(),
        });
    }
    let names = |c: &Choice| {
        let ch = all.iter().find(|x| x.id == c.channel);
        format!(
            "{} {} {}",
            c.runtime,
            ch.map(|x| x.label.as_str()).unwrap_or_default(),
            c.channel
        )
        .to_lowercase()
    };
    let shape = |c: &Choice| {
        format!(
            "{} {} {}",
            c.label,
            c.model.as_deref().unwrap_or_default(),
            c.effort.as_deref().unwrap_or_default()
        )
        .to_lowercase()
    };
    let seat_hit = |c: &Choice, t: &str| {
        c.seats
            .iter()
            .find(|s| !s.label.is_empty() && s.label.to_lowercase().contains(t))
            .cloned()
    };
    let mut best: Option<(i32, &Choice, Option<Seat>, bool)> = None;
    for c in choices {
        let (n, sh) = (names(c), shape(c));
        let mut score = 0;
        let mut shaped = false;
        let mut seat = None;
        let mut all_hit = true;
        for t in &tokens {
            if sh.contains(t.as_str()) && !n.contains(t.as_str()) {
                score += 3;
                shaped = true;
            } else if n.contains(t.as_str()) || sh.contains(t.as_str()) {
                score += 2;
            } else if let Some(s) = seat_hit(c, t) {
                score += 1;
                seat = Some(s);
            } else {
                all_hit = false;
            }
        }
        if !all_hit {
            continue;
        }
        if best.as_ref().is_none_or(|(b, ..)| score > *b) {
            best = Some((score, c, seat, shaped));
        }
    }
    let (_, c, seat, shaped) = best?;
    let label = if shaped {
        c.label.clone()
    } else {
        all.iter()
            .find(|x| x.id == c.channel)
            .map_or_else(|| c.label.clone(), |x| x.label.clone())
    };
    Some(Resolved {
        agent: format!("r:{}", c.runtime),
        runtime: c.runtime.clone(),
        account: seat
            .filter(|s| s.pass)
            .and_then(|s| s.account)
            .or_else(|| c.channel.strip_prefix(ACCOUNT).map(str::to_owned)),
        model: c.model.clone().filter(|_| shaped),
        effort: c.effort.clone().filter(|_| shaped),
        label,
    })
}

pub async fn resolve(st: &Shared, query: &str) -> Option<Resolved> {
    let all = channels(st).await;
    let mut choices = Vec::new();
    for ch in &all {
        choices.extend(options::of_channel(st, ch, false, false).await);
    }
    number(&mut choices);
    matching(&choices, &all, query)
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
        "enabled": s.enabled,
        "escalate": s.escalate,
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
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub escalate: Option<bool>,
    #[serde(default)]
    pub channels: Option<Vec<String>>,
    #[serde(default)]
    pub first: Option<String>,
}

pub async fn put_settings(State(st): State<Shared>, Json(b): Json<SettingsBody>) -> Response {
    let cur = settings(&st).await;
    let explicit = b.channels.is_some();
    let list = match b.channels {
        Some(asked) => {
            let known = channels(&st).await;
            let mut list: Vec<String> = Vec::new();
            for c in asked {
                if !known.iter().any(|k| k.id == c) {
                    return fail(StatusCode::BAD_REQUEST, format!("没有这个渠道：{c}"));
                }
                if !list.contains(&c) {
                    list.push(c);
                }
            }
            list
        }
        None => cur.channels.clone(),
    };
    if list.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "至少要留一个渠道");
    }
    let first = b.first.unwrap_or_else(|| cur.first.clone());
    let first = if !explicit && !list.contains(&first) {
        list[0].clone()
    } else {
        first
    };
    if !list.contains(&first) {
        return fail(StatusCode::BAD_REQUEST, "首选渠道要在勾选的渠道里");
    }
    let mut saved = json!({
        "enabled": b.enabled.unwrap_or(cur.enabled),
        "escalate": b.escalate.unwrap_or(cur.escalate),
        "channels": list,
        "first": first,
    });
    if let Some(u) = cur.jev_url {
        saved["jev_url"] = json!(u);
    }
    match crate::office::kv_put(&st, KV_KEY, &saved).await {
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
    let s = settings(&st).await;
    let text = b
        .text
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| SAMPLE.to_owned());
    let url = s.jev_url.as_deref().unwrap_or(jev::URL);
    Json(match jev::judge_at(url, &key, &text, &[], false).await {
        Ok(v) => json!({
            "ok": true,
            "tier": rank::tier_of(Some(&v)),
            "task": rank::task_label(&v),
            "verdict": v,
        }),
        Err(f) => json!({ "ok": false, "error": f.zh() }),
    })
}

#[derive(Deserialize)]
pub struct ResolveBody {
    pub query: String,
}

pub async fn resolve_query(State(st): State<Shared>, Json(b): Json<ResolveBody>) -> Json<Value> {
    Json(match resolve(&st, &b.query).await {
        Some(r) => json!({ "ok": true, "pick": r }),
        None => json!({
            "ok": false,
            "error": format!("No model matches \"{}\"", b.query.trim()),
        }),
    })
}

#[cfg(test)]
#[path = "routing/routing_tests.rs"]
mod tests;
