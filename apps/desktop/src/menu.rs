use std::sync::Arc;

use blazar_hub::accounts::{self, MenuProvider};
use blazar_hub::state::{AppState, ServerEvent};
use tauri::image::Image;
use tauri::menu::{
    AboutMetadata, CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu,
};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-36.rgba");

const REPO: &str = "https://github.com/6B6BCraigYoung/blazar";

struct AccountMenu(Vec<Submenu<Wry>>);

pub fn install(
    app: &AppHandle,
    state: Arc<AppState>,
    rt: tokio::runtime::Handle,
) -> tauri::Result<()> {
    let account = Submenu::with_id(app, "account", "账号", true)?;
    let menu = Menu::with_items(
        app,
        &[
            &app_menu(app)?,
            &file_menu(app)?,
            &edit_menu(app)?,
            &view_menu(app)?,
            &go_menu(app)?,
            &account,
            &window_menu(app)?,
            &help_menu(app)?,
        ],
    )?;
    app.set_menu(menu)?;
    let tray_account = Submenu::with_id(app, "tray-account", "账号", true)?;
    match tray(app, &tray_account) {
        Ok(()) => tracing::info!(
            present = app.tray_by_id("blazar").is_some(),
            "菜单栏图标已创建"
        ),
        Err(e) => tracing::warn!("菜单栏图标创建失败：{e}"),
    }
    app.manage(AccountMenu(vec![account, tray_account]));

    let handle = app.clone();
    let st = state.clone();
    rt.spawn(async move {
        let mut bus = st.bus.subscribe();
        loop {
            let snapshot = accounts::menu_state(&st).await;
            let h = handle.clone();
            let _ = handle.run_on_main_thread(move || {
                if let Err(e) = fill_accounts(&h, &snapshot.0, &snapshot.1) {
                    tracing::warn!("账号菜单刷新失败：{e}");
                }
            });
            loop {
                match bus.recv().await {
                    Ok(ServerEvent::AccountsChanged) => break,
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                    Err(_) => return,
                }
            }
        }
    });

    app.on_menu_event(move |app, event| {
        let id = event.id().as_ref().to_owned();
        if let Some(rest) = id.strip_prefix("acct:") {
            let (provider, mode) = rest.split_once(':').unwrap_or((rest, ""));
            let (provider, mode) = (provider.to_owned(), mode.to_owned());
            let (st, app) = (state.clone(), app.clone());
            rt.spawn(async move {
                if let Err(e) = accounts::choose(&st, &provider, &mode).await {
                    toast(&app, &format!("切换账号失败：{e}"));
                }
                st.emit(ServerEvent::AccountsChanged);
            });
        } else if id == "acct-global" {
            let (st, app) = (state.clone(), app.clone());
            rt.spawn(async move {
                let (enabled, _) = accounts::menu_state(&st).await;
                if let Err(e) = accounts::set_global(&st, !enabled).await {
                    toast(&app, &format!("同步全局登录失败：{e}"));
                }
                st.emit(ServerEvent::AccountsChanged);
            });
        } else if id == "quit" {
            crate::QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
            app.exit(0);
        } else if id == "show" {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        } else if id == "help-repo" {
            if let Ok(url) = REPO.parse() {
                crate::open_in_browser(&url);
            }
        } else {
            dispatch(app, &id);
        }
    });
    Ok(())
}

fn tray(app: &AppHandle, account: &Submenu<Wry>) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &item(app, "show", "打开 Blazar", None)?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "new-workspace", "新建工作区…", None)?,
            &item(app, "act:newchat", "新对话", None)?,
            &item(app, "act:palette", "命令面板", None)?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "go:/inbox", "收件箱", None)?,
            &item(app, "go:/nodes", "机器与组网", None)?,
            &item(app, "go:/usage", "用量", None)?,
            &PredefinedMenuItem::separator(app)?,
            account,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "go:/settings", "设置…", None)?,
            &item(app, "quit", "退出 Blazar", Some("CmdOrCtrl+Q"))?,
        ],
    )?;
    TrayIconBuilder::with_id("blazar")
        .icon(Image::new(TRAY_ICON, 36, 36))
        .icon_as_template(true)
        .tooltip("Blazar")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;
    Ok(())
}

fn dispatch(app: &AppHandle, id: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let detail = serde_json::to_string(id).unwrap_or_default();
        let _ = w.eval(format!(
            "window.dispatchEvent(new CustomEvent('blazar-menu', {{ detail: {detail} }}))"
        ));
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn toast(app: &AppHandle, message: &str) {
    dispatch(app, &format!("toast:{message}"));
}

fn item(
    app: &AppHandle,
    id: &str,
    text: &str,
    accel: Option<&str>,
) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, id, text, true, accel)
}

fn app_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let about = AboutMetadata {
        name: Some("Blazar".into()),
        version: Some(app.package_info().version.to_string()),
        website: Some(REPO.into()),
        ..Default::default()
    };
    Submenu::with_items(
        app,
        "Blazar",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("关于 Blazar"), Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "go:/settings", "设置…", Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some("服务"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("隐藏 Blazar"))?,
            &PredefinedMenuItem::hide_others(app, Some("隐藏其他"))?,
            &PredefinedMenuItem::show_all(app, Some("全部显示"))?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "quit", "退出 Blazar", None)?,
        ],
    )
}

fn file_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "文件",
        true,
        &[
            &item(app, "new-workspace", "新建工作区…", Some("CmdOrCtrl+N"))?,
            &item(app, "act:newchat", "新对话", Some("CmdOrCtrl+Shift+N"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some("关闭窗口"))?,
        ],
    )
}

fn edit_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("撤销"))?,
            &PredefinedMenuItem::redo(app, Some("重做"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("剪切"))?,
            &PredefinedMenuItem::copy(app, Some("拷贝"))?,
            &PredefinedMenuItem::paste(app, Some("粘贴"))?,
            &PredefinedMenuItem::select_all(app, Some("全选"))?,
        ],
    )
}

fn view_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "视图",
        true,
        &[
            &item(app, "act:palette", "命令面板", Some("CmdOrCtrl+K"))?,
            &item(app, "act:side", "侧边栏", Some("CmdOrCtrl+Backslash"))?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "act:explorer", "资源管理器", Some("CmdOrCtrl+B"))?,
            &item(app, "act:chat", "对话面板", Some("CmdOrCtrl+Alt+B"))?,
            &item(app, "act:panel", "底部面板", Some("CmdOrCtrl+J"))?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "act:diff", "变更", Some("CmdOrCtrl+Shift+D"))?,
            &item(app, "act:git", "Git", Some("CmdOrCtrl+Shift+G"))?,
            &item(app, "act:preview", "预览", Some("CmdOrCtrl+Shift+P"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, Some("进入全屏幕"))?,
        ],
    )
}

fn go_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "前往",
        true,
        &[
            &item(app, "go:/inbox", "收件箱", None)?,
            &item(app, "go:/", "全部工作区", None)?,
            &item(app, "go:/tasks", "任务", None)?,
            &item(app, "go:/autopilots", "自动化", None)?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "go:/runtimes", "运行时", None)?,
            &item(app, "go:/agents", "智能体", None)?,
            &item(app, "go:/skills", "SKILLs", None)?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "go:/apps", "接入的软件", None)?,
            &item(app, "go:/nodes", "机器与组网", None)?,
            &item(app, "go:/usage", "用量", None)?,
        ],
    )
}

fn window_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let menu = Submenu::with_items(
        app,
        "窗口",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("最小化"))?,
            &PredefinedMenuItem::maximize(app, Some("缩放"))?,
        ],
    )?;
    #[cfg(target_os = "macos")]
    menu.set_as_windows_menu_for_nsapp()?;
    Ok(menu)
}

fn help_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let menu = Submenu::with_items(
        app,
        "帮助",
        true,
        &[
            &item(app, "go:/settings?s=shortcuts", "键盘快捷键", None)?,
            &item(app, "help-repo", "Blazar 项目主页", None)?,
        ],
    )?;
    #[cfg(target_os = "macos")]
    menu.set_as_help_menu_for_nsapp()?;
    Ok(menu)
}

fn runtime_name(provider: &str) -> &str {
    match provider {
        "claude" => "Claude Code",
        "codex" => "Codex",
        p => p,
    }
}

fn fill_accounts(app: &AppHandle, global: &bool, providers: &[MenuProvider]) -> tauri::Result<()> {
    for menu in &app.state::<AccountMenu>().0 {
        fill_account_menu(app, menu, *global, providers)?;
    }
    Ok(())
}

fn fill_account_menu(
    app: &AppHandle,
    menu: &Submenu<Wry>,
    global: bool,
    providers: &[MenuProvider],
) -> tauri::Result<()> {
    for old in menu.items()? {
        match old {
            MenuItemKind::MenuItem(i) => menu.remove(&i)?,
            MenuItemKind::Check(i) => menu.remove(&i)?,
            MenuItemKind::Submenu(i) => menu.remove(&i)?,
            MenuItemKind::Predefined(i) => menu.remove(&i)?,
            MenuItemKind::Icon(i) => menu.remove(&i)?,
        }
    }
    for p in providers {
        let sub = Submenu::new(app, runtime_name(p.provider), true)?;
        for c in &p.choices {
            sub.append(&CheckMenuItem::with_id(
                app,
                format!("acct:{}:{}", p.provider, c.mode),
                &c.label,
                true,
                c.mode == p.current,
                None::<&str>,
            )?)?;
        }
        menu.append(&sub)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&CheckMenuItem::with_id(
        app,
        "acct-global",
        "同步为全局登录",
        true,
        global,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&item(app, "go:/runtimes", "管理账号…", None)?)?;
    menu.append(&item(app, "go:/usage", "用量…", None)?)?;
    Ok(())
}
