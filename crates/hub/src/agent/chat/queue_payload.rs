use serde_json::Value;

pub fn editable_request(request: &Value) -> Option<Value> {
    let fields = [
        "text",
        "resume",
        "resume_session",
        "env",
        "disallowed_tools",
        "permission_mode",
        "agent",
        "wait_secs",
        "brain",
        "profile",
        "model",
        "effort",
        "images",
        "output_style",
        "fast_mode",
        "thinking",
        "context_file",
        "resume_at_last",
        "retry_thread",
        "account",
        "exclude_accounts",
    ];
    let object = request.as_object()?;
    if object.keys().any(|key| !fields.contains(&key.as_str()))
        || object
            .get("env")
            .is_some_and(|env| !env.is_null() && env.as_object().is_none_or(|env| !env.is_empty()))
    {
        return None;
    }
    Some(request.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn queued_edit_payload_retains_images_context_and_options() {
        let request = json!({"text":"hello", "images":[{"media_type":"image/png", "data":"YQ=="}], "context_file":"src/main.rs", "model":"demo-model", "resume_session":"thread-a"});
        assert_eq!(editable_request(&request), Some(request));
    }

    #[test]
    fn queued_edit_payload_never_exposes_environment_or_unknown_fields() {
        assert_eq!(
            editable_request(&json!({"text":"hello", "env":{"DEMO_TOKEN":"test-only"}})),
            None
        );
        assert_eq!(
            editable_request(&json!({"text":"hello", "future_secret":"test-only"})),
            None
        );
    }
}
