#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use blazar_hub::state::AppState;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

mod profile;

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blazar=info".into()),
        )
        .init();

    inherit_login_path();

    if let Err(err) = run() {
        eprintln!("启动失败: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("创建 tokio 运行时失败")?;
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

    let (badge_tx, badge_rx) = tokio::sync::oneshot::channel::<tauri::WebviewWindow>();
    let badge_state = state.clone();
    rt.spawn(async move {
        let Ok(win) = badge_rx.await else { return };
        let mut bus = badge_state.bus.subscribe();
        let mut shown = -1;
        loop {
            let n = blazar_hub::inbox::unread(&badge_state).await;
            if n != shown {
                let _ = win.set_badge_count((n > 0).then_some(n));
                shown = n;
            }

            loop {
                match tokio::time::timeout(std::time::Duration::from_secs(60), bus.recv()).await {
                    Ok(Ok(blazar_hub::state::ServerEvent::InboxChanged)) | Ok(Err(_)) | Err(_) => {
                        break;
                    }
                    Ok(Ok(_)) => {}
                }
            }
        }
    });

    let _guard = rt;

    for arg in std::env::args_os().skip(1) {
        accept_invite_file(&state, Path::new(&arg));
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![hub_address, data_dir])
        .setup(move |app| {
            let url = format!("http://{addr}")
                .parse()
                .expect("内嵌 hub 地址应为合法 URL");

            let geo = window_geometry::load();
            let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("Blazar")
                .inner_size(geo.map_or(1440.0, |g| g.w), geo.map_or(900.0, |g| g.h))
                .min_inner_size(900.0, 600.0)
                .disable_drag_drop_handler()
                .on_new_window(|url, _features| {
                    open_in_browser(&url);
                    tauri::webview::NewWindowResponse::Deny
                });
            if let Some(g) = geo {
                b = b.position(g.x, g.y);
            }
            let win = b.build()?;
            let _ = badge_tx.send(win.clone());
            let w2 = win.clone();
            win.on_window_event(move |e| {
                if matches!(
                    e,
                    tauri::WindowEvent::CloseRequested { .. }
                        | tauri::WindowEvent::Moved(_)
                        | tauri::WindowEvent::Resized(_)
                ) {
                    window_geometry::save(&w2);
                }
            });
            app.manage(HubAddr(addr));
            Ok(())
        })
        .build(tauri::generate_context!())
        .context("Tauri 初始化失败")?;

    app.run(move |handle, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = &event {
            let mut got = false;
            for u in urls {
                if let Ok(p) = u.to_file_path() {
                    got |= accept_invite_file(&state, &p);
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

fn accept_invite_file(state: &Arc<AppState>, path: &Path) -> bool {
    use blazar_netmesh::invite::{INVITE_EXT, MAX_INVITE_BYTES};

    let is_invite = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(INVITE_EXT));
    if !is_invite {
        return false;
    }
    let too_big = std::fs::metadata(path).map_or(true, |m| m.len() > MAX_INVITE_BYTES as u64);
    if too_big {
        tracing::warn!("忽略 {}：不是有效的邀请文件", path.display());
        return false;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => {
            tracing::info!("收到邀请文件 {}", path.display());
            state.mesh_ctx.set_pending_invite(text);
            true
        }
        Err(e) => {
            tracing::warn!("读取邀请文件 {} 失败: {e}", path.display());
            false
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

struct HubAddr(SocketAddr);

#[tauri::command]
fn hub_address(state: tauri::State<'_, HubAddr>) -> String {
    format!("http://{}", state.0)
}

#[tauri::command]
fn data_dir() -> String {
    profile::data_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}
