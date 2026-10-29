use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::backend::Hub;

const LAST_DIR_FILE: &str = "download-dir.txt";
const TICK: Duration = Duration::from_millis(150);

/// 推给页面的下载进度。页面拿 `received` 和时间算速度。
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DownloadEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        id: u64,
        name: String,
        path: String,
        total: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        id: u64,
        received: u64,
        total: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        id: u64,
        path: String,
        received: u64,
    },
    Failed {
        id: u64,
        error: String,
    },
    Cancelled {
        id: u64,
    },
}

#[derive(Default)]
pub struct Downloads {
    next: AtomicU64,
    running: Mutex<HashMap<u64, Arc<Notify>>>,
    saved: Mutex<HashMap<u64, PathBuf>>,
}

/// 只许下载本 hub 的 `/api/workspaces/{id}/download`，页面不能借桌面进程的会话去拉别的地址。
fn download_url(base: &str, raw: &str) -> Result<url::Url, String> {
    let bad = || "不是工作区的下载地址".to_owned();
    if !raw.starts_with("/api/workspaces/") {
        return Err(bad());
    }
    let base = url::Url::parse(base).map_err(|e| e.to_string())?;
    let url = base.join(raw).map_err(|_| bad())?;
    let segments: Vec<_> = url
        .path_segments()
        .map(Iterator::collect)
        .unwrap_or_default();
    let same_hub = url.origin() == base.origin();
    match segments.as_slice() {
        ["api", "workspaces", id, "download"] if same_hub && !id.is_empty() => Ok(url),
        _ => Err(bad()),
    }
}

/// 页面给的建议文件名只取最后一段，去掉路径分隔符和控制字符。
fn file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base
        .chars()
        .map(|c| if c == ':' || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() || clean == "." || clean == ".." {
        "download".to_owned()
    } else {
        clean.to_owned()
    }
}

fn last_dir() -> Option<PathBuf> {
    let file = crate::profile::data_dir().ok()?.join(LAST_DIR_FILE);
    let dir = PathBuf::from(std::fs::read_to_string(file).ok()?.trim());
    dir.is_dir().then_some(dir)
}

fn remember_dir(dir: &Path) {
    if let Ok(data) = crate::profile::data_dir() {
        let _ = std::fs::write(data.join(LAST_DIR_FILE), dir.to_string_lossy().as_bytes());
    }
}

fn default_dir() -> Option<PathBuf> {
    last_dir().or_else(|| {
        directories::UserDirs::new().and_then(|d| d.download_dir().map(Path::to_path_buf))
    })
}

/// 下载中的内容先写到同目录的隐藏临时文件，完成后再改名，
/// 取消或失败时不会留下半截文件，也不会提前覆盖用户选中的同名旧文件。
fn partial_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.with_file_name(format!(".{name}.blazar-download"))
}

/// 给页面看的路径，家目录缩写成 ~。
fn shown(path: &Path) -> String {
    let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf());
    match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn mb(n: u64) -> String {
    format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
}

fn cut_off(received: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => format!("连接断了，只收到 {} / {}", mb(received), mb(total)),
        None => format!("连接断了，或节点上打包失败（已收到 {}）", mb(received)),
    }
}

fn refused(status: reqwest::StatusCode, body: &str) -> String {
    let body = body.trim();
    match status.as_u16() {
        413 => format!("文件太大：{body}"),
        400 if body.is_empty() => "路径越界".to_owned(),
        404 if body.is_empty() => "找不到这个文件".to_owned(),
        401 | 403 => "hub 拒绝了这次下载，请重新打开 Blazar 再试".to_owned(),
        _ if body.is_empty() => format!("hub 返回 HTTP {status}"),
        _ => body.to_owned(),
    }
}

/// 原生「存储为」面板：默认文件名是原文件名，默认目录是上次选的目录，第一次是「下载」。
async fn ask_where(window: &WebviewWindow, name: &str) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("存储为")
        .set_file_name(name)
        .set_parent(window);
    if let Some(dir) = default_dir() {
        dialog = dialog.set_directory(dir);
    }
    Some(dialog.save_file().await?.path().to_path_buf())
}

/// 开发实例给界面驱动用：设了 BLAZAR_DEV_DOWNLOAD_DIR 就不弹面板，直接存进那个目录。
fn dev_target(name: &str) -> Option<PathBuf> {
    if !crate::dev_instance() {
        return None;
    }
    let dir = PathBuf::from(std::env::var_os("BLAZAR_DEV_DOWNLOAD_DIR")?);
    dir.is_dir().then(|| dir.join(name))
}

/// 弹出原生「存储为」面板；用户取消就返回 None，什么都不下载。
/// 选好位置后在后台流式下载，进度经 `on_event` 推给页面，命令本身立即返回下载 id。
#[tauri::command]
pub async fn save_download(
    window: WebviewWindow,
    hub: State<'_, Arc<Hub>>,
    downloads: State<'_, Arc<Downloads>>,
    url: String,
    suggested_name: String,
    on_event: Channel<DownloadEvent>,
) -> Result<Option<u64>, String> {
    let url = download_url(hub.url(), &url)?;
    let name = file_name(&suggested_name);
    let dest = match dev_target(&name) {
        Some(dest) => dest,
        None => match ask_where(&window, &name).await {
            Some(dest) => dest,
            None => return Ok(None),
        },
    };
    if let Some(dir) = dest.parent() {
        remember_dir(dir);
    }
    let id = downloads.next.fetch_add(1, Ordering::Relaxed) + 1;
    let cancel = Arc::new(Notify::new());
    downloads.running.lock().unwrap().insert(id, cancel.clone());
    let downloads = downloads.inner().clone();
    let base = hub.url().to_owned();
    tokio::spawn(async move {
        let saved = dest
            .file_name()
            .map_or_else(|| name.clone(), |n| n.to_string_lossy().into_owned());
        let report = |p: DownloadEvent| {
            let _ = on_event.send(p);
        };
        let outcome = tokio::select! {
            r = fetch(&base, url, &dest, id, saved, &report) => Some(r),
            () = cancel.notified() => None,
        };
        downloads.running.lock().unwrap().remove(&id);
        match outcome {
            Some(Ok(received)) => {
                tracing::info!(path = %dest.display(), received, "下载完成");
                downloads.saved.lock().unwrap().insert(id, dest.clone());
                report(DownloadEvent::Done {
                    id,
                    path: shown(&dest),
                    received,
                });
            }
            Some(Err(error)) => {
                let _ = tokio::fs::remove_file(partial_path(&dest)).await;
                tracing::warn!(path = %dest.display(), "下载失败：{error}");
                report(DownloadEvent::Failed { id, error });
            }
            None => {
                let _ = tokio::fs::remove_file(partial_path(&dest)).await;
                tracing::info!(path = %dest.display(), "下载已取消");
                report(DownloadEvent::Cancelled { id });
            }
        }
    });
    Ok(Some(id))
}

async fn fetch(
    base: &str,
    url: url::Url,
    dest: &Path,
    id: u64,
    name: String,
    report: &impl Fn(DownloadEvent),
) -> Result<u64, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let cookie = crate::remote::session_cookie(&client, base).await?;
    let mut response = client
        .get(url)
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
        .map_err(|e| format!("连不上 hub：{e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(refused(status, &body));
    }
    let total = response.content_length();
    report(DownloadEvent::Started {
        id,
        name,
        path: shown(dest),
        total,
    });
    let partial = partial_path(dest);
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| format!("写不了 {}：{e}", dest.display()))?;
    let write_failed = |e: std::io::Error| format!("写入 {} 失败：{e}", dest.display());
    let mut received = 0u64;
    let mut last = Instant::now();
    loop {
        let chunk = response.chunk().await.map_err(|e| {
            tracing::warn!("下载中断：{e}");
            cut_off(received, total)
        })?;
        let Some(chunk) = chunk else { break };
        file.write_all(&chunk).await.map_err(write_failed)?;
        received += chunk.len() as u64;
        if last.elapsed() >= TICK {
            last = Instant::now();
            report(DownloadEvent::Progress {
                id,
                received,
                total,
            });
        }
    }
    if total.is_some_and(|t| t != received) {
        return Err(cut_off(received, total));
    }
    file.flush().await.map_err(write_failed)?;
    file.sync_all().await.map_err(write_failed)?;
    drop(file);
    tokio::fs::rename(&partial, dest)
        .await
        .map_err(write_failed)?;
    Ok(received)
}

#[tauri::command]
pub fn cancel_download(downloads: State<'_, Arc<Downloads>>, id: u64) {
    if let Some(cancel) = downloads.running.lock().unwrap().get(&id) {
        cancel.notify_one();
    }
}

/// 只能显示这次会话里由 save_download 存下来的文件。
#[tauri::command]
pub fn reveal_download(downloads: State<'_, Arc<Downloads>>, id: u64) -> Result<(), String> {
    let path = downloads
        .saved
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| "找不到这次下载".to_owned())?;
    if !path.exists() {
        return Err("文件已经被移走或删除了".into());
    }
    crate::reveal_download(&path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_workspace_downloads_on_the_same_hub_are_fetched() {
        let base = "http://127.0.0.1:7000";
        let ok = download_url(base, "/api/workspaces/w1/download?path=a%2Fb.zip&dir=true").unwrap();
        assert_eq!(
            ok.as_str(),
            "http://127.0.0.1:7000/api/workspaces/w1/download?path=a%2Fb.zip&dir=true"
        );
        for bad in [
            "http://evil.example/api/workspaces/w1/download",
            "//evil.example/api/workspaces/w1/download",
            "/api/workspaces/w1/files?path=a",
            "/api/workspaces//download",
            "/api/workspaces/w1/download/../../export",
            "/api/workspaces/w1/x/download",
            "/api/export",
            "",
        ] {
            assert!(download_url(base, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn suggested_names_stay_a_single_file_name() {
        assert_eq!(file_name("报告 \"v2\".pdf"), "报告 \"v2\".pdf");
        assert_eq!(file_name("a/b/../c.tar.gz"), "c.tar.gz");
        assert_eq!(file_name("..\\..\\x"), "x");
        assert_eq!(file_name("a:b\u{7}"), "a_b_");
        assert_eq!(file_name(".."), "download");
        assert_eq!(file_name("  "), "download");
        assert_eq!(file_name(".env"), ".env");
    }

    #[test]
    fn broken_transfers_say_how_far_they_got() {
        assert_eq!(
            cut_off(3 * 1024 * 1024, Some(10 * 1024 * 1024)),
            "连接断了，只收到 3.0 MB / 10.0 MB"
        );
        assert_eq!(
            cut_off(1024 * 1024, None),
            "连接断了，或节点上打包失败（已收到 1.0 MB）"
        );
    }

    #[test]
    fn partial_files_sit_next_to_the_target_and_stay_hidden() {
        assert_eq!(
            partial_path(Path::new("/Users/me/Downloads/a.zip")),
            Path::new("/Users/me/Downloads/.a.zip.blazar-download")
        );
    }
}
