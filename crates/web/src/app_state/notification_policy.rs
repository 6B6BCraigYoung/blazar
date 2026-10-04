pub fn workspace_alert_kind<'a>(
    activity: &str,
    approval_tools: impl IntoIterator<Item = &'a str>,
    muted: &[String],
) -> Option<&'static str> {
    let enabled = |kind: &&str| !muted.iter().any(|muted| muted == kind);
    match activity {
        "completed" => Some("run_done").filter(enabled),
        "errored" => Some("run_failed").filter(enabled),
        "awaiting_approval" => approval_tools
            .into_iter()
            .map(|tool| {
                if tool.rsplit("__").next() == Some("AskUserQuestion") {
                    "question"
                } else {
                    "approval"
                }
            })
            .find(enabled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muted_completion_and_failure_never_trigger_workspace_alerts() {
        for (activity, kind) in [("completed", "run_done"), ("errored", "run_failed")] {
            assert_eq!(workspace_alert_kind(activity, [], &[kind.into()]), None);
            assert_eq!(workspace_alert_kind(activity, [], &[]), Some(kind));
        }
    }

    #[test]
    fn question_and_approval_mutes_are_independent() {
        assert_eq!(
            workspace_alert_kind("awaiting_approval", ["Bash"], &["approval".into()]),
            None
        );
        assert_eq!(
            workspace_alert_kind(
                "awaiting_approval",
                ["mcp__blazar__AskUserQuestion"],
                &["question".into()]
            ),
            None
        );
        assert_eq!(
            workspace_alert_kind(
                "awaiting_approval",
                ["AskUserQuestion", "Bash"],
                &["question".into()]
            ),
            Some("approval")
        );
        assert_eq!(
            workspace_alert_kind(
                "awaiting_approval",
                ["Bash", "AskUserQuestion"],
                &["approval".into()]
            ),
            Some("question")
        );
        assert_eq!(
            workspace_alert_kind(
                "awaiting_approval",
                ["Bash", "AskUserQuestion"],
                &["approval".into(), "question".into()]
            ),
            None
        );
    }

    #[test]
    fn settled_or_unrecognized_workspaces_do_not_trigger_alerts() {
        assert_eq!(workspace_alert_kind("awaiting_approval", [], &[]), None);
        assert_eq!(workspace_alert_kind("idle", [], &[]), None);
        assert_eq!(workspace_alert_kind("running", [], &[]), None);
    }
}
