use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendContext {
    pub workspace: String,
    pub thread: Option<String>,
    pub revision: u64,
}

impl SendContext {
    pub fn queue_request(&self, request: &Value) -> Value {
        json!({"thread_id": self.thread, "request": request, "append": true})
    }

    pub fn matches(&self, workspace: &str, thread: Option<&str>, revision: u64) -> bool {
        self.workspace == workspace && self.thread.as_deref() == thread && self.revision == revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(thread: Option<&str>) -> SendContext {
        SendContext {
            workspace: "workspace-a".into(),
            thread: thread.map(str::to_owned),
            revision: 3,
        }
    }

    #[test]
    fn queue_fallback_uses_the_original_thread_and_complete_request() {
        let captured = origin(Some("thread-a"));
        let request = json!({"text":"hello", "resume_session":"thread-a", "account":"account-a"});
        let queued = captured.queue_request(&request);
        assert_eq!(queued["thread_id"], "thread-a");
        assert_eq!(queued["request"], request);
        assert_eq!(queued["append"], true);
        assert!(!captured.matches("workspace-a", Some("thread-b"), 4));
    }

    #[test]
    fn delayed_responses_do_not_select_another_workspace_or_new_chat() {
        let captured = origin(None);
        assert!(captured.matches("workspace-a", None, 3));
        assert!(!captured.matches("workspace-b", None, 3));
        assert!(!captured.matches("workspace-a", None, 5));
        assert!(!captured.matches("workspace-a", Some("thread-a"), 3));
        assert_eq!(
            captured.queue_request(&json!({"text":"hello"}))["thread_id"],
            Value::Null
        );
    }
}
