pub(super) fn unchanged(
    known_mtime: u64,
    known_version: Option<&str>,
    current_mtime: u64,
    current_version: Option<&str>,
) -> bool {
    match (known_version, current_version) {
        (Some(known), Some(current)) => known == current,
        _ => known_mtime == current_mtime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_content_reloads_even_when_the_timestamp_is_unchanged() {
        assert!(!unchanged(12, Some("first"), 12, Some("second")));
    }

    #[test]
    fn identical_content_does_not_reload_after_a_timestamp_only_change() {
        assert!(unchanged(12, Some("same"), 13, Some("same")));
    }

    #[test]
    fn legacy_servers_keep_timestamp_based_reload_behavior() {
        assert!(unchanged(12, None, 12, None));
        assert!(!unchanged(12, None, 13, None));
        assert!(!unchanged(12, None, 13, Some("new")));
    }
}
