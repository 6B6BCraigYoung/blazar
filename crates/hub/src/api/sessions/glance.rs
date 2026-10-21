use super::*;

const GLANCE_CHARS: usize = 300;

fn glance_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(GLANCE_CHARS)
        .collect()
}

#[derive(Default)]
pub(crate) struct Glance {
    prompt: Option<(String, String)>,
    reply: Option<(String, String, bool)>,
}

impl Glance {
    pub(crate) fn take(&mut self, payload: &str, ts: &str) -> bool {
        if let Ok(kind) = serde_json::from_str::<serde_json::Value>(payload) {
            let failed = kind["status"] == "failed";
            let text = glance_text(
                kind[if failed { "message" } else { "text" }]
                    .as_str()
                    .unwrap_or_default(),
            );
            if !text.is_empty() {
                match kind["type"].as_str() {
                    Some("user_message") if self.prompt.is_none() => {
                        self.prompt = Some((text, ts.to_owned()));
                    }
                    Some("assistant_message" | "finished") if self.reply.is_none() => {
                        self.reply = Some((text, ts.to_owned(), failed));
                    }
                    _ => {}
                }
            }
        }
        self.prompt.is_some() && self.reply.is_some()
    }

    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "prompt": self.prompt.as_ref().map(|p| &p.0),
            "prompt_at": self.prompt.as_ref().map(|p| &p.1),
            "reply": self.reply.as_ref().map(|r| &r.0),
            "reply_at": self.reply.as_ref().map(|r| &r.1),
            "failed": self.reply.as_ref().is_some_and(|r| r.2),
        })
    }
}

pub async fn workspace_glance(
    State(st): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    use futures::TryStreamExt;
    let pool = st.db.pool();
    let sessions: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE workspace_id = ?1 AND rewound_at IS NULL
         ORDER BY created_at DESC LIMIT 20",
    )
    .bind(&id)
    .fetch_all(pool)
    .await?;
    let mut glance = Glance::default();
    'sessions: for session in sessions {
        let mut rows = sqlx::query_as::<_, (String, String)>(
            "SELECT payload, ts FROM events WHERE session_id = ?1 AND parent_tool_id IS NULL
               AND (payload LIKE '{\"type\":\"user_message\"%'
                 OR payload LIKE '{\"type\":\"assistant_message\"%'
                 OR payload LIKE '{\"type\":\"finished\"%')
             ORDER BY seq DESC",
        )
        .bind(&session)
        .fetch(pool);
        while let Some((payload, ts)) = rows.try_next().await? {
            if glance.take(&payload, &ts) {
                break 'sessions;
            }
        }
    }
    Ok(Json(glance.json()))
}

#[cfg(test)]
mod tests {
    use super::Glance;

    fn row(payload: &str, ts: &str) -> (String, String) {
        (payload.to_owned(), ts.to_owned())
    }

    fn glance(newest_first: impl IntoIterator<Item = (String, String)>) -> serde_json::Value {
        let mut g = Glance::default();
        for (payload, ts) in newest_first {
            if g.take(&payload, &ts) {
                break;
            }
        }
        g.json()
    }

    #[test]
    fn takes_the_newest_prompt_and_reply() {
        let g = glance([
            row(
                r#"{"type":"user_message","text":"run  the\n third pass"}"#,
                "t3",
            ),
            row(
                r#"{"type":"finished","status":"success","text":"loss 0.82"}"#,
                "t2",
            ),
            row(r#"{"type":"assistant_message","text":"loss 0.82"}"#, "t2"),
            row(
                r#"{"type":"user_message","text":"run the second pass"}"#,
                "t1",
            ),
        ]);
        assert_eq!(g["prompt"], "run the third pass");
        assert_eq!(g["prompt_at"], "t3");
        assert_eq!(g["reply"], "loss 0.82");
        assert_eq!(g["reply_at"], "t2");
        assert_eq!(g["failed"], false);
    }

    #[test]
    fn reports_a_failed_turn_and_skips_empty_text() {
        let g = glance([
            row(r#"{"type":"finished","status":"success"}"#, "t3"),
            row(
                r#"{"type":"finished","status":"failed","message":"node offline"}"#,
                "t2",
            ),
            row("not json", "t1"),
        ]);
        assert_eq!(g["reply"], "node offline");
        assert_eq!(g["failed"], true);
        assert!(g["prompt"].is_null());
    }

    #[test]
    fn caps_long_text() {
        let long = "字".repeat(400);
        let g = glance([row(
            &format!(r#"{{"type":"assistant_message","text":"{long}"}}"#),
            "t1",
        )]);
        assert_eq!(g["reply"].as_str().unwrap().chars().count(), 300);
    }
}
