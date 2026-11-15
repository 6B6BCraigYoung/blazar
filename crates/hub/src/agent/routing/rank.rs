use serde::{Deserialize, Serialize};

use super::jev::{Failure, Verdict};

pub const MIN_CONFIDENCE: f64 = 0.4;

const FIRST_BONUS: f64 = 0.3;

pub const SOFT_LINE: f64 = 0.5;

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

#[must_use]
pub fn pressure(peak: f64, line: f64) -> f64 {
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

const QUOTA_WEIGHT: f64 = 0.3;

const FIRST_NUDGE: f64 = 0.05;

const METERED_NUDGE: f64 = 0.05;

pub const SWITCH_GAP: f64 = 0.3;

pub const SWITCH_CONFIDENCE: f64 = 0.6;

pub const UNHAPPY: f64 = 0.7;

#[must_use]
pub fn load(peak: Option<f64>, line: f64) -> f64 {
    let p = peak.unwrap_or(SOFT_LINE);
    pressure(p, line) + ROOM * p
}

#[must_use]
pub fn worth(prob: f64, peak: Option<f64>, first: bool, metered: bool, line: f64) -> f64 {
    let mut w = prob - QUOTA_WEIGHT * pressure(peak.unwrap_or(SOFT_LINE), line);
    if first {
        w += FIRST_NUDGE;
    }
    if metered {
        w -= METERED_NUDGE;
    }
    w
}

#[must_use]
pub fn should_switch(top: f64, current: f64, confidence: f64) -> bool {
    top - current >= SWITCH_GAP && confidence >= SWITCH_CONFIDENCE
}

fn limit_label(label: &str, peak: f64) -> String {
    format!("{label} at {:.0}% of its limit", (peak * 100.0).floor())
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
    pub label: &'a str,
    pub channel: &'a str,
    pub seat: Option<&'a str>,
    pub judged: Option<Result<&'a Verdict, &'a Failure>>,
    pub first: Option<&'a Candidate>,
    pub first_skip: Option<&'a Skip>,
    pub fallback: bool,
}

fn judged_part(judged: Option<Result<&Verdict, &Failure>>) -> Option<String> {
    match judged {
        Some(Ok(v)) => Some(task_label(v)),
        Some(Err(f)) => Some(format!("Jev {}", f.en())),
        None => None,
    }
}

#[must_use]
pub fn note(s: &Story<'_>) -> String {
    let mut parts = vec![format!("Auto → {}", s.label)];
    parts.extend(s.seat.filter(|x| !x.is_empty()).map(str::to_owned));
    parts.extend(judged_part(s.judged));
    if s.fallback {
        parts.push("fallback: no channel is available".into());
    } else if let Some(skip) = s.first_skip {
        parts.push(format!("{} {}", skip.label, skip.why.en()));
    } else if let Some(f) = s.first.filter(|f| f.channel != s.channel)
        && let Some(peak) = f.peak.filter(|p| *p >= SOFT_LINE)
    {
        parts.push(limit_label(&f.label, peak));
    }
    parts.join(" · ")
}

pub struct Shift<'a> {
    pub label: &'a str,
    pub seat: Option<&'a str>,
    pub judged: Option<Result<&'a Verdict, &'a Failure>>,
    pub left: Option<(&'a str, f64)>,
    pub model_changed: bool,
    pub unhappy: bool,
}

#[must_use]
pub fn shift_note(s: &Shift<'_>) -> String {
    let mut parts = vec![format!(
        "Auto → {}",
        if s.model_changed {
            s.label
        } else {
            s.seat.unwrap_or(s.label)
        }
    )];
    if s.model_changed {
        parts.extend(s.seat.filter(|x| !x.is_empty()).map(str::to_owned));
        if s.unhappy {
            parts.push("you seemed unhappy with the last result".into());
        } else {
            parts.extend(judged_part(s.judged));
        }
    }
    if let Some((old, peak)) = s.left {
        parts.push(limit_label(old, peak));
    }
    parts.join(" · ")
}

#[must_use]
pub fn top_note(label: &str, try_next: Option<&str>) -> String {
    match try_next {
        Some(cmd) => format!("Already on the strongest {label} setup · try {cmd}"),
        None => format!("Already on the strongest {label} setup"),
    }
}

#[cfg(test)]
#[path = "rank_tests.rs"]
mod tests;
