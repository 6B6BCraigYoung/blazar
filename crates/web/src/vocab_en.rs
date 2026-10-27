use blazar_core_types::failure::FailureCause;

use crate::vocab::{ErrorKind, error_kind, http_code};

#[must_use]
pub fn machine(node: &str) -> String {
    if node == "local" {
        "this machine".to_owned()
    } else {
        blazar_core_types::api::machine_label(node).to_owned()
    }
}

#[must_use]
pub fn state(raw: &str) -> &'static str {
    match raw {
        "running" | "in_progress" | "busy" => "Running",
        "starting" => "Starting",
        "awaiting_approval" | "awaiting" | "approval" => "Waiting for approval",
        "asking" => "Waiting for your answer",
        "lost" => "Lost",
        "in_review" => "In review",
        "failed" | "errored" | "error" => "Failed",
        "paused" => "Paused",
        "offline" => "Offline",
        "online" => "Online",
        "completed" | "done" => "Completed",
        "interrupted" => "Interrupted",
        "killed" => "Killed",
        "cancelled" => "Cancelled",
        "skipped" => "Skipped",
        "pending" => "Pending",
        "active" => "Active",
        "archived" => "Archived",
        _ => "Idle",
    }
}

#[must_use]
pub fn duration(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 if s.is_multiple_of(60) => format!("{}m", s / 60),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        3600..86_400 if s % 3600 / 60 == 0 => format!("{}h", s / 3600),
        3600..86_400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        _ if s % 86_400 / 3600 == 0 => format!("{}d", s / 86_400),
        _ => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

#[must_use]
pub fn error(raw: &str, status: Option<u16>) -> String {
    match error_kind(raw, status) {
        Some(ErrorKind::Machine) => {
            "Machine unreachable. Check that it is on and on the network".into()
        }
        Some(ErrorKind::Remote) => "The remote command failed".into(),
        Some(ErrorKind::Busy) => "Service overloaded. Try again shortly".into(),
        Some(ErrorKind::RateLimit) => "Rate limited. Try again shortly".into(),
        Some(ErrorKind::Login) => "Not logged in. Please log in again".into(),
        Some(ErrorKind::Forbidden) => "Permission denied".into(),
        Some(ErrorKind::Billing) => "Credit balance too low. Check the account".into(),
        Some(ErrorKind::Server) => "Server error. Try again shortly".into(),
        Some(ErrorKind::Timeout) => "Timed out. Try again shortly".into(),
        Some(ErrorKind::Offline) => "Can't reach the service. Check the network".into(),
        Some(ErrorKind::Missing) => "Not found. It may have been deleted".into(),
        None => raw.to_owned(),
    }
}

#[must_use]
pub fn cause(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return "Request failed".into();
    }
    let code = http_code(raw);
    let mut why = error(raw, None);
    if why == raw
        && let Some(c) = code
    {
        why = error("", Some(c));
    }
    let head: String = if why == raw || why.is_empty() {
        let line = raw
            .rsplit("stderr: ")
            .next()
            .unwrap_or(raw)
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let mut cut: String = line.chars().take(60).collect();
        if line.chars().count() > 60 {
            cut.push('…');
        }
        cut
    } else {
        why.split(". ").next().unwrap_or(&why).to_owned()
    };
    match code {
        Some(code) => format!("{head} ({code})"),
        None => head,
    }
}

#[must_use]
pub fn failure(
    cause: FailureCause,
    machine: &str,
    runtime: &str,
    resets: Option<&str>,
    message: &str,
) -> String {
    match cause {
        FailureCause::Network => "Network dropped before the turn finished".into(),
        FailureCause::Busy => "Service overloaded before the turn finished".into(),
        FailureCause::Node => format!("Can't reach {machine}"),
        FailureCause::Login => format!("{} is not logged in", crate::vocab::term(runtime)),
        FailureCause::Quota => match resets.filter(|r| !r.is_empty()) {
            Some(at) => format!("Usage limit reached · resets {at}"),
            None => "Usage limit reached".into(),
        },
        FailureCause::Resume | FailureCause::Other => self::cause(message),
    }
}

#[must_use]
pub const fn next_step(cause: FailureCause) -> &'static str {
    match cause {
        FailureCause::Network | FailureCause::Busy => "Continue",
        FailureCause::Node => "Reconnect",
        FailureCause::Login => "Log in…",
        FailureCause::Quota => "Switch account…",
        FailureCause::Resume | FailureCause::Other => "Retry",
    }
}

#[must_use]
pub fn retry_round(attempt: u64, max: u64) -> String {
    if max > 0 {
        format!("{attempt}/{max}")
    } else {
        format!("attempt {attempt}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_read_in_english() {
        assert_eq!(
            failure(FailureCause::Login, "local", "claude", None, "401"),
            "Claude Code is not logged in"
        );
        assert_eq!(
            failure(
                FailureCause::Quota,
                "local",
                "claude",
                Some("Sun 08:00"),
                "429"
            ),
            "Usage limit reached · resets Sun 08:00"
        );
        assert_eq!(
            cause("API Error: 529 overloaded"),
            "Service overloaded (529)"
        );
        assert_eq!(cause(""), "Request failed");
        assert_eq!(next_step(FailureCause::Quota), "Switch account…");
        assert_eq!(retry_round(3, 0), "attempt 3");
    }
}
