#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use crate::backend::Hub;
use crate::remote::{Phase, RemoteHub};

mod backend;
mod commands;
mod forward;
mod hub_mode;
mod menu;
mod profile;
mod remote;
mod snapshot;

const BUNDLED_PAGE: &str = "index.html";

static STARTUP_NOTICE: Mutex<Option<&'static str>> = Mutex::new(None);

fn dev_instance() -> bool {
    std::env::var("BLAZAR_DEV_INSTANCE").is_ok_and(|v| v == "1")
}

static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some(blazar_mcp::remote::SUBCOMMAND) {
        std::process::exit(mcp_remote(&argv));
    }

    if argv.get(1).map(String::as_str) == Some(blazar_hub::office::MCP_SUBCOMMAND) {
        let code = tokio::runtime::Runtime::new()
            .map(|rt| rt.block_on(blazar_hub::office::serve_mcp(&argv)).is_ok())
            .map_or(1, |ok| i32::from(!ok));
        std::process::exit(code);
    }
    init_logging();

    inherit_login_path();

    if let Err(err) = run() {
        eprintln!("启动失败: {err:#}");
        std::process::exit(1);
    }
}

fn init_logging() {
    use tracing_subscriber::fmt::writer::MakeWriterExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "blazar=info,blazar_desktop=info".into());
    let file = profile::data_dir().ok().and_then(|dir| {
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).ok()?;
        let path = logs.join("desktop.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 4 * 1024 * 1024) {
            let _ = std::fs::rename(&path, logs.join("desktop.log.1"));
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match file {
        Some(f) => builder
            .with_ansi(false)
            .with_writer(std::io::stderr.and(std::sync::Mutex::new(f)))
            .init(),
        None => builder.init(),
    }
}

fn run() -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("创建 tokio 运行时失败")?;
    let dir = profile::data_dir()?;
    let mode = hub_mode::HubMode::load(&dir);
    let url_override = std::env::var(hub_mode::URL_ENV).ok();
    let hub = Arc::new(match mode.target(url_override.as_deref()) {
        Some(target) => {
            tracing::info!("远程模式，连 {} 上的 hub", target.label());
            let _enter = rt.enter();
            Hub::Remote(Arc::new(
                RemoteHub::new(target, &dir.join(remote::PORTS_FILE))
                    .context("启动 SSH 隧道失败")?,
            ))
        }
        None => start_embedded(&rt)?,
    });

    let (badge_tx, badge_rx) = tokio::sync::oneshot::channel::<tauri::WebviewWindow>();
    rt.spawn(badge(hub.clone(), badge_rx));

    let rt_handle = rt.handle().clone();
    let setup_hub = hub.clone();
    let _guard = rt;

    for arg in std::env::args_os().skip(1) {
        if let Some(text) = read_invite_file(Path::new(&arg))
            && let Err(notice) = hub.accept_invite(text)
        {
            *STARTUP_NOTICE.lock().unwrap() = Some(notice);
        }
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            commands::hub_mode,
            commands::test_remote_hub,
            commands::set_hub_mode,
            commands::retry_hub,
            commands::forward_port,
            snapshot::snapshot_webview,
        ])
        .setup(move |app| {
            let hub = setup_hub;
            let geo = window_geometry::load();
            let dev = dev_instance();
            #[cfg(target_os = "macos")]
            if dev {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            let base = if dev { "Blazar Dev" } else { "Blazar" };
            let (url, title) = match hub.remote() {
                None => (
                    WebviewUrl::External(hub.url().parse().expect("内嵌 hub 地址应为合法 URL")),
                    base.to_owned(),
                ),
                Some(r) => (
                    WebviewUrl::App(BUNDLED_PAGE.into()),
                    format!("{base} — {}", r.target.label()),
                ),
            };
            tracing::info!(%title, "主窗口");
            let mut b = WebviewWindowBuilder::new(app, "main", url)
                .title(title)
                .focused(!dev)
                .inner_size(geo.map_or(1440.0, |g| g.w), geo.map_or(900.0, |g| g.h))
                .min_inner_size(900.0, 600.0)
                .disable_drag_drop_handler()
                .on_new_window(|url, _features| {
                    open_in_browser(&url);
                    tauri::webview::NewWindowResponse::Deny
                })
                .on_download(|_, event| {
                    if let tauri::webview::DownloadEvent::Finished {
                        path: Some(path),
                        success: true,
                        ..
                    } = event
                    {
                        reveal_download(&path);
                    }
                    true
                })
                .on_page_load(|webview, payload| {
                    if payload.event() == tauri::webview::PageLoadEvent::Finished {
                        tracing::info!(url = %payload.url(), "页面已加载");
                    }
                    if payload.event() == tauri::webview::PageLoadEvent::Finished
                        && payload.url().scheme() == "http"
                        && let Some(notice) = STARTUP_NOTICE.lock().unwrap().take()
                    {
                        let app = webview.app_handle().clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_secs(2));
                            menu::toast(&app, notice);
                        });
                    }
                });
            if let Some(g) = geo {
                b = b.position(g.x, g.y);
            }
            if dev {
                b = b.background_throttling(BackgroundThrottlingPolicy::Disabled);
            }
            let win = b.build()?;
            let _ = badge_tx.send(win.clone());
            let w2 = win.clone();
            let focus_hub = hub.clone();
            win.on_window_event(move |e| match e {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    window_geometry::save(&w2);
                    if !QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
                        api.prevent_close();
                        #[cfg(target_os = "macos")]
                        if w2.is_fullscreen().unwrap_or(false) {
                            let _ = w2.app_handle().hide();
                            return;
                        }
                        let _ = w2.hide();
                    }
                }
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                    window_geometry::save(&w2);
                }
                tauri::WindowEvent::Focused(true) => focus_hub.inbox_maybe_changed(),
                _ => {}
            });
            if let Some(r) = hub.remote() {
                rt_handle.spawn(watch_remote(r.clone(), app.handle().clone()));
                rt_handle.spawn(forward::keep_in_sync(r.clone()));
                rt_handle.spawn(quit_on_signal(app.handle().clone()));
            }
            let page = tauri::ipc::CapabilityBuilder::new("hub-page")
                .remote(format!("{}/*", hub.url()))
                .local(false)
                .window("main")
                .permission("hub-mode");
            let page = if hub.remote().is_some() {
                page.permission("remote-forward")
            } else {
                page
            };
            app.add_capability(if snapshot::enabled() {
                page.permission(snapshot::PERMISSION)
            } else {
                page
            })?;
            app.manage(hub.clone());
            menu::install(app.handle(), hub, rt_handle.clone(), !dev)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .context("Tauri 初始化失败")?;

    app.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = &event {
            QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if let tauri::RunEvent::Exit = &event {
            hub.shutdown();
            if RESTART.load(std::sync::atomic::Ordering::SeqCst) {
                restart();
            }
        }
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = &event
            && let Some(w) = handle.get_webview_window("main")
        {
            let _ = handle.show();
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = &event {
            let mut got = false;
            for u in urls {
                let Some(text) = u.to_file_path().ok().and_then(|p| read_invite_file(&p)) else {
                    continue;
                };
                match hub.accept_invite(text) {
                    Ok(()) => got = true,
                    Err(notice) => menu::toast(handle, notice),
                }
            }
            if got {
                notify_page(handle);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (handle, &event);
    });
    Ok(())
}

fn start_embedded(rt: &tokio::runtime::Runtime) -> Result<Hub> {
    let mut cfg = profile::hub_config()?;
    tracing::info!("数据目录: {}", cfg.db_path.display());

    let port_file = profile::data_dir()?.join("hub.port");
    let saved: u16 = std::fs::read_to_string(&port_file)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let free = saved != 0 && std::net::TcpListener::bind(("127.0.0.1", saved)).is_ok();
    cfg.bind = ([127, 0, 0, 1], if free { saved } else { 0 }).into();
    let (addr, state) = rt
        .block_on(blazar_hub::spawn(cfg))
        .context("启动内嵌 hub 失败")?;
    if addr.port() != saved {
        let _ = std::fs::write(&port_file, addr.port().to_string());
    }
    tracing::info!("内嵌 hub 已就绪: http://{addr}");
    Ok(Hub::Embedded {
        state,
        url: format!("http://{addr}"),
    })
}

async fn badge(hub: Arc<Hub>, window: tokio::sync::oneshot::Receiver<tauri::WebviewWindow>) {
    let Ok(win) = window.await else { return };
    let mut watch = hub.watch_inbox();
    let mut shown = -1;
    loop {
        match hub.unread().await {
            Ok(n) => {
                tracing::debug!(unread = n, "角标已刷新");
                if n != shown {
                    let _ = win.set_badge_count((n > 0).then_some(n));
                    shown = n;
                }
            }
            Err(e) => tracing::debug!("角标这次没刷新：{e}"),
        }
        if !watch.next().await {
            return;
        }
    }
}

fn bundled_url() -> tauri::Url {
    let origin = if cfg!(windows) {
        "http://tauri.localhost/"
    } else {
        "tauri://localhost/"
    };
    format!("{origin}{BUNDLED_PAGE}")
        .parse()
        .expect("随包页面地址应为合法 URL")
}

fn point_window(app: &tauri::AppHandle, url: &str) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let Ok(url) = url.parse::<tauri::Url>() else {
        return;
    };
    if win.url().is_ok_and(|current| {
        current.scheme() == url.scheme()
            && current.host_str() == url.host_str()
            && current.port() == url.port()
    }) {
        return;
    }
    if let Err(e) = win.navigate(url) {
        tracing::warn!("窗口换不了页面：{e}");
    }
}

async fn watch_remote(r: Arc<RemoteHub>, app: tauri::AppHandle) {
    loop {
        r.set_phase(Phase::Connecting);
        if r.wait_reachable().await {
            tracing::info!("远程 hub 已连上：{}", r.base);
            r.set_phase(Phase::Connected);
            point_window(&app, &r.base);
            if let Some(forwards) = &r.forwards {
                forwards.kick();
            }
            r.inbox_kick.notify_one();
            r.accounts_kick.notify_one();
            let mut misses = 0;
            while misses < 3 {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if r.reachable().await {
                    misses = 0;
                } else {
                    misses += 1;
                }
            }
            tracing::warn!("远程 hub 连不上了：{}", r.target.label());
        }
        let error = r.failure();
        tracing::warn!("连不上 {} 上的 hub：{error}", r.target.label());
        r.set_phase(Phase::Failed(error));
        point_window(&app, bundled_url().as_str());
        tokio::select! {
            () = r.retry.notified() => {}
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
        }
        r.kick();
    }
}

#[cfg(unix)]
async fn quit_on_signal(app: tauri::AppHandle) {
    use tokio::signal::unix::{SignalKind, signal};

    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    ) else {
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
        _ = hup.recv() => {}
    }
    tracing::info!("收到退出信号");
    QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
}

#[cfg(not(unix))]
async fn quit_on_signal(app: tauri::AppHandle) {
    let _ = app;
}

fn restart() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    match std::process::Command::new(exe)
        .env_remove(hub_mode::URL_ENV)
        .spawn()
    {
        Ok(_) => tracing::info!("应用已重启"),
        Err(e) => tracing::warn!("重启失败：{e}"),
    }
}

fn open_in_browser(url: &tauri::Url) {
    if !matches!(url.scheme(), "http" | "https" | "obsidian") {
        return;
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");

    match cmd.arg(url.as_str()).spawn() {
        Ok(_) => tracing::info!(host = url.host_str().unwrap_or(""), "外链交给系统浏览器"),
        Err(e) => tracing::warn!("打不开系统浏览器：{e}"),
    }
}

fn reveal_download(path: &Path) {
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let spawned = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    match spawned {
        Ok(_) => tracing::info!(path = %path.display(), "下载完成"),
        Err(e) => tracing::warn!("下载完成但打不开所在文件夹：{e}"),
    }
}

fn read_invite_file(path: &Path) -> Option<String> {
    use blazar_netmesh::invite::{INVITE_EXT, MAX_INVITE_BYTES};

    let is_invite = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(INVITE_EXT));
    if !is_invite {
        return None;
    }
    let too_big = std::fs::metadata(path).map_or(true, |m| m.len() > MAX_INVITE_BYTES as u64);
    if too_big {
        tracing::warn!("忽略 {}：不是有效的邀请文件", path.display());
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => {
            tracing::info!("收到邀请文件 {}", path.display());
            Some(text)
        }
        Err(e) => {
            tracing::warn!("读取邀请文件 {} 失败: {e}", path.display());
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn notify_page(handle: &tauri::AppHandle) {
    if let Some(w) = handle.get_webview_window("main") {
        let _ = w.eval("window.blazarInvite && window.blazarInvite()");
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(target_os = "macos")]
fn inherit_login_path() {
    use std::os::unix::process::CommandExt;

    const MARK: &str = "BLAZAR_LOGIN_PATH";
    if std::env::var_os(MARK).is_some() {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());

    let Ok(out) = std::process::Command::new(&shell)
        .args([
            "-ilc",
            "printf '__BLAZAR_PATH__%s__BLAZAR_PATH__' \"$PATH\"",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(path) = text
        .split("__BLAZAR_PATH__")
        .nth(1)
        .filter(|p| !p.is_empty())
    else {
        return;
    };
    let mut merged: Vec<&str> = path.split(':').collect();
    let current = std::env::var("PATH").unwrap_or_default();
    for p in current.split(':') {
        if !p.is_empty() && !merged.contains(&p) {
            merged.push(p);
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };

    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("PATH", merged.join(":"))
        .env(MARK, "1")
        .exec();
    tracing::warn!("带登录 PATH 重启失败，沿用当前 PATH: {err}");
}

#[cfg(not(target_os = "macos"))]
fn inherit_login_path() {}

fn mcp_remote(argv: &[String]) -> i32 {
    let Some(target) = blazar_mcp::remote::target_from_args(argv) else {
        eprintln!(
            "用法: {} --node <主机> --root <路径>",
            blazar_mcp::remote::SUBCOMMAND
        );
        return 2;
    };
    let Ok(rt) = tokio::runtime::Runtime::new() else {
        return 1;
    };
    match rt.block_on(blazar_mcp::remote::serve(target)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}

mod window_geometry {
    #[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
    pub struct Geo {
        pub x: f64,
        pub y: f64,
        pub w: f64,
        pub h: f64,
    }

    fn path() -> Option<std::path::PathBuf> {
        crate::profile::data_dir()
            .ok()
            .map(|d| d.join("window.json"))
    }

    pub fn load() -> Option<Geo> {
        let g: Geo = serde_json::from_str(&std::fs::read_to_string(path()?).ok()?).ok()?;

        (g.w >= 900.0 && g.h >= 600.0 && g.x > -200.0 && g.y > -200.0).then_some(g)
    }

    pub fn save(w: &tauri::WebviewWindow) {
        let (Ok(scale), Ok(pos), Ok(size)) = (w.scale_factor(), w.outer_position(), w.inner_size())
        else {
            return;
        };
        let g = Geo {
            x: f64::from(pos.x) / scale,
            y: f64::from(pos.y) / scale,
            w: f64::from(size.width) / scale,
            h: f64::from(size.height) / scale,
        };
        if let (Some(p), Ok(t)) = (path(), serde_json::to_string(&g)) {
            let _ = std::fs::write(p, t);
        }
    }
}
