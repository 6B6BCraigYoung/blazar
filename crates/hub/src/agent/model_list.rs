use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{ApiResult, Shared};

pub const RECHECK_AFTER: Duration = Duration::from_secs(60);

pub const REFETCH_AFTER: Duration = Duration::from_secs(30 * 60);

const FETCH_TIMEOUT: Duration = Duration::from_secs(45);

const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

const REQUEST_ID: &str = "blazar-models";

const VERSION_MARK: &str = "__BLAZAR_CLI_VERSION__";

#[derive(Clone, Debug)]
struct Entry {
    version: Option<String>,
    models: Vec<Value>,
    fetched_at: Instant,
    fetched_wall: String,
    checked_at: Instant,
}

static CACHE: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(Default::default);

static GATES: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Cached,
    Check,
    Fetch,
}

#[must_use]
pub fn step(cached: Option<(Duration, Duration)>, refresh: bool) -> Step {
    match cached {
        None => Step::Fetch,
        Some((age, _)) if refresh && age >= REFETCH_AFTER => Step::Fetch,
        Some((_, since_check)) if refresh || since_check >= RECHECK_AFTER => Step::Check,
        Some(_) => Step::Cached,
    }
}

#[must_use]
pub fn same_version(a: Option<&str>, b: Option<&str>) -> bool {
    match (
        a.and_then(crate::remote_cli::parse_version),
        b.and_then(crate::remote_cli::parse_version),
    ) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[must_use]
pub fn rows(list: &[Value]) -> Vec<Value> {
    list.iter()
        .map(|m| {
            let v = m["value"].as_str().unwrap_or_default();
            json!({
                "id": if v == "default" { "" } else { v },
                "label": m["displayName"].as_str().unwrap_or(v),
                "desc": m["description"].as_str().unwrap_or_default(),
                "efforts": m.get("supportedEffortLevels").cloned().unwrap_or_else(|| json!([])),
            })
        })
        .collect()
}

#[derive(Debug, Default, PartialEq)]
pub struct Probe {
    pub version: Option<String>,
    pub models: Option<Vec<Value>>,
}

#[must_use]
pub fn parse(stdout: &str) -> Probe {
    let mut out = Probe::default();
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix(VERSION_MARK) {
            out.version =
                crate::remote_cli::parse_version(v).map(|(a, b, c)| format!("{a}.{b}.{c}"));
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] != "control_response" {
            continue;
        }
        let r = v.get("response").cloned().unwrap_or(Value::Null);
        if r["request_id"] != REQUEST_ID {
            continue;
        }
        let resp = r.get("response").cloned().unwrap_or(r);
        out.models = Some(
            resp.get("models")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
    }
    out
}

#[derive(Debug, Default, Clone)]
pub struct RunEnv {
    pub vars: BTreeMap<String, String>,
    pub files: BTreeMap<String, String>,
    pub unset: Vec<String>,
    pub account: Option<String>,
}

#[must_use]
pub fn env_block(env: &RunEnv) -> String {
    let ok = |k: &str| blazar_transport::validate_env_key(k).is_ok();
    let mut s = String::new();
    for k in env.unset.iter().filter(|k| ok(k)) {
        s.push_str(&format!("unset {k}\n"));
    }
    for (k, v) in env.vars.iter().filter(|(k, _)| ok(k)) {
        s.push_str(&format!(
            "export {k}={}\n",
            blazar_transport::shell_quote(v)
        ));
    }
    for (k, p) in env.files.iter().filter(|(k, _)| ok(k)) {
        s.push_str(&format!(
            "export {k}=\"$(cat -- {} 2>/dev/null)\"\n",
            blazar_transport::shell_quote(p)
        ));
    }
    s
}

#[must_use]
pub fn script(program: &str) -> String {
    format!(
        r#"{prelude}
IFS= read -r B
eval "$(printf '%s' "$B" | base64 --decode 2>/dev/null)" 2>/dev/null
cd "$HOME" 2>/dev/null || cd /tmp
P={program}
printf '%s %s\n' '{VERSION_MARK}' "$("$P" --version 2>/dev/null | head -n 1)"
T=
command -v timeout >/dev/null 2>&1 && T="timeout 30"
exec $T "$P" -p --output-format stream-json --verbose --input-format stream-json --no-session-persistence 2>/dev/null
"#,
        prelude = blazar_transport::PATH_PRELUDE,
        program = blazar_transport::shell_quote(program),
    )
}

#[must_use]
pub fn payload(env: &RunEnv) -> String {
    let init = json!({
        "type": "control_request",
        "request_id": REQUEST_ID,
        "request": { "subtype": "initialize" },
    });
    format!(
        "{}\n{init}\n",
        base64::engine::general_purpose::STANDARD.encode(env_block(env))
    )
}

fn machine(node: &str) -> String {
    if node == "local" {
        "this computer".to_owned()
    } else {
        node.to_owned()
    }
}

async fn exec(
    st: &Shared,
    node: &str,
    script: String,
    stdin: Option<String>,
    limit: Duration,
) -> Result<String, String> {
    let mut spec = blazar_transport::ExecSpec::new("bash")
        .arg("-lc")
        .arg(script);
    if let Some(s) = stdin {
        spec = spec.stdin(s);
    }
    match tokio::time::timeout(limit, st.transport(node).exec(spec)).await {
        Ok(Ok(out)) => Ok(out.stdout),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {} s", limit.as_secs())),
    }
}

async fn version(st: &Shared, node: &str, program: &str) -> Option<String> {
    let script = format!(
        "{}\nprintf '%s %s\\n' '{VERSION_MARK}' \"$({} --version 2>/dev/null | head -n 1)\"",
        blazar_transport::PATH_PRELUDE,
        blazar_transport::shell_quote(program),
    );
    let out = exec(st, node, script, None, VERSION_TIMEOUT).await.ok()?;
    parse(&out).version
}

async fn fetch(
    st: &Shared,
    node: &str,
    program: &str,
    env: &RunEnv,
) -> Result<(Option<String>, Vec<Value>), (String, String)> {
    let out = exec(st, node, script(program), Some(payload(env)), FETCH_TIMEOUT)
        .await
        .map_err(|e| {
            (
                format!("Couldn't reach {} to list its models", machine(node)),
                e,
            )
        })?;
    let probe = parse(&out);
    match probe.models.filter(|m| !m.is_empty()) {
        Some(models) => Ok((probe.version, models)),
        None if probe.version.is_none() => Err((
            format!("Claude Code isn't installed on {}", machine(node)),
            "no version line".to_owned(),
        )),
        None => Err((
            format!("Claude Code on {} didn't list any models", machine(node)),
            "no models in the initialize answer".to_owned(),
        )),
    }
}

const INHERITED_API_ENV: [&str; 3] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
];

async fn config(st: &Shared, node: &str) -> (String, BTreeMap<String, String>) {
    let cfg = crate::api::effective_agent_config(st, node, "claude").await;
    let program = cfg
        .as_ref()
        .and_then(|c| c.program_path.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "claude".to_owned());
    (program, cfg.map(|c| c.custom_env).unwrap_or_default())
}

async fn prior_account(st: &Shared, ws: &str, thread: Option<&str>) -> Option<String> {
    sqlx::query_scalar(
        "SELECT account_id FROM sessions
         WHERE workspace_id = ?1 AND COALESCE(thread_id, id) = ?2 AND account_id IS NOT NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(ws)
    .bind(thread?)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
}

async fn env_for(
    st: &Shared,
    run_node: &str,
    custom: BTreeMap<String, String>,
    requested: Option<&str>,
    prior: Option<&str>,
) -> Result<RunEnv, String> {
    let chosen =
        crate::accounts::resolve(st, "claude", run_node, requested, prior, None, &[]).await?;
    let mut env = RunEnv {
        vars: custom,
        unset: INHERITED_API_ENV.iter().map(|k| (*k).to_owned()).collect(),
        ..RunEnv::default()
    };
    if let Some(c) = &chosen {
        env.account = Some(c.id.clone());
        if let Some((k, v)) = &c.env {
            env.vars.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if let Some((k, p)) = &c.token_file
            && !env.vars.contains_key(k)
        {
            env.files.insert(k.clone(), p.display().to_string());
        }
    }
    if run_node != "local" {
        for (k, v) in crate::proxy::node_net_env(st, run_node).await {
            env.vars.entry(k).or_insert(v);
        }
    }
    if let Some(c) = chosen.filter(|c| c.proxy) {
        let port = crate::proxy::ensure_tunnel(st, run_node).await?;
        let secret = crate::proxy::secret(st, &c.id).await?;
        env.vars.insert(
            "ANTHROPIC_BASE_URL".into(),
            format!("http://127.0.0.1:{port}"),
        );
        env.vars.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), secret);
        env.vars.remove("ANTHROPIC_API_KEY");
        env.vars.remove("ANTHROPIC_AUTH_TOKEN");
        crate::proxy::bypass_loopback(&mut env.vars);
    }
    Ok(env)
}

fn gate(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    GATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key.to_owned())
        .or_default()
        .clone()
}

fn cached(key: &str) -> Option<Entry> {
    CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(key)
        .cloned()
}

fn keep(key: &str, entry: Entry) {
    CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key.to_owned(), entry);
}

fn checked(key: &str) {
    if let Some(e) = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_mut(key)
    {
        e.checked_at = Instant::now();
    }
}

fn answer(node: &str, env: &RunEnv, entry: &Entry, error: Option<String>) -> Value {
    json!({
        "runtime": "claude",
        "source": "cli",
        "node": node,
        "account": env.account,
        "cli_version": entry.version,
        "fetched_at": entry.fetched_wall,
        "models": rows(&entry.models),
        "stale": error.is_some(),
        "error": error,
    })
}

fn failed(node: &str, account: Option<String>, error: &str) -> Value {
    json!({
        "runtime": "claude",
        "source": "cli",
        "node": node,
        "account": account,
        "models": [],
        "stale": false,
        "error": error,
    })
}

async fn note_version(st: &Shared, node: &str, version: &str) {
    if node == "local" {
        return;
    }
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(capabilities, '$.agents') FROM nodes WHERE name = ?1",
    )
    .bind(node)
    .fetch_optional(st.db.pool())
    .await
    .ok()
    .flatten()
    .flatten();
    let known = stored
        .and_then(|s| serde_json::from_str::<Vec<blazar_runtime::DiscoveredAgent>>(&s).ok())
        .and_then(|list| list.into_iter().find(|a| a.id == "claude"))
        .and_then(|a| a.version);
    if same_version(known.as_deref(), Some(version)) {
        return;
    }
    if let Ok(found) = blazar_runtime::discover(&st.transport(node)).await {
        crate::remote_cli::store_agents(st, node, &found).await;
        tracing::info!(
            target: "blazar::models",
            "{node} 上的 Claude Code 版本变了（{} → {version}），已重新扫描",
            known.as_deref().unwrap_or("未知")
        );
    }
}

async fn refetch(
    st: &Shared,
    node: &str,
    program: &str,
    env: &RunEnv,
    key: &str,
    before: Option<Entry>,
) -> Value {
    match fetch(st, node, program, env).await {
        Ok((version, models)) => {
            let now = Instant::now();
            let entry = Entry {
                version: version.clone(),
                models,
                fetched_at: now,
                fetched_wall: chrono::Utc::now().to_rfc3339(),
                checked_at: now,
            };
            keep(key, entry.clone());
            if let Some(v) = &version {
                note_version(st, node, v).await;
            }
            answer(node, env, &entry, None)
        }
        Err((shown, detail)) => {
            tracing::warn!(target: "blazar::models", "{node} 上取型号列表失败：{shown}（{detail}）");
            match before {
                Some(entry) => answer(node, env, &entry, Some(shown)),
                None => failed(node, env.account.clone(), &shown),
            }
        }
    }
}

pub async fn claude_for(
    st: &Shared,
    ws: &str,
    run_node: &str,
    thread: Option<&str>,
    account: Option<&str>,
    refresh: bool,
) -> Value {
    let prior = prior_account(st, ws, thread).await;
    let requested = account.filter(|a| !a.is_empty());
    let (program, custom) = config(st, run_node).await;
    let env = match env_for(st, run_node, custom, requested, prior.as_deref()).await {
        Ok(env) => env,
        Err(e) => return failed(run_node, None, &e),
    };
    let key = format!(
        "{run_node}\u{1f}{}\u{1f}{program}",
        env.account.clone().unwrap_or_default()
    );
    let held = gate(&key);
    let _held = held.lock().await;
    let before = cached(&key);
    let now = Instant::now();
    let plan = step(
        before.as_ref().map(|c| {
            (
                now.saturating_duration_since(c.fetched_at),
                now.saturating_duration_since(c.checked_at),
            )
        }),
        refresh,
    );
    match (plan, before) {
        (Step::Cached, Some(entry)) => answer(run_node, &env, &entry, None),
        (Step::Check, Some(entry)) => match version(st, run_node, &program).await {
            Some(v) if same_version(entry.version.as_deref(), Some(&v)) => {
                checked(&key);
                note_version(st, run_node, &v).await;
                answer(run_node, &env, &entry, None)
            }
            Some(_) => refetch(st, run_node, &program, &env, &key, Some(entry)).await,
            None => answer(
                run_node,
                &env,
                &entry,
                Some(format!(
                    "Couldn't check the Claude Code version on {}",
                    machine(run_node)
                )),
            ),
        },
        (_, before) => refetch(st, run_node, &program, &env, &key, before).await,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    thread: Option<String>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    brain: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    refresh: bool,
}

pub async fn workspace_models(
    State(st): State<Shared>,
    Path(ws): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let runtime = q
        .runtime
        .clone()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "claude".to_owned());
    if runtime != "claude" {
        return Ok(Json(crate::api::fixed_models(&runtime)));
    }
    let (node, _) = crate::api::locate(&st, &ws).await?;
    let run_node = if node != "local" && q.brain.as_deref() != Some("node") {
        "local".to_owned()
    } else {
        node
    };
    let thread = q.thread.as_deref().filter(|t| !t.is_empty());
    let account = match q.account.clone().filter(|a| !a.is_empty()) {
        Some(a) => Some(a),
        None => match q.profile.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => crate::agents::get(&st, p).await.and_then(|p| p.account),
            None => None,
        },
    };
    Ok(Json(
        claude_for(&st, &ws, &run_node, thread, account.as_deref(), q.refresh).await,
    ))
}

#[cfg(test)]
#[path = "model_list_tests.rs"]
mod tests;
