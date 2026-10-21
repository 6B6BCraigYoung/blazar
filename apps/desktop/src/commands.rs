use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, State};

use crate::backend::Hub;
use crate::hub_mode::{HubMode, Target};
use crate::remote::{self, Phase};

fn saved_mode() -> HubMode {
    crate::profile::data_dir()
        .map(|dir| HubMode::load(&dir))
        .unwrap_or_default()
}

#[tauri::command]
pub fn hub_mode(hub: State<'_, Arc<Hub>>) -> Value {
    let saved = saved_mode();
    let Some(r) = hub.remote() else {
        return json!({
            "mode": "embedded",
            "phase": "connected",
            "connected": true,
            "saved": saved,
        });
    };
    let (ssh, port) = match &r.target {
        Target::Ssh { alias, port } => (Some(alias.as_str()), Some(*port)),
        Target::Direct(_) => (None, None),
    };
    let (phase, error) = match r.phase() {
        Phase::Connecting => ("connecting", String::new()),
        Phase::Connected => ("connected", String::new()),
        Phase::Failed(e) => ("failed", e),
    };
    json!({
        "mode": "remote",
        "ssh": ssh,
        "port": port,
        "label": r.target.label(),
        "phase": phase,
        "connected": phase == "connected",
        "error": error,
        "saved": saved,
    })
}

#[tauri::command]
pub async fn test_remote_hub(ssh: String, port: u16) -> Result<String, String> {
    let HubMode::Remote { ssh, port } = HubMode::remote(&ssh, port)? else {
        return Err("只能测试远程 hub".into());
    };
    remote::probe(&ssh, port).await?;
    Ok(format!("能连上 {ssh} 上的 hub"))
}

#[tauri::command]
pub fn set_hub_mode(
    app: AppHandle,
    mode: String,
    ssh: Option<String>,
    port: Option<u16>,
) -> Result<(), String> {
    let next = match mode.as_str() {
        "embedded" => HubMode::Embedded,
        "remote" => HubMode::remote(
            ssh.as_deref().unwrap_or_default(),
            port.unwrap_or(crate::hub_mode::DEFAULT_PORT),
        )?,
        other => return Err(format!("不认识的模式 {other}")),
    };
    switch(&app, &next)
}

pub fn switch(app: &AppHandle, next: &HubMode) -> Result<(), String> {
    let dir = crate::profile::data_dir().map_err(|e| e.to_string())?;
    next.save(&dir)?;
    tracing::info!(?next, "hub 模式已更改，重启应用");
    crate::RESTART.store(true, Ordering::SeqCst);
    crate::QUITTING.store(true, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn retry_hub(hub: State<'_, Arc<Hub>>) -> Result<(), String> {
    let Some(r) = hub.remote().cloned() else {
        return Ok(());
    };
    if r.phase() != Phase::Connected {
        r.set_phase(Phase::Connecting);
        r.retry.notify_one();
    }
    let wait = async {
        loop {
            let changed = r.phase_changed.notified();
            match r.phase() {
                Phase::Connecting => changed.await,
                Phase::Connected => return Ok(()),
                Phase::Failed(e) => return Err(e),
            }
        }
    };
    tokio::time::timeout(remote::READY_TIMEOUT + Duration::from_secs(5), wait)
        .await
        .unwrap_or_else(|_| Err(r.failure()))
}

#[tauri::command]
pub async fn forward_port(hub: State<'_, Arc<Hub>>, port: u16) -> Result<u16, String> {
    let Some(r) = hub.remote().cloned() else {
        return Err("只有远程模式需要转发端口".into());
    };
    r.forward(port).await
}
