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
}

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
}
