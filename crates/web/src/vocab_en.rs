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
    // 网页自己写的中文说法（请求根本没到 hub）：对话里说英文。
    if raw.starts_with("连不上 Blazar") || raw.starts_with("连不上服务") {
        return "Can't reach Blazar right now. Check the connection".into();
    }
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
        Some(ErrorKind::DiskFull) => "The machine's disk is full. Free some space and retry".into(),
        Some(ErrorKind::NoTool) => "A program this needs is missing on the machine".into(),
        None => raw.to_owned(),
    }
}

/// What an unrecognised error says in a conversation; [`lead`] keeps only its prefix then.
pub const VAGUE: &str = "Something went wrong";

/// The English twin of [`crate::vocab::explain`]: one plain sentence, plus the original text
/// to fold under “Details”.
#[must_use]
pub fn explain(raw: &str, status: Option<u16>) -> (String, Option<String>) {
    crate::vocab::explain_with(raw, status, error, VAGUE)
}

/// “Rename failed: Service overloaded. Try again shortly”; an unrecognised error keeps only
/// the prefix (“Rename failed”), its original stays under “Details”.
#[must_use]
pub fn lead(lead: &str, shown: (String, Option<String>)) -> (String, Option<String>) {
    crate::vocab::lead_with(lead, shown, VAGUE, ": ")
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

    #[test]
    fn unknown_errors_fold_their_original_text_in_english() {
        assert_eq!(
            explain("database is locked", Some(500)),
            (VAGUE.to_owned(), Some("database is locked".to_owned()))
        );
        assert_eq!(
            lead("Rename failed", explain("API Error: 529 overloaded", None)),
            (
                "Rename failed: Service overloaded. Try again shortly".to_owned(),
                Some("API Error: 529 overloaded".to_owned())
            )
        );
        assert_eq!(
            lead("Stop failed", explain("weird", None)),
            ("Stop failed".to_owned(), Some("weird".to_owned()))
        );
    }

    #[test]
    fn sends_that_never_reached_the_hub_read_in_english() {
        assert_eq!(
            error("连不上 Blazar 后台服务，请确认它在运行", None),
            "Can't reach Blazar right now. Check the connection"
        );
        assert_eq!(
            error("Failed to fetch", None),
            "Can't reach the service. Check the network"
        );
    }
}
