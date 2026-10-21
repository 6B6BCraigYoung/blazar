use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::global_login;
use crate::api::Shared;
use crate::state::ServerEvent;

use super::{Account, PROVIDERS, all, fail, get, mode_key, mode_of, root};

#[derive(Deserialize)]
pub struct Mode {
    pub provider: String,
    pub mode: String,
}

pub async fn set_mode(State(st): State<Shared>, Json(m): Json<Mode>) -> Response {
    if !PROVIDERS.contains(&m.provider.as_str()) {
        return fail(
            StatusCode::BAD_REQUEST,
            format!("不支持的运行时 {}", m.provider),
        );
    }
    let mode = m.mode.trim();
    if !matches!(mode, "" | "auto") {
        match get(&st, mode).await {
            Some(a) if a.provider == m.provider => {}
            _ => return fail(StatusCode::BAD_REQUEST, "没有这个账号"),
        }
    }
    if let Err(e) = crate::office::kv_put(&st, &mode_key(&m.provider), &json!(mode)).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    let global = if global_enabled(&st).await && mode != "auto" {
        Some(sync_global(&st, &m.provider, mode).await)
    } else {
        None
    };
    st.emit(ServerEvent::AccountsChanged);
    match global {
        Some(Err(e)) => {
            Json(json!({ "ok": true, "global": false, "global_error": e })).into_response()
        }
        Some(Ok(())) => Json(json!({ "ok": true, "global": true })).into_response(),
        None => Json(json!({ "ok": true })).into_response(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuChoice {
    pub mode: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuProvider {
    pub provider: &'static str,
    pub current: String,
    pub choices: Vec<MenuChoice>,
}

pub async fn menu_state(st: &Shared) -> (bool, Vec<MenuProvider>) {
    let mut providers = Vec::new();
    for p in PROVIDERS {
        let mut choices = vec![MenuChoice {
            mode: "auto".into(),
            label: "自动轮换".into(),
        }];
        for a in all(st, Some(p)).await.into_iter().filter(|a| !a.disabled) {
            let label = match a
                .email
                .as_deref()
                .filter(|e| !e.is_empty() && *e != a.label)
            {
                Some(email) => format!("{} · {email}", a.label),
                None => a.label.clone(),
            };
            choices.push(MenuChoice {
                mode: if a.builtin { String::new() } else { a.id },
                label,
            });
        }
        if !choices.iter().any(|c| c.mode.is_empty()) {
            choices.insert(
                1,
                MenuChoice {
                    mode: String::new(),
                    label: "本机登录".into(),
                },
            );
        }
        providers.push(MenuProvider {
            provider: p,
            current: mode_of(st, p).await,
            choices,
        });
    }
    (global_enabled(st).await, providers)
}

async fn response_error(r: Response) -> Result<(), String> {
    if r.status().is_success() {
        return Ok(());
    }
    let body = axum::body::to_bytes(r.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    let v: Value = serde_json::from_slice(&body).unwrap_or_default();
    Err(v["error"].as_str().map_or_else(
        || String::from_utf8_lossy(&body).into_owned(),
        str::to_owned,
    ))
}

pub async fn choose(st: &Shared, provider: &str, mode: &str) -> Result<(), String> {
    let r = set_mode(
        State(st.clone()),
        Json(Mode {
            provider: provider.to_owned(),
            mode: mode.to_owned(),
        }),
    )
    .await;
    response_error(r).await
}

pub async fn set_global(st: &Shared, enabled: bool) -> Result<(), String> {
    response_error(global_put(State(st.clone()), Json(GlobalBody { enabled })).await).await
}

const GLOBAL_KEY: &str = "accounts.global_sync";

fn global_key(provider: &str) -> String {
    format!("accounts.global.{provider}")
}

async fn global_enabled(st: &Shared) -> bool {
    crate::office::kv_get(st, GLOBAL_KEY).await.as_bool() == Some(true)
}

async fn global_current(st: &Shared, provider: &str) -> Option<Account> {
    let id = crate::office::kv_get(st, &global_key(provider))
        .await
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if id.is_empty() {
        return None;
    }
    get(st, &id).await.filter(|a| !a.builtin)
}

async fn sync_global(st: &Shared, provider: &str, mode: &str) -> Result<(), String> {
    let target = if mode.is_empty() {
        None
    } else {
        get(st, mode).await.filter(|a| !a.builtin)
    };
    let prev = global_current(st, provider).await;
    if prev.as_ref().map(|a| &a.id) == target.as_ref().map(|a| &a.id) {
        return Ok(());
    }
    let backups = root(st).join("global-backup");
    let places = global_login::Places::from_env();
    let p2 = provider.to_owned();
    let (prev2, target2) = (prev.clone(), target.clone());
    tokio::task::spawn_blocking(move || match p2.as_str() {
        "claude" => {
            global_login::switch_claude(&places, &backups, prev2.as_ref(), target2.as_ref())
        }
        _ => global_login::switch_codex(&places, &backups, prev2.as_ref(), target2.as_ref()),
    })
    .await
    .map_err(|e| e.to_string())??;
    let id = target.map(|a| a.id).unwrap_or_default();
    crate::office::kv_put(st, &global_key(provider), &json!(id))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(target: "blazar::accounts", provider, account = %id, "全局登录已切换");
    Ok(())
}

pub async fn global_get(State(st): State<Shared>) -> Response {
    let mut cur = serde_json::Map::new();
    for p in PROVIDERS {
        cur.insert(
            (*p).into(),
            json!(global_current(&st, p).await.map(|a| a.id)),
        );
    }
    Json(json!({
        "enabled": global_enabled(&st).await,
        "current": cur,
        "keychain": cfg!(target_os = "macos"),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct GlobalBody {
    pub enabled: bool,
}

pub async fn global_put(State(st): State<Shared>, Json(b): Json<GlobalBody>) -> Response {
    let mut errors = Vec::new();
    for p in PROVIDERS {
        let mode = if b.enabled {
            mode_of(&st, p).await
        } else {
            String::new()
        };
        if mode == "auto" {
            continue;
        }
        if let Err(e) = sync_global(&st, p, &mode).await {
            errors.push(format!("{p}：{e}"));
        }
    }
    if errors.is_empty() || !b.enabled {
        let _ = crate::office::kv_put(&st, GLOBAL_KEY, &json!(b.enabled)).await;
    }
    st.emit(ServerEvent::AccountsChanged);
    if errors.is_empty() {
        Json(json!({ "ok": true })).into_response()
    } else {
        fail(StatusCode::CONFLICT, errors.join("；"))
    }
}
