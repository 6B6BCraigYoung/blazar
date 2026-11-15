use serde::{Deserialize, Serialize};

use super::jev::{Failure, Verdict};

pub const MIN_CONFIDENCE: f64 = 0.4;

const FIRST_BONUS: f64 = 0.3;

const SOFT_LINE: f64 = 0.5;

const PRESSURE: f64 = 0.6;

const WEAK_PENALTY: f64 = 0.4;

const METERED_PENALTY: f64 = 0.15;

const ROOM: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Light,
    Medium,
    Heavy,
}

#[must_use]
pub fn tier_of(v: Option<&Verdict>) -> Tier {
    let Some(v) = v.filter(|v| v.difficulty_confidence >= MIN_CONFIDENCE) else {
        return Tier::Medium;
    };
    let base = if v.difficulty < 1.5 {
        Tier::Light
    } else if v.difficulty < 2.5 {
        Tier::Medium
    } else {
        Tier::Heavy
    };
    if v.kind_confidence < MIN_CONFIDENCE {
        return base;
    }
    match (v.kind.as_str(), base) {
        ("question" | "docs" | "research", Tier::Heavy) | ("refactor", Tier::Light) => Tier::Medium,
        _ => base,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub channel: String,
    pub runtime: String,
    pub account: Option<String>,
    pub label: String,
    pub peak: Option<f64>,
    pub strong: bool,
    pub metered: bool,
    pub first: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    Missing,
    LoggedOut,
    LocalOnly,
    NoImages,
    OverLine,
    Gone,
}

impl Why {
    #[must_use]
    pub fn en(self) -> &'static str {
        match self {
            Self::Missing => "isn't installed",
            Self::LoggedOut => "isn't logged in",
            Self::LocalOnly => "only runs in local workspaces",
            Self::NoImages => "can't take images",
            Self::OverLine => "is over its usage line",
            Self::Gone => "was removed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skip {
    pub channel: String,
    pub label: String,
    pub why: Why,
}

fn pressure(peak: f64, line: f64) -> f64 {
    if line <= SOFT_LINE {
        return if peak >= line { 1.0 } else { 0.0 };
    }
    ((peak - SOFT_LINE) / (line - SOFT_LINE)).clamp(0.0, 1.0)
}

#[must_use]
pub fn merit(c: &Candidate, tier: Tier, line: f64) -> f64 {
    let peak = c.peak.unwrap_or(SOFT_LINE);
    let mut m = ROOM * (1.0 - peak) - PRESSURE * pressure(peak, line);
    if c.first {
        m += FIRST_BONUS;
    }
    if tier == Tier::Heavy && !c.strong {
        m -= WEAK_PENALTY;
    }
    if tier != Tier::Light && c.metered {
        m -= METERED_PENALTY;
    }
    m
}

#[must_use]
pub fn best(cands: &[Candidate], tier: Tier, line: f64) -> Option<&Candidate> {
    let mut top: Option<(&Candidate, f64)> = None;
    for c in cands {
        let m = merit(c, tier, line);
        if top.is_none_or(|(_, t)| m > t + 1e-9) {
            top = Some((c, m));
        }
    }
    top.map(|(c, _)| c)
}

#[must_use]
pub fn tuning(channel: &str, tier: Tier) -> (Option<&'static str>, Option<&'static str>) {
    match (channel, tier) {
        ("claude", Tier::Light) => (Some("sonnet"), None),
        ("codex", Tier::Light) => (None, Some("low")),
        ("codex", Tier::Heavy) => (None, Some("high")),
        _ => (None, None),
    }
}

fn tuning_label(model: Option<&str>, effort: Option<&str>) -> Option<String> {
    match (model, effort) {
        (Some("sonnet"), _) => Some("Sonnet".into()),
        (Some(m), _) => Some(m.to_owned()),
        (None, Some(e)) => Some(format!("{e} effort")),
        (None, None) => None,
    }
}

#[must_use]
pub fn task_label(v: &Verdict) -> String {
    let level = match v.difficulty {
        d if d < 0.5 => "trivial",
        d if d < 1.5 => "easy",
        d if d < 2.5 => "moderate",
        d if d < 3.5 => "hard",
        _ => "very hard",
    };
    let kind = match v.kind.as_str() {
        "fix" => "bug fix",
        "ops" => "ops task",
        k => k,
    };
    format!("{level} {kind}")
}

pub struct Story<'a> {
    pub chosen: &'a Candidate,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub judged: Option<Result<&'a Verdict, &'a Failure>>,
    pub first: Option<&'a Candidate>,
    pub first_skip: Option<&'a Skip>,
    pub fallback: bool,
}

#[must_use]
pub fn note(s: &Story<'_>) -> String {
    let mut parts = vec![format!("Auto → {}", s.chosen.label)];
    parts.extend(tuning_label(s.model, s.effort));
    match s.judged {
        Some(Ok(v)) => parts.push(task_label(v)),
        Some(Err(f)) => parts.push(format!("Jev {}", f.en())),
        None => {}
    }
    if s.fallback {
        parts.push("fallback: no channel is available".into());
    } else if let Some(skip) = s.first_skip {
        parts.push(format!("{} {}", skip.label, skip.why.en()));
    } else if let Some(f) = s.first.filter(|f| f.channel != s.chosen.channel)
        && let Some(peak) = f.peak.filter(|p| *p >= SOFT_LINE)
    {
        parts.push(format!(
            "{} at {:.0}% of its limit",
            f.label,
            (peak * 100.0).floor()
        ));
    }
    parts.join(" · ")
}

#[cfg(test)]
#[path = "rank_tests.rs"]
mod tests;
