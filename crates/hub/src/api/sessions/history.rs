use super::*;

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    pub after: u64,
}

pub async fn session_events(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Vec<NormalizedEntry>>> {
    let sid = SessionId(id.parse()?);
    Ok(Json(st.db.events_since(sid, q.after).await?))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
}

pub(crate) fn page_history(
    rows: Vec<serde_json::Value>,
    after: Option<&str>,
    limit: Option<usize>,
    summary: bool,
) -> Vec<serde_json::Value> {
    let start = after
        .filter(|a| !a.is_empty())
        .and_then(|a| rows.iter().position(|r| r["cursor"] == a))
        .map_or(0, |i| i + 1);
    let rows: Vec<serde_json::Value> = rows
        .into_iter()
        .skip(start)
        .filter(|r| {
            !summary
                || matches!(
                    r["kind"]["type"].as_str(),
                    Some(
                        "user_message" | "assistant_message" | "tool_use" | "approval" | "finished"
                    )
                )
        })
        .map(|mut r| {
            if summary && r["kind"]["type"] == "tool_use" {
                let input = r["kind"]["input"].to_string();
                r["kind"]["input"] =
                    serde_json::Value::String(input.chars().take(200).collect::<String>());
            }
            r
        })
        .collect();
    let limit = limit.unwrap_or(200).clamp(1, 5000);
    if after.is_some_and(|a| !a.is_empty()) || rows.len() <= limit {
        rows.into_iter().take(limit).collect()
    } else {
        rows[rows.len() - limit..].to_vec()
    }
}

struct StoredEvent {
    seq: i64,
    ts: String,
    session_id: String,
    parent: Option<String>,
    rewound: bool,
    payload: String,
}

impl StoredEvent {
    fn is(&self, kind: &str) -> bool {
        self.payload
            .strip_prefix("{\"type\":\"")
            .and_then(|rest| rest.strip_prefix(kind))
            .is_some_and(|rest| rest.starts_with('"'))
    }

    fn cursor(&self) -> String {
        format!("{}:{}", self.session_id, self.seq)
    }

    fn elided(&self) -> Option<String> {
        #[derive(Deserialize)]
        struct Head {
            id: serde_json::Value,
            ok: bool,
        }
        self.parent.as_ref()?;
        if self.is("thinking") {
            return Some(r#"{"type":"thinking","text":""}"#.to_owned());
        }
        if !self.is("tool_result") {
            return None;
        }
        let head: Head = serde_json::from_str(&self.payload).ok()?;
        Some(format!(
            r#"{{"type":"tool_result","id":{},"ok":{},"content":""}}"#,
            serde_json::to_string(&head.id).ok()?,
            head.ok
        ))
    }
}

#[derive(Serialize)]
struct HistoryRow {
    seq: i64,
    ts: String,
    session_id: String,
    cursor: String,
    parent: Option<String>,
    rewound: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    elided: bool,
    kind: Box<serde_json::value::RawValue>,
}

pub async fn workspace_history(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Response> {
    let one = q.session.filter(|s| !s.is_empty());
    let parent = q.parent.filter(|p| !p.is_empty());
    let events: Vec<StoredEvent> = sqlx::query(
        "SELECT e.seq, e.ts, e.payload, e.session_id, e.parent_tool_id, s.rewound_at
         FROM events e JOIN sessions s ON s.id = e.session_id
         WHERE s.workspace_id = ?1 AND (?2 IS NULL OR COALESCE(s.thread_id, s.id) = ?2)
           AND (?3 IS NULL OR e.parent_tool_id = ?3)
         ORDER BY s.created_at, e.seq",
    )
    .bind(&id)
    .bind(one)
    .bind(&parent)
    .fetch_all(st.db.pool())
    .await?
    .into_iter()
    .filter_map(|r| {
        Some(StoredEvent {
            seq: r.try_get::<i64, _>("seq").unwrap_or(0),
            ts: r.try_get::<String, _>("ts").unwrap_or_default(),
            session_id: r.try_get::<String, _>("session_id").unwrap_or_default(),
            parent: r
                .try_get::<Option<String>, _>("parent_tool_id")
                .ok()
                .flatten(),
            rewound: r
                .try_get::<Option<String>, _>("rewound_at")
                .ok()
                .flatten()
                .is_some(),
            payload: r.try_get("payload").ok()?,
        })
    })
    .collect();

    let paged = q.limit.is_some() || q.after.is_some() || q.mode.is_some();
    if paged {
        let all: Vec<serde_json::Value> = events
            .into_iter()
            .filter_map(|e| {
                let kind: serde_json::Value = serde_json::from_str(&e.payload).ok()?;
                Some(serde_json::json!({
                    "seq": e.seq,
                    "ts": e.ts,
                    "cursor": e.cursor(),
                    "session_id": e.session_id,
                    "parent": e.parent,
                    "rewound": e.rewound,
                    "kind": kind,
                }))
            })
            .collect();
        return Ok(Json(page_history(
            all,
            q.after.as_deref(),
            q.limit,
            q.mode.as_deref() == Some("summary"),
        ))
        .into_response());
    }

    let last_prompt = events
        .iter()
        .rposition(|e| e.parent.is_none() && e.is("user_message"));
    let rows: Vec<HistoryRow> = events
        .into_iter()
        .enumerate()
        .filter(|(i, e)| !(e.is("progress") && last_prompt.is_some_and(|p| *i < p)))
        .filter_map(|(_, e)| {
            let elided = parent.is_none().then(|| e.elided()).flatten();
            Some(HistoryRow {
                cursor: e.cursor(),
                elided: elided.is_some(),
                kind: serde_json::value::RawValue::from_string(elided.unwrap_or(e.payload)).ok()?,
                seq: e.seq,
                ts: e.ts,
                session_id: e.session_id,
                parent: e.parent,
                rewound: e.rewound,
            })
        })
        .collect();
    Ok(Json(rows).into_response())
}
