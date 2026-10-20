use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Caller, caller_of, claims, facts, jobs, memory, status};
use crate::api::Shared;

fn fail(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn bad(msg: impl Into<String>) -> Response {
    fail(StatusCode::BAD_REQUEST, msg)
}

fn not_found(msg: impl Into<String>) -> Response {
    fail(StatusCode::NOT_FOUND, msg)
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub recent: Option<usize>,
    #[serde(default)]
    pub archived: Option<bool>,
}

pub async fn list_bots(State(st): State<Shared>, Query(q): Query<ListQuery>) -> Response {
    let recent = q.recent.unwrap_or(0).min(20);
    let list = if q.archived == Some(true) {
        super::archived(&st).await
    } else {
        super::all(&st).await
    };
    let mut out = Vec::new();
    for b in list {
        out.push(status::bot_status(&st, &b, recent).await);
    }
    Json(json!({ "bots": out })).into_response()
}

pub async fn spawn_bot(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<super::Spawn>,
) -> Response {
    match super::spawn(&st, req, caller_of(&headers).as_ref()).await {
        Ok((b, m)) => {
            let mut v = status::bot_status(&st, &b, 0).await;
            v["mail"] = json!(m);
            Json(v).into_response()
        }
        Err(e) => bad(e),
    }
}

pub async fn promote_bot(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match super::promote(&st, &id).await {
        Ok(b) => Json(status::bot_status(&st, &b, 0).await).into_response(),
        Err(e) => not_found(e),
    }
}

pub async fn restore_bot(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match super::restore(&st, &id).await {
        Ok(b) => Json(status::bot_status(&st, &b, 0).await).into_response(),
        Err(e) => bad(e),
    }
}

pub async fn create_bot(State(st): State<Shared>, Json(req): Json<super::NewBot>) -> Response {
    match super::create(&st, req).await {
        Ok(b) => Json(status::bot_status(&st, &b, 0).await).into_response(),
        Err(e) => bad(e),
    }
}

pub async fn get_bot(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(b) = super::get(&st, &id).await else {
        return not_found("没有这个 bot");
    };
    let mut v = status::bot_status(&st, &b, 12).await;
    v["all_threads"] = json!(super::threads(&st, &b.id).await);
    v["mailbox"] = json!(super::mailbox(&st, &b.id, None, 50).await);
    v["sent"] = json!(super::sent_by(&st, &b.id, 30).await);
    v["memories"] = json!(memory::list(&st, Some(&format!("bot:{}", b.id)), None).await);
    Json(v).into_response()
}

pub async fn update_bot(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(p): Json<super::Patch>,
) -> Response {
    match super::update(&st, &id, p).await {
        Ok(b) => Json(status::bot_status(&st, &b, 0).await).into_response(),
        Err(e) => bad(e),
    }
}

pub async fn archive_bot(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match super::archive(&st, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => not_found(e),
    }
}

pub async fn pause_bot(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match super::pause(&st, &id, true).await {
        Ok(b) => Json(status::bot_status(&st, &b, 0).await).into_response(),
        Err(e) => not_found(e),
    }
}

pub async fn resume_bot(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match super::pause(&st, &id, false).await {
        Ok(b) => Json(status::bot_status(&st, &b, 0).await).into_response(),
        Err(e) => not_found(e),
    }
}

pub async fn bot_status(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(b) = super::get(&st, &id).await else {
        return not_found("没有这个 bot");
    };
    Json(status::bot_status(&st, &b, 8).await).into_response()
}

pub async fn fleet_status(State(st): State<Shared>) -> Response {
    Json(status::fleet_status(&st, 3).await).into_response()
}

#[derive(Deserialize)]
pub struct ThreadBody {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

pub async fn open_thread(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<ThreadBody>,
) -> Response {
    let Some(bot) = super::get(&st, &id).await else {
        return not_found("没有这个 bot");
    };
    match super::open_thread(
        &st,
        &bot.id,
        b.kind.as_deref().unwrap_or("task"),
        b.title.as_deref().unwrap_or(""),
    )
    .await
    {
        Ok(t) => {
            super::changed(&st);
            Json(json!(t)).into_response()
        }
        Err(e) => bad(e),
    }
}

pub async fn close_thread(
    State(st): State<Shared>,
    Path((id, tid)): Path<(String, String)>,
    Json(b): Json<ThreadBody>,
) -> Response {
    let Some(bot) = super::get(&st, &id).await else {
        return not_found("没有这个 bot");
    };
    match super::thread(&st, &tid).await {
        Some(t) if t.bot_id == bot.id => {
            match super::close_thread(&st, &t.id, b.summary.as_deref().unwrap_or("")).await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => bad(e),
            }
        }
        _ => not_found("没有这个线程"),
    }
}

pub async fn send_mail(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(mut req): Json<super::NewMail>,
) -> Response {
    match caller_of(&headers) {
        Some(c) => {
            req.from_kind = "bot".into();
            req.from_ref = c.bot.clone();
            req.from_thread = super::caller_thread(&st, &c).await;
        }
        None => {
            req.from_kind = "user".into();
            req.from_ref = String::new();
            req.from_thread = None;
        }
    }
    match super::send(&st, &id, req).await {
        Ok(m) => Json(json!(m)).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
pub struct MailQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

pub async fn mailbox(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<MailQuery>,
) -> Response {
    let Some(bot) = super::get(&st, &id).await else {
        return not_found("没有这个 bot");
    };
    let status = q.status.filter(|s| !s.is_empty());
    Json(json!({
        "mail": super::mailbox(&st, &bot.id, status.as_deref(), q.limit.unwrap_or(100)).await
    }))
    .into_response()
}

pub async fn get_mail(State(st): State<Shared>, Path(id): Path<String>) -> Response {
    match super::mail(&st, &id).await {
        Some(m) => {
            let replies = super::replies_to(&st, &m.id).await;
            Json(json!({ "mail": m, "replies": replies })).into_response()
        }
        None => not_found("没有这封邮件"),
    }
}

#[derive(Deserialize)]
pub struct ReplyBody {
    pub body: String,
}

pub async fn reply_mail(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(b): Json<ReplyBody>,
) -> Response {
    let Some(c) = caller_of(&headers) else {
        return bad("reply 只给 bot 用；你要联系 bot 直接给它发邮件");
    };
    match super::reply(&st, &id, &b.body, &c).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
pub struct WaitQuery {
    #[serde(default)]
    pub timeout: Option<u64>,
}

pub async fn wait_mail(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<WaitQuery>,
) -> Response {
    match super::wait_reply(&st, &id, q.timeout.unwrap_or(120)).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => not_found(e),
    }
}

#[derive(Deserialize)]
pub struct FactsQuery {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub history: Option<bool>,
}

pub async fn list_facts(State(st): State<Shared>, Query(q): Query<FactsQuery>) -> Response {
    Json(json!({
        "facts": facts::list(&st, q.prefix.as_deref().unwrap_or(""), q.limit.unwrap_or(500)).await
    }))
    .into_response()
}

pub async fn get_fact(
    State(st): State<Shared>,
    Path(key): Path<String>,
    Query(q): Query<FactsQuery>,
) -> Response {
    match facts::get(&st, &key).await {
        Some(f) => {
            let mut v = json!(f);
            if q.history == Some(true) {
                v["history"] = json!(facts::history(&st, &key, q.limit.unwrap_or(50)).await);
            }
            Json(v).into_response()
        }
        None => not_found("没有这条事实"),
    }
}

pub async fn put_fact(
    State(st): State<Shared>,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(p): Json<facts::Put>,
) -> Response {
    let caller = caller_of(&headers);
    match facts::put(&st, &key, p, caller.as_ref()).await {
        Ok(f) => Json(json!(f)).into_response(),
        Err(e) => bad(e),
    }
}

pub async fn delete_fact(
    State(st): State<Shared>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    match facts::remove(&st, &key, caller_of(&headers).as_ref()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

pub async fn report_facts(State(st): State<Shared>, Json(body): Json<Value>) -> Response {
    let items: Vec<Value> = match body {
        Value::Array(list) => list,
        single => vec![single],
    };
    let mut written = Vec::new();
    for item in items {
        let Ok(p) = serde_json::from_value::<facts::Put>(item.clone()) else {
            return bad("每条上报要有 key 和 value");
        };
        let Some(key) = p.key.clone() else {
            return bad("每条上报要有 key");
        };
        match facts::put(&st, &key, p, None).await {
            Ok(f) => written.push(f.key),
            Err(e) => return bad(format!("{key}: {e}")),
        }
    }
    Json(json!({ "written": written })).into_response()
}

pub async fn get_report_token(State(st): State<Shared>) -> Response {
    match facts::report_token(&st, false).await {
        Ok(t) => Json(json!({ "token": t })).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn rotate_report_token(State(st): State<Shared>) -> Response {
    match facts::report_token(&st, true).await {
        Ok(t) => Json(json!({ "token": t })).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
pub struct ClaimsQuery {
    #[serde(default)]
    pub bot: Option<String>,
}

pub async fn list_claims(
    State(st): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<ClaimsQuery>,
) -> Response {
    let who = q.bot.or_else(|| caller_of(&headers).map(|c| c.bot));
    let list = match who.as_deref().filter(|b| !b.is_empty()) {
        Some(b) => match super::get(&st, b).await {
            Some(bot) => claims::of_bot(&st, &bot.id).await,
            None => return not_found("没有这个 bot"),
        },
        None => claims::all(&st).await,
    };
    Json(json!({ "claims": list })).into_response()
}

pub async fn create_claim(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<claims::NewClaim>,
) -> Response {
    match claims::create(&st, req, caller_of(&headers).as_ref()).await {
        Ok(c) => Json(json!(c)).into_response(),
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}

pub async fn release_claim(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match claims::release(&st, &id, caller_of(&headers).as_ref()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
pub struct MemoryQuery {
    #[serde(default)]
    pub scope: Option<String>,
}

pub async fn list_memories(
    State(st): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<MemoryQuery>,
) -> Response {
    let caller = caller_of(&headers);
    let scope = q.scope.filter(|s| !s.is_empty());
    Json(json!({ "memories": memory::list(&st, scope.as_deref(), caller.as_ref()).await }))
        .into_response()
}

pub async fn get_memory(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match memory::read(&st, &id, caller_of(&headers).as_ref()).await {
        Some(m) => Json(json!(m)).into_response(),
        None => not_found("没有这条 memory"),
    }
}

pub async fn write_memory(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(w): Json<memory::Write>,
) -> Response {
    match memory::write(&st, w, caller_of(&headers).as_ref()).await {
        Ok(m) => Json(json!(m)).into_response(),
        Err(memory::WriteError::Invalid(e)) => bad(e),
        Err(memory::WriteError::Conflict { reason, existing }) => (
            StatusCode::CONFLICT,
            Json(memory::conflict_json(&reason, &existing)),
        )
            .into_response(),
    }
}

pub async fn archive_memory(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match memory::archive(&st, &id, caller_of(&headers).as_ref()).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad(e),
    }
}

pub async fn list_jobs(
    State(st): State<Shared>,
    headers: HeaderMap,
    Query(f): Query<jobs::Filter>,
) -> Response {
    match jobs::list(&st, &f, caller_of(&headers).as_ref()).await {
        Ok(list) => Json(json!({ "jobs": list })).into_response(),
        Err(e) => bad(e),
    }
}

pub async fn spawn_job(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<jobs::Spawn>,
) -> Response {
    let owner = jobs::owner_of(&st, &headers).await;
    match jobs::spawn(&st, req, &owner).await {
        Ok(j) => Json(json!(j)).into_response(),
        Err(e) => bad(e),
    }
}

pub async fn register_job(
    State(st): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<jobs::Register>,
) -> Response {
    let owner = jobs::owner_of(&st, &headers).await;
    match jobs::register(&st, req, &owner).await {
        Ok(j) => Json(json!(j)).into_response(),
        Err(e) => bad(e),
    }
}

async fn visible_job(st: &Shared, id: &str, headers: &HeaderMap) -> Result<jobs::Job, Response> {
    match jobs::get(st, id).await {
        Some(j) if jobs::visible_to(&j, caller_of(headers).as_ref()) => Ok(j),
        Some(_) => Err(fail(
            StatusCode::FORBIDDEN,
            "worker 只能看自己发起的后台任务",
        )),
        None => Err(not_found("没有这个后台任务")),
    }
}

pub async fn get_job(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    match visible_job(&st, &id, &headers).await {
        Ok(j) => Json(json!(j)).into_response(),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub tail: Option<usize>,
}

pub async fn job_log(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<LogQuery>,
) -> Response {
    let job = match visible_job(&st, &id, &headers).await {
        Ok(j) => j,
        Err(r) => return r,
    };
    let lines = q.tail.unwrap_or(200).clamp(1, jobs::MAX_TAIL);
    match jobs::log(&st, &job, lines).await {
        Ok(text) => Json(json!({
            "id": job.id,
            "path": job.log_file(),
            "status": job.status,
            "lines": lines,
            "log": text,
        }))
        .into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, e),
    }
}

pub async fn kill_job(
    State(st): State<Shared>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let killer = jobs::owner_of(&st, &headers).await;
    match jobs::kill(&st, &id, caller_of(&headers).as_ref(), &killer).await {
        Ok(j) => Json(json!(j)).into_response(),
        Err(e) => bad(e),
    }
}

pub fn caller_header(c: &Caller) -> (String, String) {
    (super::CALLER_HEADER.to_owned(), c.header_value())
}
