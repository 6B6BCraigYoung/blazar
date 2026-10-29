use super::*;

pub use blazar_core_types::api::{NodeView, StateSnapshot, WorkspaceStep, WorkspaceView};

pub async fn get_state(State(st): State<Shared>) -> ApiResult<Json<StateSnapshot>> {
    Ok(Json(snapshot(&st).await?))
}

pub async fn list_workspaces(State(st): State<Shared>) -> ApiResult<Json<Vec<WorkspaceView>>> {
    Ok(Json(snapshot(&st).await?.workspaces))
}

pub async fn list_nodes(State(st): State<Shared>) -> ApiResult<Json<Vec<NodeView>>> {
    Ok(Json(snapshot(&st).await?.nodes))
}

const STEP_TEXT_MAX: usize = 160;
const STEP_INPUT_MAX: usize = 200;
const STEP_KINDS: [&str; 5] = [
    "tool_use",
    "assistant_message",
    "user_message",
    "finished",
    "api_retry",
];

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    if line.chars().count() > max {
        let mut cut: String = line.chars().take(max - 1).collect();
        cut.push('…');
        cut
    } else {
        line.to_owned()
    }
}

fn compact_input(input: &serde_json::Value) -> serde_json::Value {
    let Some(map) = input.as_object() else {
        return serde_json::Value::Null;
    };
    let kept = map
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "content" | "old_string" | "new_string"))
        .filter_map(|(k, v)| {
            let text = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(items) if k == "command" => items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some((
                k.clone(),
                serde_json::Value::String(first_line(&text, STEP_INPUT_MAX)),
            ))
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::Value::Object(kept)
}

#[must_use]
pub fn step_of(payload: &str) -> Option<WorkspaceStep> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let s = |k: &str| v[k].as_str().unwrap_or_default();
    let step = match s("type") {
        "tool_use" => WorkspaceStep {
            kind: "tool".into(),
            tool: s("name").to_owned(),
            input: compact_input(&v["input"]),
            text: String::new(),
        },
        "assistant_message" => WorkspaceStep {
            kind: "message".into(),
            text: first_line(s("text"), STEP_TEXT_MAX),
            ..WorkspaceStep::default()
        },
        "user_message" => WorkspaceStep {
            kind: "prompt".into(),
            text: first_line(s("text"), STEP_TEXT_MAX),
            ..WorkspaceStep::default()
        },
        "finished" if s("status") == "failed" => WorkspaceStep {
            kind: "error".into(),
            text: first_line(s("message"), STEP_TEXT_MAX),
            ..WorkspaceStep::default()
        },
        "finished" => WorkspaceStep {
            kind: "done".into(),
            text: first_line(s("text"), STEP_TEXT_MAX),
            ..WorkspaceStep::default()
        },
        "api_retry" => WorkspaceStep {
            kind: "retry".into(),
            text: first_line(s("error"), STEP_TEXT_MAX),
            input: serde_json::json!({
                "attempt": v["attempt"].as_u64().unwrap_or(0),
                "max_retries": v["max_retries"].as_u64().unwrap_or(0),
                "status": v["status"].as_u64().unwrap_or(0),
            }),
            ..WorkspaceStep::default()
        },
        _ => return None,
    };
    Some(step)
}

fn wants_step(activity: &str) -> bool {
    matches!(activity, "running" | "errored" | "awaiting_approval")
}

/// 跟读暂时连不上机器、agent 还在远端跑：当前一步写成 `reconnecting`，`text` 是机器名。
pub const RECONNECTING_STEP: &str = "reconnecting";

fn reconnecting_step(st: &AppState, activity: &str, session_id: &str) -> Option<WorkspaceStep> {
    if !matches!(activity, "running" | "awaiting_approval") {
        return None;
    }
    let sid = session_id.parse().map(blazar_core_types::SessionId).ok()?;
    let node = st.health.reconnecting(sid)?;
    Some(WorkspaceStep {
        kind: RECONNECTING_STEP.to_owned(),
        text: node,
        ..WorkspaceStep::default()
    })
}

async fn latest_step(st: &AppState, session_id: &str) -> Option<WorkspaceStep> {
    let filter = STEP_KINDS
        .iter()
        .map(|k| format!("payload LIKE '{{\"type\":\"{k}\"%'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let sql = format!(
        "SELECT payload FROM events WHERE session_id = ?1 AND ({filter}) ORDER BY seq DESC LIMIT 1"
    );
    let payload: Option<String> = sqlx::query_scalar(&sql)
        .bind(session_id)
        .fetch_optional(st.db.pool())
        .await
        .ok()
        .flatten();
    payload.as_deref().and_then(step_of)
}

pub(crate) async fn snapshot(st: &AppState) -> anyhow::Result<StateSnapshot> {
    let nodes: Vec<NodeView> = sqlx::query(
        "SELECT n.name, n.transport, n.endpoint, n.status, n.labels, n.network, n.role, n.last_seen_at,
                (SELECT COUNT(*) FROM workspaces w WHERE w.node_id = n.id AND w.kind = 'project') AS wc
         FROM nodes n ORDER BY n.name",
    )
    .fetch_all(st.db.pool())
    .await?
    .into_iter()
    .map(|r| {
        let labels: String = r.try_get("labels").unwrap_or_default();
        let meta: serde_json::Value = serde_json::from_str(&labels).unwrap_or_default();
        NodeView {
            name: r.try_get("name").unwrap_or_default(),
            transport: r.try_get("transport").unwrap_or_default(),
            ipv4: r.try_get::<Option<String>, _>("endpoint").unwrap_or(None),
            status: r.try_get("status").unwrap_or_default(),
            latency_ms: meta.get("latency_ms").and_then(serde_json::Value::as_f64),
            cost: meta
                .get("cost")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            workspace_count: r.try_get("wc").unwrap_or(0),
            network: r.try_get::<Option<String>, _>("network").unwrap_or(None),
            role: r.try_get::<Option<String>, _>("role").unwrap_or(None),
            last_seen_at: r.try_get::<Option<String>, _>("last_seen_at").unwrap_or(None),
        }
    })
    .collect();

    let rows = sqlx::query(
        "SELECT w.id, w.name, w.path, w.activity, w.last_active_at, w.parent_id, w.kind,
                w.diff_added, w.diff_removed, w.diff_files, w.diff_at,
                n.name AS node, p.name AS project,
                (SELECT s.id FROM sessions s WHERE s.workspace_id = w.id
                 ORDER BY s.created_at DESC LIMIT 1) AS session_id,
                (SELECT s.summary FROM sessions s WHERE s.workspace_id = w.id
                 ORDER BY s.created_at DESC LIMIT 1) AS summary,
                (SELECT s.ask FROM sessions s WHERE s.workspace_id = w.id
                 ORDER BY s.created_at DESC LIMIT 1) AS ask,
                (SELECT r.title FROM sessions s JOIN sessions r ON r.id = COALESCE(s.thread_id, s.id)
                 WHERE s.workspace_id = w.id ORDER BY s.created_at DESC LIMIT 1) AS title
         FROM workspaces w
         JOIN nodes n ON n.id = w.node_id
         LEFT JOIN projects p ON p.id = w.project_id
         ORDER BY p.name IS NULL, p.name, n.name, w.name",
    )
    .fetch_all(st.db.pool())
    .await?;
    let mut assistant = None;
    let mut workspaces: Vec<WorkspaceView> = Vec::with_capacity(rows.len());
    for r in rows {
        let activity: String = r.try_get("activity").unwrap_or_else(|_| "idle".into());
        let text = |k: &str| {
            r.try_get::<Option<String>, _>(k)
                .unwrap_or(None)
                .filter(|s| !s.trim().is_empty())
        };
        let summary = if matches!(activity.as_str(), "running" | "starting") {
            text("title")
        } else {
            text("summary")
        };
        let view = WorkspaceView {
            id: r.try_get("id").unwrap_or_default(),
            name: r.try_get("name").unwrap_or_default(),
            node: r.try_get("node").unwrap_or_default(),
            path: r.try_get("path").unwrap_or_default(),
            project: r.try_get("project").unwrap_or(None),
            activity,
            last_active_at: r.try_get("last_active_at").unwrap_or(None),
            session_id: r.try_get("session_id").unwrap_or(None),
            diff: match (
                r.try_get::<Option<i64>, _>("diff_added").unwrap_or(None),
                r.try_get::<Option<i64>, _>("diff_removed").unwrap_or(None),
                r.try_get::<Option<i64>, _>("diff_files").unwrap_or(None),
            ) {
                (Some(a), Some(d), Some(f)) => Some(blazar_core_types::api::DiffStat {
                    added: u64::try_from(a).unwrap_or(0),
                    removed: u64::try_from(d).unwrap_or(0),
                    files: u64::try_from(f).unwrap_or(0),
                }),
                _ => None,
            },
            diff_at: r.try_get("diff_at").unwrap_or(None),
            parent: r.try_get("parent_id").unwrap_or(None),
            step: None,
            failure: None,
            summary,
            ask: crate::brief::ask_from_row(&r, "ask"),
        };
        if r.try_get::<String, _>("kind").unwrap_or_default() == super::assistant::KIND {
            assistant.get_or_insert(view);
        } else {
            workspaces.push(view);
        }
    }

    let online = |node: &str| {
        node == "local" || nodes.iter().any(|n| n.name == node && n.status == "online")
    };
    for w in workspaces.iter_mut().chain(assistant.as_mut()) {
        if !wants_step(&w.activity) {
            continue;
        }
        if let Some(sid) = w.session_id.as_deref() {
            w.step = match reconnecting_step(st, &w.activity, sid) {
                Some(step) => Some(step),
                None => latest_step(st, sid).await,
            };
        }
        if w.activity == "errored" {
            w.failure = crate::failures::of_workspace(st, &w.id, online(&w.node)).await;
        }
    }

    Ok(StateSnapshot {
        nodes,
        workspaces,
        assistant,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_call_keeps_its_name_and_short_arguments() {
        let step = step_of(
            r#"{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"python -m pytest -q\n# second line","description":"跑测试","content":"big"}}"#,
        )
        .unwrap();
        assert_eq!(step.kind, "tool");
        assert_eq!(step.tool, "Bash");
        assert_eq!(step.input["command"], "python -m pytest -q");
        assert_eq!(step.input["description"], "跑测试");
        assert!(step.input.get("content").is_none());
    }

    #[test]
    fn command_arrays_and_long_values_are_flattened() {
        let long = "x".repeat(400);
        let step = step_of(&format!(
            r#"{{"type":"tool_use","id":"t","name":"exec","input":{{"command":["ls","-la"],"note":"{long}","n":3}}}}"#
        ))
        .unwrap();
        assert_eq!(step.input["command"], "ls -la");
        assert_eq!(step.input["n"], "3");
        assert_eq!(step.input["note"].as_str().unwrap().chars().count(), 200);
        assert!(step.input["note"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn messages_failures_and_retries_become_one_line_each() {
        let said =
            step_of(r#"{"type":"assistant_message","text":"\n\n改好了。\n第二行"}"#).unwrap();
        assert_eq!(
            (said.kind.as_str(), said.text.as_str()),
            ("message", "改好了。")
        );
        let asked = step_of(r#"{"type":"user_message","text":"补测试"}"#).unwrap();
        assert_eq!(
            (asked.kind.as_str(), asked.text.as_str()),
            ("prompt", "补测试")
        );
        let failed =
            step_of(r#"{"type":"finished","status":"failed","message":"API Error: 529"}"#).unwrap();
        assert_eq!(
            (failed.kind.as_str(), failed.text.as_str()),
            ("error", "API Error: 529")
        );
        let done = step_of(r#"{"type":"finished","status":"success","text":"ok"}"#).unwrap();
        assert_eq!(done.kind, "done");
        let retry = step_of(
            r#"{"type":"api_retry","attempt":2,"max_retries":10,"status":529,"error":"server_error","delay_ms":4000}"#,
        )
        .unwrap();
        assert_eq!(
            (retry.kind.as_str(), retry.text.as_str()),
            ("retry", "server_error")
        );
        assert_eq!(retry.input["attempt"], 2);
        assert_eq!(retry.input["max_retries"], 10);
        assert_eq!(retry.input["status"], 529);
        assert!(step_of(r#"{"type":"thinking","text":"hmm"}"#).is_none());
        assert!(step_of("not json").is_none());
    }

    #[test]
    fn only_active_or_failed_workspaces_carry_a_step() {
        assert!(wants_step("running"));
        assert!(wants_step("awaiting_approval"));
        assert!(wants_step("errored"));
        assert!(!wants_step("idle"));
        assert!(!wants_step("completed"));
    }
}
