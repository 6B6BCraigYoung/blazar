use serde_json::Value;

fn s(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}

/// Where in the conversation the card was asked: `at=<session>:<seq>`, plus the tool call when known.
/// Cards stored before positions were kept have none and open the conversation at its end.
fn anchor_query(it: &Value) -> String {
    let sid = s(it, "session_id");
    let Some(seq) = it["anchor_seq"].as_i64().filter(|_| !sid.is_empty()) else {
        return String::new();
    };
    let enc = crate::md::encode_component;
    let mut q = format!("at={}", enc(&format!("{sid}:{seq}")));
    let tool = s(it, "anchor_tool");
    if !tool.is_empty() {
        q.push_str(&format!("&tool={}", enc(&tool)));
    }
    q
}

pub fn with_anchor(base: String, it: &Value) -> String {
    let at = anchor_query(it);
    if at.is_empty() || s(it, "thread_id").is_empty() || s(it, "workspace_id").is_empty() {
        return base;
    }
    let join = if base.contains('?') { '&' } else { '?' };
    format!("{base}{join}{at}")
}

/// The `at` parameter of a conversation link: the session and the event to open at.
pub fn parse_at(raw: &str) -> Option<(String, u64)> {
    let (sid, seq) = raw.rsplit_once(':')?;
    Some((sid.to_owned(), seq.parse().ok()?)).filter(|(sid, _)| !sid.is_empty())
}

fn key_pos(key: &str) -> Option<(&str, u64)> {
    let mut parts = key.split(':');
    let sid = parts.next()?;
    let seq = parts.next()?.parse().ok()?;
    Some((sid, seq))
}

/// The transcript item a card points at: the last one of its session at or before the event,
/// else the session's first item. Keys read `<session>:<seq>`, with an optional suffix.
pub fn anchor_index<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    sid: &str,
    seq: u64,
) -> Option<usize> {
    let mut first = None;
    let mut best = None;
    for (i, key) in keys.into_iter().enumerate() {
        let Some((s, q)) = key_pos(key) else { continue };
        if s != sid {
            continue;
        }
        first.get_or_insert(i);
        if q <= seq {
            best = Some(i);
        }
    }
    best.or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(extra: Value) -> Value {
        let mut v = json!({
            "id": "c1", "kind": "question", "workspace_id": "w1", "thread_id": "t1",
            "session_id": "s1", "ref_id": "ask:s1",
        });
        if let (Some(v), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            v.extend(extra.clone());
        }
        v
    }

    #[test]
    fn a_card_links_to_the_message_that_asked_it() {
        let it = card(json!({ "anchor_seq": 12, "anchor_tool": "toolu_1" }));
        assert_eq!(
            with_anchor("/w/w1?thread=t1".into(), &it),
            "/w/w1?thread=t1&at=s1%3A12&tool=toolu_1"
        );
        assert_eq!(
            with_anchor("/chats/t1".into(), &it),
            "/chats/t1?at=s1%3A12&tool=toolu_1"
        );
        let tail = card(json!({ "anchor_seq": 30 }));
        assert_eq!(
            with_anchor("/w/w1?thread=t1".into(), &tail),
            "/w/w1?thread=t1&at=s1%3A30"
        );
    }

    #[test]
    fn an_old_card_without_a_position_opens_the_conversation_as_before() {
        assert_eq!(
            with_anchor("/w/w1?thread=t1".into(), &card(json!({}))),
            "/w/w1?thread=t1"
        );
        assert_eq!(
            with_anchor("/chats/t1".into(), &card(json!({}))),
            "/chats/t1"
        );
        let no_session = card(json!({ "session_id": null, "anchor_seq": 3 }));
        assert_eq!(
            with_anchor("/w/w1?thread=t1".into(), &no_session),
            "/w/w1?thread=t1"
        );
        let no_thread = card(json!({ "thread_id": null, "anchor_seq": 3 }));
        assert_eq!(with_anchor("/runtimes".into(), &no_thread), "/runtimes");
    }

    #[test]
    fn the_at_parameter_reads_back() {
        assert_eq!(parse_at("s1:12"), Some(("s1".into(), 12)));
        assert_eq!(parse_at("a:b:3"), Some(("a:b".into(), 3)));
        assert_eq!(parse_at("s1"), None);
        assert_eq!(parse_at(":3"), None);
        assert_eq!(parse_at("s1:x"), None);
    }

    #[test]
    fn a_position_finds_its_item_in_the_transcript() {
        let keys = ["s0:3", "s1:1", "s1:5", "s1:9:text", "rw-s1:12", "s2:1"];
        assert_eq!(anchor_index(keys, "s1", 6), Some(2));
        assert_eq!(anchor_index(keys, "s1", 9), Some(3));
        assert_eq!(anchor_index(keys, "s1", 0), Some(1));
        assert_eq!(anchor_index(keys, "s9", 4), None);
    }
}
