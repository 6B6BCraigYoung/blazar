use std::collections::BTreeMap;

use serde_json::{Value, json};

pub fn take_composer<T>(text: &mut String, images: &mut Vec<T>) -> (String, Vec<T>) {
    (std::mem::take(text), std::mem::take(images))
}

#[derive(Clone, Debug, Default)]
pub struct Deliveries {
    next: u32,
    sending: BTreeMap<u32, Value>,
    failed: BTreeMap<u32, Value>,
}

impl Deliveries {
    pub fn begin(&mut self, request: Value) -> u32 {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        self.sending.insert(id, request);
        id
    }

    pub fn finish(&mut self, id: u32, accepted: bool) {
        if let Some(request) = self.sending.remove(&id)
            && !accepted
        {
            self.failed.insert(id, request);
        }
    }

    pub fn failed(&self) -> Vec<(u32, Value)> {
        self.failed
            .iter()
            .map(|(id, body)| (*id, body.clone()))
            .collect()
    }

    pub fn take_failed(&mut self, id: u32) -> Option<Value> {
        self.failed.remove(&id)
    }

    pub fn edit_failed(&mut self, id: u32, request: Value) -> bool {
        let Some(failed) = self.failed.get_mut(&id) else {
            return false;
        };
        *failed = request;
        true
    }

    /// 页面重新载入后，把本地存着的发送失败的消息放回「Failed to send」那一栏。
    pub fn restore(&mut self, requests: Vec<Value>) {
        for request in requests {
            if request["text"].as_str().is_none() {
                continue;
            }
            let id = self.next;
            self.next = self.next.wrapping_add(1);
            self.failed.insert(id, request);
        }
    }

    /// 写进本地存储的那份：图片太大时只留文字（存储总共只有几 MB）。
    pub fn persistable(&self) -> Vec<Value> {
        let all: Vec<Value> = self.failed.values().cloned().collect();
        let size: usize = all.iter().map(|r| r.to_string().len()).sum();
        if size <= PERSIST_LIMIT {
            return all;
        }
        all.into_iter()
            .map(|mut r| {
                if r["images"].as_array().is_some_and(|a| !a.is_empty()) {
                    r["images"] = json!([]);
                }
                r
            })
            .filter(|r| r.to_string().len() <= PERSIST_LIMIT)
            .collect()
    }
}

/// 发送失败的消息在本地存储里的键，每个工作区一份。
pub fn unsent_key(ws: &str) -> String {
    format!("blazar.unsent.{ws}")
}

const PERSIST_LIMIT: usize = 512 * 1024;

pub fn edit_request(request: &Value, text: String) -> Value {
    let mut request = request.clone();
    request["text"] = json!(text);
    request
}

pub fn steer_request(request: &Value) -> Value {
    json!({ "text": request["text"], "images": request.get("images").cloned().unwrap_or_else(|| json!([])), "context_file": request["context_file"], "thinking": request["thinking"] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        json!({"text":"first", "images":[{"media_type":"image/png", "data":"YQ=="}], "context_file":"src/main.rs", "model":"demo-model", "thinking":false, "resume_session":"thread-a"})
    }

    #[test]
    fn sending_takes_only_its_own_draft_and_attachments() {
        let mut text = "first".to_owned();
        let mut images = vec!["first-image"];
        let taken = take_composer(&mut text, &mut images);
        assert_eq!(taken, ("first".into(), vec!["first-image"]));
        assert!(text.is_empty());
        assert!(images.is_empty());
        text.push_str("second");
        images.push("second-image");
        let mut deliveries = Deliveries::default();
        let first = deliveries.begin(request());
        deliveries.finish(first, true);
        assert_eq!(text, "second");
        assert_eq!(images, vec!["second-image"]);
        assert!(deliveries.failed().is_empty());
    }

    #[test]
    fn failed_and_out_of_order_sends_retain_each_complete_request() {
        let mut deliveries = Deliveries::default();
        let first = deliveries.begin(request());
        let second_body = edit_request(&request(), "second".into());
        let second = deliveries.begin(second_body.clone());
        deliveries.finish(second, false);
        deliveries.finish(first, false);
        assert_eq!(
            deliveries.failed(),
            vec![(first, request()), (second, second_body.clone())]
        );
        assert_eq!(deliveries.take_failed(second), Some(second_body));
        assert_eq!(deliveries.failed(), vec![(first, request())]);
    }

    #[test]
    fn queue_text_edits_preserve_images_context_and_original_options() {
        let mut expected = request();
        expected["text"] = json!("edited");
        assert_eq!(edit_request(&request(), "edited".into()), expected);
    }

    #[test]
    fn steer_includes_every_supported_message_field() {
        let input = steer_request(&request());
        assert_eq!(input["images"], request()["images"]);
        assert_eq!(input["context_file"], "src/main.rs");
        assert_eq!(input["thinking"], false);
        assert_eq!(input["text"], "first");
        assert!(input.get("model").is_none());
    }

    #[test]
    fn failed_messages_survive_a_reload_through_storage() {
        let mut before = Deliveries::default();
        let id = before.begin(request());
        before.finish(id, false);
        let ok = before.begin(edit_request(&request(), "sent".into()));
        before.finish(ok, true);
        let stored = serde_json::to_string(&before.persistable()).unwrap();

        let mut after = Deliveries::default();
        after.restore(serde_json::from_str(&stored).unwrap());
        after.restore(vec![json!({"no":"text"})]);
        let failed = after.failed();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].1, request());
        let fresh = after.begin(request());
        assert_ne!(
            fresh, failed[0].0,
            "restored ids never collide with new sends"
        );
        assert_eq!(unsent_key("w1"), "blazar.unsent.w1");
    }

    #[test]
    fn huge_attachments_are_dropped_before_storing() {
        let mut deliveries = Deliveries::default();
        let mut big = request();
        big["images"] = json!([{"media_type":"image/png", "data": "A".repeat(PERSIST_LIMIT)}]);
        let id = deliveries.begin(big);
        deliveries.finish(id, false);
        let kept = deliveries.persistable();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["text"], "first");
        assert_eq!(kept[0]["images"], json!([]));
    }

    #[test]
    fn failed_messages_can_be_edited_without_restoring_already_retried_requests() {
        let mut deliveries = Deliveries::default();
        let id = deliveries.begin(request());
        deliveries.finish(id, false);
        let mut edited = edit_request(&request(), "corrected".into());
        edited["images"] = json!([]);
        assert!(deliveries.edit_failed(id, edited.clone()));
        assert_eq!(deliveries.take_failed(id), Some(edited.clone()));
        assert_eq!(edited["context_file"], "src/main.rs");
        assert_eq!(edited["model"], "demo-model");
        assert!(!deliveries.edit_failed(id, request()));
        assert!(deliveries.failed().is_empty());
    }
}
