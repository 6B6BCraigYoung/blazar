use std::time::Duration;

use blazar_core_types::failure::FailureCause;
use blazar_core_types::{EntryKind, NormalizedEntry, SessionId, WorkspaceId};
use serde_json::{Value, json};

use crate::api::{PromptRequest, Shared};
use crate::state::ServerEvent;

pub const BACKOFF_SECS: [u64; 3] = [15, 30, 60];

pub const DEFAULT_ATTEMPTS: u64 = 20;

pub const MAX_ATTEMPTS: u64 = 100;

const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com";

#[must_use]
pub fn eligible(cause: FailureCause) -> bool {
    matches!(
        cause,
        FailureCause::Network | FailureCause::Busy | FailureCause::Node
    )
}

#[must_use]
pub fn delay(attempt: u64) -> Duration {
    let at = usize::try_from(attempt).unwrap_or(usize::MAX);
    Duration::from_secs(BACKOFF_SECS[at.min(BACKOFF_SECS.len() - 1)])
}

#[must_use]
pub fn exhausted(attempt: u64, limit: u64) -> bool {
    attempt >= limit
}

#[must_use]
pub fn settings_of(saved: &Value) -> Value {
    json!({
        "enabled": saved["auto_resume"]["enabled"].as_bool().unwrap_or(true),
        "max_attempts": saved["auto_resume"]["max_attempts"]
            .as_u64()
            .filter(|n| (1..=MAX_ATTEMPTS).contains(n))
            .unwrap_or(DEFAULT_ATTEMPTS),
    })
}

pub fn merge(cur: &mut Value, body: &Value) -> Result<(), String> {
    if let Some(v) = body["auto_resume"]["enabled"].as_bool() {
        cur["auto_resume"]["enabled"] = json!(v);
    }
    if let Some(n) = body["auto_resume"].get("max_attempts") {
        match n.as_u64().filter(|n| (1..=MAX_ATTEMPTS).contains(n)) {
            Some(n) => cur["auto_resume"]["max_attempts"] = json!(n),
            None => return Err(format!("自动续跑次数要在 1 到 {MAX_ATTEMPTS} 之间")),
        }
    }
    Ok(())
}

async fn settings(st: &Shared) -> (bool, u64) {
    let s = settings_of(&crate::office::kv_get(st, "prefs").await);
    (
        s["enabled"].as_bool().unwrap_or(true),
        s["max_attempts"].as_u64().unwrap_or(DEFAULT_ATTEMPTS),
    )
}

pub async fn takes_over(st: &Shared, sid: SessionId) -> bool {
    settings(st).await.0
        && crate::failures::cause_of(st, &sid.to_string())
            .await
            .is_some_and(eligible)
}

#[must_use]
pub fn continue_request(mut req: Value, thread: &str) -> Value {
    req["text"] = json!("Continue");
    req["images"] = json!([]);
    req["context_file"] = Value::Null;
    req["resume"] = json!(true);
    req["resume_at_last"] = json!(false);
    req["resume_session"] = json!(thread);
    req["interject"] = json!(false);
    req["wait_secs"] = json!(0);
    req
}

pub(crate) fn send_again(
    st: Shared,
    ws: String,
    req: PromptRequest,
) -> futures::future::BoxFuture<'static, Result<Value, String>> {
    Box::pin(crate::api::prompt_for_bot(st, ws, req))
}

async fn reachable() -> bool {
    let endpoint = std::env::var("ANTHROPIC_BASE_URL")
        .ok()
        .filter(|u| u.starts_with("http"))
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned());
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return false;
    };
    client.head(endpoint).send().await.is_ok()
}

struct Failed {
    ws: String,
    thread: String,
    created: String,
    request: Option<String>,
}

async fn load(st: &Shared, sid: SessionId) -> Option<Failed> {
    let row: (String, String, String, Option<String>) = sqlx::query_as(
        "SELECT workspace_id, COALESCE(thread_id, id), created_at, request FROM sessions WHERE id = ?1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()?;
    Some(Failed {
        ws: row.0,
        thread: row.1,
        created: row.2,
        request: row.3,
    })
}

async fn streak(st: &Shared, f: &Failed) -> u64 {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions s WHERE COALESCE(s.thread_id, s.id) = ?1
           AND s.status = 'failed' AND s.failure_cause IN ('network', 'busy', 'node')
           AND s.created_at < ?2
           AND s.created_at > COALESCE((SELECT MAX(d.created_at) FROM sessions d
                 WHERE COALESCE(d.thread_id, d.id) = ?1 AND d.status <> 'failed'
                   AND d.created_at < ?2), '')",
    )
    .bind(&f.thread)
    .bind(&f.created)
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0);
    u64::try_from(n).unwrap_or(0)
}

async fn still_waiting(st: &Shared, sid: SessionId) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sessions s WHERE s.id = ?1 AND s.status = 'failed'
           AND s.handled_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM sessions n WHERE n.workspace_id = s.workspace_id
                           AND n.created_at > s.created_at)",
    )
    .bind(sid.to_string())
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0)
        > 0
}

pub(crate) async fn note(st: &Shared, sid: SessionId, ws: &str, message: String) {
    let Ok(id) = ws.parse().map(WorkspaceId) else {
        return;
    };
    let Ok(seq) = st.db.max_seq(sid).await else {
        return;
    };
    let entry = NormalizedEntry {
        seq: seq + 1,
        ts: chrono::Utc::now(),
        parent_tool_use_id: None,
        kind: EntryKind::Notice { message },
    };
    if st.db.append_event(sid, id, &entry).await.is_ok() {
        st.emit(ServerEvent::Entry {
            workspace_id: id,
            session_id: sid,
            entry: Box::new(entry),
        });
    }
}

/// 工作区的显示名和所在机器。
pub(crate) async fn place(st: &Shared, ws: &str) -> (String, String) {
    sqlx::query_as::<_, (String, String)>(
        "SELECT w.name, n.name FROM workspaces w JOIN nodes n ON n.id = w.node_id WHERE w.id = ?1",
    )
    .bind(ws)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .map(|(name, node)| (blazar_core_types::api::workspace_label(&name, &node), node))
    .unwrap_or_default()
}

async fn give_up(st: &Shared, sid: SessionId, f: &Failed, why: &str, why_en: &str) {
    let headline =
        if crate::failures::cause_of(st, &sid.to_string()).await == Some(FailureCause::Node) {
            "机器连不上，没能自动继续"
        } else {
            "网络没恢复，没能自动继续"
        };
    give_up_as(st, sid, f, why, why_en, headline).await;
}

async fn give_up_as(
    st: &Shared,
    sid: SessionId,
    f: &Failed,
    why: &str,
    why_en: &str,
    headline: &str,
) {
    let (label, _) = place(st, &f.ws).await;
    let title = format!("{label} · {headline}");
    // 机器每次探测都会走“回来了”的流程：同一次失败已经报过同样的卡片就不再报。
    let reported: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inbox WHERE kind = 'run_failed' AND session_id = ?1
           AND title = ?2 AND resolved_at IS NULL AND archived_at IS NULL",
    )
    .bind(sid.to_string())
    .bind(&title)
    .fetch_one(st.db.pool())
    .await
    .unwrap_or(0);
    if reported > 0 {
        return;
    }
    note(st, sid, &f.ws, format!("Auto-resume stopped: {why_en}")).await;
    let message = crate::failures::last_message(st, &sid.to_string()).await;
    crate::inbox::push(
        st,
        crate::inbox::Item {
            kind: "run_failed",
            title,
            body: format!("{why}\n{}", message.chars().take(200).collect::<String>()),
            workspace_id: Some(f.ws.clone()),
            thread_id: Some(f.thread.clone()),
            session_id: Some(sid.to_string()),
            ref_id: None,
        },
    )
    .await;
}

pub async fn on_run_finished(st: Shared, sid: SessionId, status: &'static str) {
    if status != "failed" || !takes_over(&st, sid).await {
        return;
    }
    let cause = crate::failures::cause_of(&st, &sid.to_string()).await;
    let message = crate::failures::last_message(&st, &sid.to_string()).await;
    let limit = capped_limit(settings(&st).await.1, &message);
    let Some(f) = load(&st, sid).await else {
        return;
    };
    let Some(req) = f
        .request
        .as_deref()
        .and_then(|r| serde_json::from_str::<Value>(r).ok())
    else {
        give_up(
            &st,
            sid,
            &f,
            "没有留下原始请求",
            "the original request was not saved",
        )
        .await;
        return;
    };
    let (_, node) = place(&st, &f.ws).await;
    let online = node_online(&st, &node).await;
    let start = streak(&st, &f).await;
    if exhausted(start, limit) {
        give_up(
            &st,
            sid,
            &f,
            &format!("已连续自动续跑 {start} 次"),
            &format!("already auto-resumed {start} times in a row"),
        )
        .await;
        return;
    }
    if waits_for_machine(cause, online) {
        // 机器连不上：不在这里轮询，等它回来时由 failures::node_back 接着做。机器在线的
        // Node 失败（本机、或 agent 进程没了但机器还在）照常往下走，直接重跑。
        note(
            &st,
            sid,
            &f.ws,
            format!(
                "Will continue automatically once {} is back (up to {} more attempts)",
                machine(&node),
                limit - start
            ),
        )
        .await;
        return;
    }
    note(
        &st,
        sid,
        &f.ws,
        format!(
            "Will continue automatically once the connection is back (up to {} more attempts)",
            limit - start
        ),
    )
    .await;
    let mut attempt = start;
    while !exhausted(attempt, limit) {
        tokio::time::sleep(delay(attempt)).await;
        attempt += 1;
        if !still_waiting(&st, sid).await {
            tracing::info!(target: "blazar::auto_resume", "{sid} 已被手动处理，取消自动续跑");
            return;
        }
        if !back_up(&st, &node).await {
            continue;
        }
        if crate::leftover::take_back(&st, &f.ws, &f.thread).await {
            tracing::info!(target: "blazar::auto_resume", "{sid} 这条对话的智能体其实还在运行，已接回，不再另开一轮");
            return;
        }
        let Ok(parsed) =
            serde_json::from_value::<PromptRequest>(continue_request(req.clone(), &f.thread))
        else {
            give_up(
                &st,
                sid,
                &f,
                "原始请求读不回来",
                "the original request could not be read",
            )
            .await;
            return;
        };
        note(
            &st,
            sid,
            &f.ws,
            format!("Connection is back. Continuing automatically (attempt {attempt})"),
        )
        .await;
        match send_again(st.clone(), f.ws.clone(), parsed).await {
            Ok(v) if v["admitted"] != json!(false) => {
                let _ = crate::failures::handle(&st, &f.ws, Some(&sid.to_string())).await;
                tracing::info!(target: "blazar::auto_resume", "{sid} 已自动继续");
                return;
            }
            Ok(v) => {
                tracing::info!(target: "blazar::auto_resume", "{sid} 自动继续没被接受：{v}");
            }
            Err(e) => tracing::warn!(target: "blazar::auto_resume", "{sid} 自动继续失败：{e}"),
        }
    }
    give_up(
        &st,
        sid,
        &f,
        &format!("试了 {limit} 次都没恢复"),
        &format!("the connection did not come back after {limit} attempts"),
    )
    .await;
}

pub const PROXY_REJECT_ATTEMPTS: u64 = 3;

/// 代理落在被拒区域的 403 会自己好，但别无限重试：顶多试 PROXY_REJECT_ATTEMPTS 次。
#[must_use]
pub fn capped_limit(full: u64, message: &str) -> u64 {
    if blazar_core_types::failure::proxy_rejected(message) {
        full.min(PROXY_REJECT_ATTEMPTS)
    } else {
        full
    }
}

/// 只有「机器连不上且此刻仍不在线」才交给 node_back 等；其余（网络、繁忙、本机、或机器已
/// 在线的 Node 失败）都就地重跑。
#[must_use]
pub fn waits_for_machine(cause: Option<FailureCause>, node_online: bool) -> bool {
    cause == Some(FailureCause::Node) && !node_online
}

/// 续跑前的放行：本机看 API 通不通；远端看那台机器在不在线，让重跑去试它自己到 API 的链路。
async fn back_up(st: &Shared, node: &str) -> bool {
    if node == "local" {
        reachable().await
    } else {
        node_online(st, node).await
    }
}

async fn node_online(st: &Shared, node: &str) -> bool {
    node == "local"
        || sqlx::query_scalar::<_, String>("SELECT status FROM nodes WHERE name = ?1")
            .bind(node)
            .fetch_optional(st.db.pool())
            .await
            .ok()
            .flatten()
            .is_some_and(|s| s == "online")
}

/// hub 重启后，还在等自动续跑的失败会话（内存里的轮询任务已经随进程没了）重新接上：
/// 网络 / 繁忙的接着轮询；机器已经回来的 Node 失败直接重跑；机器还没回来的留给 node_back。
pub async fn rearm(st: Shared) {
    if !settings(&st).await.0 {
        return;
    }
    for id in waiting_sessions(&st).await {
        if let Ok(sid) = id.parse().map(SessionId) {
            st.services
                .spawn(on_run_finished(st.clone(), sid, "failed"));
        }
    }
}

/// 还在等自动续跑的失败会话：各工作区里最新的那一轮、没人处理过、且原因是可续跑的那几类。
async fn waiting_sessions(st: &Shared) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT s.id FROM sessions s
         WHERE s.status = 'failed' AND s.handled_at IS NULL
           AND s.failure_cause IN ('network', 'busy', 'node')
           AND s.id = (SELECT x.id FROM sessions x WHERE x.workspace_id = s.workspace_id
                       ORDER BY x.created_at DESC LIMIT 1)",
    )
    .fetch_all(st.db.pool())
    .await
    .unwrap_or_default()
}

fn machine(node: &str) -> String {
    if node == "local" {
        "this computer".to_owned()
    } else {
        node.to_owned()
    }
}

/// 因机器连不上而中断的会话，机器回来了：开着自动续跑就接着做（算进连续续跑次数），
/// 关着就保留卡片、改成“机器回来了，可以继续”。同一个会话只会续跑一次。
pub(crate) async fn on_node_back<S>(st: &Shared, sid: SessionId, node: &str, send: &S)
where
    S: Fn(
        Shared,
        String,
        PromptRequest,
    ) -> futures::future::BoxFuture<'static, Result<Value, String>>,
{
    let Some(_claim) = st.health.claim_resume(sid) else {
        return;
    };
    if !still_waiting(st, sid).await {
        return;
    }
    let Some(f) = load(st, sid).await else {
        return;
    };
    let (enabled, limit) = settings(st).await;
    if !enabled {
        offer_continue(st, sid, &f, node).await;
        return;
    }
    let headline = format!("{node} 回来了，没能自动继续");
    let Some(req) = f
        .request
        .as_deref()
        .and_then(|r| serde_json::from_str::<Value>(r).ok())
    else {
        give_up_as(
            st,
            sid,
            &f,
            "没有留下原始请求",
            "the original request was not saved",
            &headline,
        )
        .await;
        return;
    };
    let start = streak(st, &f).await;
    if exhausted(start, limit) {
        give_up_as(
            st,
            sid,
            &f,
            &format!("已连续自动续跑 {start} 次"),
            &format!("already auto-resumed {start} times in a row"),
            &headline,
        )
        .await;
        return;
    }
    let Ok(parsed) = serde_json::from_value::<PromptRequest>(continue_request(req, &f.thread))
    else {
        give_up_as(
            st,
            sid,
            &f,
            "原始请求读不回来",
            "the original request could not be read",
            &headline,
        )
        .await;
        return;
    };
    note(
        st,
        sid,
        &f.ws,
        format!(
            "{} is back. Continuing automatically (attempt {})",
            machine(node),
            start + 1
        ),
    )
    .await;
    match send(st.clone(), f.ws.clone(), parsed).await {
        Ok(v) if v["admitted"] != json!(false) => {
            let _ = crate::failures::handle(st, &f.ws, Some(&sid.to_string())).await;
            tracing::info!(target: "blazar::auto_resume", "{node} 回来了，{sid} 已自动继续");
        }
        Ok(v) => {
            tracing::info!(target: "blazar::auto_resume", "{sid} 机器回来后自动继续没被接受：{v}");
            offer_continue(st, sid, &f, node).await;
        }
        Err(e) => {
            tracing::warn!(target: "blazar::auto_resume", "{sid} 机器回来后自动继续失败：{e}");
            offer_continue(st, sid, &f, node).await;
        }
    }
}

/// 卡片标题：机器回来了，可以继续。
#[must_use]
pub fn back_title(label: &str, node: &str) -> String {
    if label.is_empty() {
        format!("{node} 回来了，可以继续")
    } else {
        format!("{label} · {node} 回来了，可以继续")
    }
}

/// 不自动续跑时：保留（或补上）这次失败的卡片，改成“机器回来了，可以继续”，并重新标为未读。
async fn offer_continue(st: &Shared, sid: SessionId, f: &Failed, node: &str) {
    let (label, _) = place(st, &f.ws).await;
    let title = back_title(&label, node);
    let body = "机器重新连上了，点「继续」从停下的地方接着做。".to_owned();
    let card: Option<(String, String)> = sqlx::query_as(
        "SELECT id, title FROM inbox WHERE kind = 'run_failed' AND session_id = ?1
           AND resolved_at IS NULL AND archived_at IS NULL ORDER BY created_at DESC LIMIT 1",
    )
    .bind(sid.to_string())
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten();
    match card {
        Some((_, t)) if t == title => return,
        Some((id, _)) => {
            let _ =
                sqlx::query("UPDATE inbox SET title = ?2, body = ?3, read_at = NULL WHERE id = ?1")
                    .bind(id)
                    .bind(&title)
                    .bind(&body)
                    .execute(st.db.pool())
                    .await;
            st.emit(ServerEvent::InboxChanged);
        }
        None => {
            crate::inbox::push(
                st,
                crate::inbox::Item {
                    kind: "run_failed",
                    title,
                    body,
                    workspace_id: Some(f.ws.clone()),
                    thread_id: Some(f.thread.clone()),
                    session_id: Some(sid.to_string()),
                    ref_id: None,
                },
            )
            .await;
        }
    }
    note(
        st,
        sid,
        &f.ws,
        format!(
            "{} is back. Press Continue to pick up where it stopped",
            machine(node)
        ),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_network_kinds_resume() {
        let hit = [
            "API Error: getaddrinfo ENOTFOUND api.anthropic.com",
            "API Error: read ECONNRESET",
            "API Error: Connection lost while your computer was asleep",
            "API Error: Self-signed certificate detected",
            "API Error: 529 overloaded_error",
            "API Error: 503 Service Unavailable",
        ];
        for m in hit {
            assert!(eligible(FailureCause::of(m, true)), "{m}");
        }
        let miss = [
            "API Error: 429 rate_limit_error",
            "Not logged in · Please run /login",
            "No message found with message.uuid of: x",
            "agent exited unexpectedly (exit code 1)",
        ];
        for m in miss {
            assert!(!eligible(FailureCause::of(m, true)), "{m}");
        }
        // 机器连不上的失败等机器回来再接着做。
        assert!(eligible(FailureCause::of(
            "ssh: connect to host gpu-1 port 22: No route to host",
            true
        )));
        assert!(eligible(FailureCause::of("API Error: ECONNRESET", false)));
    }

    #[test]
    fn backoff_caps_at_a_minute() {
        let secs: Vec<u64> = (0..6).map(|a| delay(a).as_secs()).collect();
        assert_eq!(secs, [15, 30, 60, 60, 60, 60]);
        assert_eq!(delay(u64::MAX).as_secs(), 60);
    }

    #[test]
    fn attempts_stop_at_the_limit() {
        assert!(!exhausted(0, DEFAULT_ATTEMPTS));
        assert!(!exhausted(19, DEFAULT_ATTEMPTS));
        assert!(exhausted(20, DEFAULT_ATTEMPTS));
        let saved = json!({ "auto_resume": { "max_attempts": 3 } });
        assert_eq!(settings_of(&saved)["max_attempts"], 3);
        assert_eq!(settings_of(&json!({}))["max_attempts"], DEFAULT_ATTEMPTS);
        assert_eq!(settings_of(&json!({}))["enabled"], true);
        let mut cur = json!({});
        assert!(merge(&mut cur, &json!({ "auto_resume": { "max_attempts": 0 } })).is_err());
        assert!(merge(&mut cur, &json!({ "auto_resume": { "max_attempts": 101 } })).is_err());
        merge(
            &mut cur,
            &json!({ "auto_resume": { "enabled": false, "max_attempts": 5 } }),
        )
        .unwrap();
        assert_eq!(
            cur["auto_resume"],
            json!({ "enabled": false, "max_attempts": 5 })
        );
    }

    #[test]
    fn a_proxy_reject_is_capped_low_while_a_plain_network_fault_keeps_the_full_budget() {
        assert_eq!(
            capped_limit(20, "API Error: Connection dropped (ECONNRESET)"),
            20
        );
        assert_eq!(
            capped_limit(
                20,
                "Failed to authenticate. API Error: 403 Request not allowed"
            ),
            PROXY_REJECT_ATTEMPTS
        );
        assert_eq!(
            capped_limit(2, "403 request not allowed"),
            2,
            "别超过用户设的上限"
        );
    }

    #[test]
    fn only_an_offline_machine_waits_for_node_back() {
        use FailureCause::*;
        assert!(waits_for_machine(Some(Node), false));
        assert!(!waits_for_machine(Some(Node), true), "机器在线就直接重跑");
        assert!(!waits_for_machine(Some(Network), false));
        assert!(!waits_for_machine(Some(Busy), true));
        assert!(!waits_for_machine(None, false));
    }

    async fn db() -> Shared {
        let db = blazar_db::Db::open_in_memory().await.unwrap();
        crate::state::AppState::with_services(
            db,
            "local".into(),
            None,
            crate::mesh::MeshCtx::new(None, std::env::temp_dir().join("blazar-resume-tests")),
            crate::services::Services::Isolated,
        )
    }

    #[tokio::test]
    async fn an_online_machine_reads_as_up_and_offline_as_down() {
        let st = db().await;
        sqlx::query("INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n1','gpu-x','ssh','online','0'),('n2','gpu-y','ssh','offline','0')")
            .execute(st.db.pool()).await.unwrap();
        assert!(node_online(&st, "local").await, "本机总当在线");
        assert!(node_online(&st, "gpu-x").await);
        assert!(!node_online(&st, "gpu-y").await);
        assert!(!node_online(&st, "gone").await);
    }

    #[tokio::test]
    async fn rearm_picks_the_latest_unhandled_resumable_failure_per_workspace() {
        let st = db().await;
        sqlx::query("INSERT INTO nodes (id, name, transport, status, created_at) VALUES ('n1','local','local','online','0')")
            .execute(st.db.pool()).await.unwrap();
        let ws = |id: &str| {
            format!(
                "INSERT INTO workspaces (id, node_id, name, path, activity, created_at) VALUES ('{id}','n1','w','/tmp/{id}','errored','0')"
            )
        };
        for w in ["w1", "w2", "w3", "w4"] {
            sqlx::query(&ws(w)).execute(st.db.pool()).await.unwrap();
        }
        let sess = |id: &str, w: &str, cause: &str, created: &str, handled: &str, status: &str| {
            format!(
                "INSERT INTO sessions (id, workspace_id, runtime_kind, status, failure_cause, handled_at, created_at) VALUES ('{id}','{w}','claude','{status}','{cause}',{handled},'{created}')"
            )
        };
        // w1: newer resumable failure wins over an older one.
        sqlx::query(&sess(
            "10000000-0000-0000-0000-000000000001",
            "w1",
            "login",
            "1",
            "NULL",
            "failed",
        ))
        .execute(st.db.pool())
        .await
        .unwrap();
        sqlx::query(&sess(
            "10000000-0000-0000-0000-000000000002",
            "w1",
            "network",
            "2",
            "NULL",
            "failed",
        ))
        .execute(st.db.pool())
        .await
        .unwrap();
        // w2: latest run is a success → nothing to resume.
        sqlx::query(&sess(
            "20000000-0000-0000-0000-000000000001",
            "w2",
            "network",
            "1",
            "NULL",
            "failed",
        ))
        .execute(st.db.pool())
        .await
        .unwrap();
        sqlx::query(&sess(
            "20000000-0000-0000-0000-000000000002",
            "w2",
            "",
            "2",
            "NULL",
            "done",
        ))
        .execute(st.db.pool())
        .await
        .unwrap();
        // w3: already handled → skip.
        sqlx::query(&sess(
            "30000000-0000-0000-0000-000000000001",
            "w3",
            "node",
            "1",
            "'9'",
            "failed",
        ))
        .execute(st.db.pool())
        .await
        .unwrap();
        // w4: a non-resumable cause → skip.
        sqlx::query(&sess(
            "40000000-0000-0000-0000-000000000001",
            "w4",
            "quota",
            "1",
            "NULL",
            "failed",
        ))
        .execute(st.db.pool())
        .await
        .unwrap();

        let mut got = waiting_sessions(&st).await;
        got.sort();
        assert_eq!(got, vec!["10000000-0000-0000-0000-000000000002".to_owned()]);
    }

    #[test]
    fn continuing_keeps_the_permission_mode() {
        let stored = json!({ "text": "改一下", "permission_mode": "acceptEdits", "images": [{}], "model": "opus" });
        let r = continue_request(stored, "t1");
        assert_eq!(r["text"], "Continue");
        assert_eq!(r["resume"], true);
        assert_eq!(r["resume_session"], "t1");
        assert_eq!(r["permission_mode"], "acceptEdits");
        assert_eq!(r["model"], "opus");
        assert_eq!(r["images"], json!([]));
    }
}
