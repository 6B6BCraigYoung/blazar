use serde::Serialize;
use serde_json::Value;

use super::Channel;
use crate::accounts::model_family;
use crate::api::Shared;

const EFFORTS: [&str; 3] = ["low", "medium", "high"];

const MAX_EXTRA_CODEX: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Seat {
    pub account: Option<String>,
    pub label: String,
    pub peak: Option<f64>,
    pub pass: bool,
    #[serde(skip)]
    pub blocked: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Choice {
    pub key: String,
    pub channel: String,
    pub runtime: String,
    pub label: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub about: String,
    pub seats: Vec<Seat>,
    pub metered: bool,
    pub strong: bool,
    pub first: bool,
    pub rank: u8,
}

#[must_use]
pub fn strength(
    runtime: &str,
    model: Option<&str>,
    effort: Option<&str>,
    small: Option<&str>,
) -> u8 {
    if model.is_some() && model == small {
        return 1;
    }
    let family = model.map(model_family);
    match (runtime, family.as_deref(), effort) {
        ("claude", Some("haiku"), _) | ("codex", None, Some("low")) => 1,
        ("claude", Some("opus"), _) | ("codex", None, Some("high")) => 3,
        ("claude", Some("fable"), _) | ("codex", None, Some("xhigh")) => 4,
        _ => 2,
    }
}

#[must_use]
pub fn claude_about(id: &str, label: &str, desc: &str) -> String {
    match id {
        "fable" => "Claude Code with Fable, the most capable Claude model, for the hardest and longest tasks; slowest, and uses the most subscription quota.".into(),
        "opus" => "Claude Code with Opus, a strong model for everyday and complex coding work.".into(),
        "sonnet" => "Claude Code with Sonnet, an efficient model for routine coding tasks; faster than Opus.".into(),
        "haiku" => "Claude Code with Haiku, the fastest model, for quick answers and trivial edits.".into(),
        _ => format!("Claude Code with {label}: {desc}"),
    }
}

#[must_use]
pub fn effort_about(effort: &str) -> &'static str {
    match effort {
        "low" => {
            "OpenAI Codex with low reasoning effort: quick, for simple and well-defined changes."
        }
        "high" => {
            "OpenAI Codex with high reasoning effort: deep reasoning for hard debugging and complex changes; slower."
        }
        _ => "OpenAI Codex with medium reasoning effort: balanced, for typical coding tasks.",
    }
}

fn rows(v: &Value) -> Vec<(String, String, String, Vec<String>)> {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            (
                m["id"].as_str().unwrap_or_default().to_owned(),
                m["label"].as_str().unwrap_or_default().to_owned(),
                m["desc"].as_str().unwrap_or_default().to_owned(),
                m["efforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect(),
            )
        })
        .collect()
}

fn seat_of(s: crate::accounts::SeatInfo, pass: bool) -> Seat {
    Seat {
        account: Some(s.id),
        label: s.label,
        peak: s.peak,
        pass,
        blocked: s.blocked,
    }
}

async fn seats_of(st: &Shared, ch: &Channel, remote: bool) -> Vec<Seat> {
    if let Some(id) = &ch.account {
        return crate::accounts::get(st, id)
            .await
            .filter(crate::accounts::Account::usable)
            .map(|a| Seat {
                account: Some(a.id),
                label: a.label,
                peak: None,
                pass: true,
                blocked: Vec::new(),
            })
            .into_iter()
            .collect();
    }
    match ch.runtime.as_str() {
        "claude" => crate::accounts::seats(st, "claude", remote)
            .await
            .into_iter()
            .map(|s| seat_of(s, true))
            .collect(),
        "codex" => crate::accounts::seats(st, "codex", false)
            .await
            .into_iter()
            .map(|s| seat_of(s, true))
            .collect(),
        "devin" | "droid" => crate::accounts::seats(st, &ch.runtime, false)
            .await
            .into_iter()
            .map(|s| seat_of(s, false))
            .collect(),
        _ => vec![Seat {
            account: None,
            label: String::new(),
            peak: None,
            pass: false,
            blocked: Vec::new(),
        }],
    }
}

fn shapes(
    endpoint: Option<&crate::accounts::Endpoint>,
    ch: &Channel,
) -> Vec<(String, Option<String>, Option<String>, String)> {
    if let Some(e) = endpoint {
        return e
            .models
            .iter()
            .map(|m| {
                let cheap = e.small_model.as_deref() == Some(m.as_str());
                let about = if cheap {
                    format!(
                        "{} {m} through a pay-per-token API: cheap and fast, for simple tasks.",
                        ch.label
                    )
                } else {
                    format!(
                        "{} {m} through a pay-per-token API: a capable general model.",
                        ch.label
                    )
                };
                (format!("{} · {m}", ch.label), Some(m.clone()), None, about)
            })
            .collect();
    }
    match ch.runtime.as_str() {
        "claude" => rows(&crate::api::fixed_models("claude"))
            .into_iter()
            .filter(|(id, ..)| !id.is_empty())
            .map(|(id, label, desc, _)| {
                (
                    format!("{} · {label}", ch.label),
                    Some(id.clone()),
                    None,
                    claude_about(&id, &label, &desc),
                )
            })
            .collect(),
        "codex" => {
            let list = rows(&crate::api::fixed_models("codex"));
            let mut out = Vec::new();
            if let Some((_, _, _, efforts)) = list.iter().find(|(id, ..)| id.is_empty()) {
                for e in EFFORTS.iter().filter(|e| efforts.iter().any(|x| x == *e)) {
                    out.push((
                        format!("{} · {e} effort", ch.label),
                        None,
                        Some((*e).to_owned()),
                        effort_about(e).to_owned(),
                    ));
                }
            }
            for (id, label, desc, _) in list
                .into_iter()
                .filter(|(id, ..)| !id.is_empty())
                .take(MAX_EXTRA_CODEX)
            {
                let about = if desc.is_empty() {
                    format!("OpenAI Codex with {label}.")
                } else {
                    format!("OpenAI Codex with {label}: {desc}")
                };
                out.push((format!("{} · {label}", ch.label), Some(id), None, about));
            }
            out
        }
        "devin" => vec![(
            ch.label.clone(),
            None,
            None,
            "Devin, an autonomous coding agent that picks its own model; good for well-scoped, self-contained tasks.".into(),
        )],
        "droid" => vec![(
            ch.label.clone(),
            None,
            None,
            "Factory Droid, a coding agent that runs on your own model key.".into(),
        )],
        _ => vec![(
            ch.label.clone(),
            None,
            None,
            format!("{}, a coding agent.", ch.label),
        )],
    }
}

pub async fn of_channel(st: &Shared, ch: &Channel, remote: bool, first: bool) -> Vec<Choice> {
    let seats = seats_of(st, ch, remote).await;
    if seats.is_empty() {
        return Vec::new();
    }
    let endpoint = match &ch.account {
        Some(id) => crate::accounts::get(st, id).await.and_then(|a| a.endpoint),
        None => None,
    };
    let small = endpoint.as_ref().and_then(|e| e.small_model.clone());
    shapes(endpoint.as_ref(), ch)
        .into_iter()
        .map(|(label, model, effort, about)| {
            let family = model.as_deref().map(model_family).unwrap_or_default();
            let rank = strength(
                &ch.runtime,
                model.as_deref(),
                effort.as_deref(),
                small.as_deref(),
            );
            Choice {
                key: String::new(),
                channel: ch.id.clone(),
                runtime: ch.runtime.clone(),
                label,
                seats: seats
                    .iter()
                    .filter(|s| family.is_empty() || !s.blocked.contains(&family))
                    .cloned()
                    .collect(),
                model,
                effort,
                about,
                metered: ch.metered,
                strong: !ch.metered && ch.runtime != "droid",
                first,
                rank,
            }
        })
        .filter(|c| !c.seats.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
