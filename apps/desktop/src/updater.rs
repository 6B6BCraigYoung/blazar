use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use tauri::{AppHandle, Url};
use tauri_plugin_updater::{Updater, UpdaterExt};

const PUBKEY: Option<&str> = option_env!("BLAZAR_UPDATER_PUBKEY");
const REPO: Option<&str> = option_env!("BLAZAR_RELEASE_REPO");

fn dev(name: &str) -> Option<String> {
    std::env::var_os("BLAZAR_DEV_INSTANCE")?;
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn source() -> Option<(String, Url)> {
    let pubkey = dev("BLAZAR_DEV_UPDATER_PUBKEY")
        .or_else(|| PUBKEY.map(str::to_owned))
        .filter(|k| !k.is_empty())?;
    let url = dev("BLAZAR_DEV_UPDATER_URL").or_else(|| {
        REPO.map(|r| format!("https://github.com/{r}/releases/latest/download/latest.json"))
    })?;
    Some((pubkey, url.parse().ok()?))
}

fn updater(app: &AppHandle) -> Result<Option<Updater>, String> {
    let Some((pubkey, url)) = source() else {
        return Ok(None);
    };
    app.updater_builder()
        .pubkey(pubkey)
        .endpoints(vec![url])
        .and_then(|b| b.build())
        .map(Some)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_ready(app: AppHandle) -> Result<Value, String> {
    let Some(u) = updater(&app)? else {
        return Ok(json!({ "ready": false }));
    };
    let found = u.check().await.map_err(|e| e.to_string())?;
    Ok(json!({
        "ready": found.is_some(),
        "version": found.map(|f| f.version),
    }))
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let Some(u) = updater(&app)? else {
        return Err("这个版本不能在应用内更新，请下载安装".into());
    };
    let Some(update) = u.check().await.map_err(|e| {
        tracing::warn!("检查更新失败：{e}");
        "连不上更新服务器，稍后再试".to_owned()
    })?
    else {
        return Err("已经是最新版本".into());
    };
    tracing::info!(version = %update.version, "下载并安装更新");
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| {
            tracing::warn!("更新没装上：{e}");
            "更新没装上，请下载安装".to_owned()
        })?;
    tracing::info!("更新已装好，重启");
    crate::RESTART.store(true, Ordering::SeqCst);
    crate::QUITTING.store(true, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}
