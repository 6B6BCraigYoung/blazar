use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate};
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use crate::app_state::use_app;
use crate::components::icon;
use crate::components::toast::toast;
use crate::palette_match::score;
use crate::{api, fmt, storage};

use super::modal::Modal;

const RECENT_KEY: &str = "blazar.palette.recent";
const RECENT_MAX: usize = 5;

const PAGES: &[(&str, &str, &str, &str, Option<&str>)] = &[
    (
        "中控台",
        "/",
        icon::CONSOLE,
        "console zkt zhongkongtai 对话 chats 助手 assistant 聊天 首页",
        Some("console"),
    ),
    (
        "全部消息",
        "/?inbox=all",
        icon::INBOX,
        "inbox sjx shoujianxiang 收件箱 通知 消息",
        Some("inbox"),
    ),
    (
        "全部对话",
        "/chats",
        icon::CHAT,
        "chats dh duihua 对话列表",
        None,
    ),
    (
        "工作区",
        "/workspaces",
        icon::FOLDER,
        "workspaces home gzq gongzuoqu 全部",
        Some("workspaces"),
    ),
    ("bot", "/bots", icon::BOT, "bots 机器人", None),
    (
        "任务",
        "/tasks",
        icon::TASKS,
        "tasks rw renwu 看板",
        Some("tasks"),
    ),
    (
        "自动化",
        "/autopilots",
        icon::CLOCK,
        "autopilots zdh zidonghua 定时 webhook",
        None,
    ),
    (
        "后台任务",
        "/jobs",
        icon::TERMINAL,
        "jobs htrw houtairenwu 进程 训练",
        None,
    ),
    (
        "角色档案",
        "/agents",
        icon::ID_CARD,
        "agents jsda juesedangan 智能体",
        None,
    ),
    (
        "技能",
        "/skills",
        icon::WAND,
        "skills jn jineng mcp 技能库",
        None,
    ),
    (
        "运行时",
        "/runtimes",
        icon::ACTIVITY,
        "runtimes yxs yunxingshi 账号 claude codex",
        None,
    ),
    (
        "用量",
        "/usage",
        icon::CHART,
        "usage yl yongliang 费用 token",
        None,
    ),
    (
        "接入的软件",
        "/apps",
        icon::APPS,
        "apps jrdrj ruanjian 飞书 lark github obsidian 仿真",
        None,
    ),
    (
        "机器与组网",
        "/nodes",
        icon::NODES,
        "nodes mesh jq jiqi zuwang 节点 ssh",
        None,
    ),
    (
        "设置",
        "/settings",
        icon::GEAR,
        "settings sz shezhi 偏好 快捷键 外观",
        Some("settings"),
    ),
];

/// Where a command means something: anywhere, inside a workspace, or where there is a
/// message box to type into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Any,
    Workspace,
    Composer,
}

struct Cmd {
    title: &'static str,
    act: &'static str,
    icon: &'static str,
    keys: &'static str,
    key: Option<&'static str>,
    scope: Scope,
}

const fn cmd(
    title: &'static str,
    act: &'static str,
    icon: &'static str,
    keys: &'static str,
    key: Option<&'static str>,
    scope: Scope,
) -> Cmd {
    Cmd {
        title,
        act,
        icon,
        keys,
        key,
        scope,
    }
}

const COMMANDS: &[Cmd] = &[
    cmd(
        "新对话",
        "newchat",
        icon::CHAT,
        "new chat xdh xinduihua 对话",
        Some("newchat"),
        Scope::Any,
    ),
    cmd(
        "新建工作区",
        "newws",
        icon::FOLDER,
        "new workspace xjgzq xinjian 创建",
        Some("newws"),
        Scope::Any,
    ),
    cmd(
        "聚焦输入框",
        "focus",
        icon::PENCIL,
        "focus composer message input jjsrk shuru 输入框 消息",
        Some("focus"),
        Scope::Composer,
    ),
    cmd(
        "上一个对话",
        "prevchat",
        icon::CHAT,
        "previous chat syg shangyige 对话 切换",
        Some("prevchat"),
        Scope::Workspace,
    ),
    cmd(
        "下一个对话",
        "nextchat",
        icon::CHAT,
        "next chat xyg xiayige 对话 切换",
        Some("nextchat"),
        Scope::Workspace,
    ),
    cmd(
        "收起 / 展开侧栏",
        "side",
        icon::WINDOW,
        "sidebar toggle cl zhankai shouqi 侧边栏",
        Some("side"),
        Scope::Any,
    ),
    cmd(
        "显示 / 隐藏文件",
        "explorer",
        icon::FILE,
        "files explorer tree wj wenjian 文件树",
        Some("explorer"),
        Scope::Workspace,
    ),
    cmd(
        "显示 / 隐藏右侧面板",
        "chat",
        icon::WINDOW,
        "inspector right panel yc youce mianban 检查器",
        Some("chat"),
        Scope::Workspace,
    ),
    cmd(
        "显示 / 隐藏终端",
        "panel",
        icon::TERMINAL,
        "terminal shell zd zhongduan 命令行",
        Some("panel"),
        Scope::Workspace,
    ),
    cmd(
        "打开更改",
        "diff",
        icon::BRANCH,
        "changes diff git gg genggai 差异 提交 review 审阅",
        Some("diff"),
        Scope::Workspace,
    ),
    cmd(
        "预览",
        "preview",
        icon::EYE,
        "preview yl yulan 网页 端口",
        Some("preview"),
        Scope::Workspace,
    ),
    cmd(
        "键盘快捷键",
        "help",
        icon::KEYBOARD,
        "keyboard shortcuts keys help kjj kuaijiejian 速查表 按键",
        Some("help"),
        Scope::Any,
    ),
    cmd(
        "主题：浅色",
        "theme-light",
        icon::PALETTE,
        "theme light zt zhuti qs qianse 外观 白天",
        None,
        Scope::Any,
    ),
    cmd(
        "主题：深色",
        "theme-dark",
        icon::PALETTE,
        "theme dark zt zhuti ss shense 外观 夜间 暗色",
        None,
        Scope::Any,
    ),
    cmd(
        "主题：跟随系统",
        "theme-system",
        icon::PALETTE,
        "theme system auto zt zhuti gsxt 外观 自动",
        None,
        Scope::Any,
    ),
    cmd(
        "提醒方式…",
        "alerts",
        icon::BELL,
        "alerts notify tx tixing 通知 提示音",
        None,
        Scope::Any,
    ),
];

fn set_theme(theme: &str) {
    let mut prefs = storage::load::<Value>("blazar.ui")
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "v": 2 }));
    prefs["theme"] = json!(theme);
    storage::save("blazar.ui", &prefs);
    crate::pages::apply_ui_preferences();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Recent,
    Suggested,
    Workspace,
    Chat,
    Page,
    Command,
    Node,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Self::Recent => "最近",
            Self::Suggested => "建议",
            Self::Workspace => "工作区",
            Self::Chat => "对话",
            Self::Page => "页面",
            Self::Command => "命令",
            Self::Node => "机器",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Act {
    Go(String),
    Cmd(&'static str),
    Ws(String),
    Chat(String),
    Node(String),
}

#[derive(Clone)]
struct Item {
    icon: &'static str,
    title: String,
    sub: String,
    keys: String,
    key: Option<&'static str>,
    act: Act,
}

impl Item {
    fn page(p: &(&str, &str, &'static str, &str, Option<&'static str>)) -> Self {
        Self {
            icon: p.2,
            title: p.0.to_owned(),
            sub: String::new(),
            keys: p.3.to_owned(),
            key: p.4,
            act: Act::Go(p.1.to_owned()),
        }
    }

    fn command(c: &Cmd) -> Self {
        Self {
            icon: c.icon,
            title: c.title.to_owned(),
            sub: String::new(),
            keys: c.keys.to_owned(),
            key: c.key,
            act: Act::Cmd(c.act),
        }
    }

    fn workspace(w: &blazar_core_types::api::WorkspaceView) -> Self {
        Self {
            icon: icon::FOLDER,
            title: fmt::ws_title(&w.name, &w.node),
            sub: if w.node == "local" {
                w.path.clone()
            } else {
                format!("{}:{}", fmt::machine(&w.node), w.path)
            },
            keys: format!(
                "{} {} {}",
                w.name,
                w.node,
                w.project.clone().unwrap_or_default()
            ),
            key: None,
            act: Act::Ws(w.id.clone()),
        }
    }

    fn chat(t: &api::ChatThread) -> Self {
        let title = if t.title.trim().is_empty() {
            "新对话".to_owned()
        } else {
            t.title.clone()
        };
        let sub = if t.summary.is_empty() {
            fmt::ago(t.when())
        } else {
            t.summary.clone()
        };
        Self {
            icon: icon::CHAT,
            title,
            sub,
            keys: format!("{} chat 对话", t.summary),
            key: None,
            act: Act::Chat(t.id.clone()),
        }
    }

    fn node(n: &blazar_core_types::api::NodeView) -> Self {
        let status = if n.status == "online" {
            "在线"
        } else {
            "离线"
        };
        Self {
            icon: icon::NODES,
            title: fmt::machine(&n.name),
            sub: match n.ipv4.as_deref().filter(|v| !v.is_empty()) {
                Some(ip) => format!("{status} · {ip}"),
                None => status.to_owned(),
            },
            keys: format!("{} machine node", n.name),
            key: None,
            act: Act::Node(n.name.clone()),
        }
    }

    fn right(&self) -> Option<String> {
        self.key
            .map(|k| crate::shortcuts::label(&crate::shortcuts::current(k)))
            .filter(|k| !k.is_empty())
    }

    fn kind(&self) -> &'static str {
        match self.act {
            Act::Go(_) => Group::Page.title(),
            Act::Cmd(_) => Group::Command.title(),
            Act::Ws(_) => Group::Workspace.title(),
            Act::Chat(_) => Group::Chat.title(),
            Act::Node(_) => Group::Node.title(),
        }
    }
}

#[derive(Clone)]
struct Action {
    label: &'static str,
    icon: &'static str,
    hint: Option<String>,
    danger: bool,
    run: Callback<()>,
}

impl Action {
    fn new(
        label: &'static str,
        icon: &'static str,
        run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            label,
            icon,
            hint: None,
            danger: false,
            run: Callback::new(move |()| run()),
        }
    }

    fn hint(mut self, text: String) -> Self {
        self.hint = (!text.is_empty()).then_some(text);
        self
    }

    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

fn recent() -> Vec<String> {
    storage::load::<Vec<String>>(RECENT_KEY).unwrap_or_default()
}

fn remember(href: &str) {
    let mut list = recent();
    list.retain(|h| h != href);
    list.insert(0, href.to_owned());
    list.truncate(RECENT_MAX);
    storage::save(RECENT_KEY, &list);
}

fn caret_at(e: &ev::KeyboardEvent, end: bool) -> bool {
    let Some(input) = e
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return false;
    };
    let len = input.value().encode_utf16().count() as u32;
    let start = input.selection_start().ok().flatten().unwrap_or(0);
    let stop = input.selection_end().ok().flatten().unwrap_or(0);
    if end {
        start == len && stop == len
    } else {
        start == 0 && stop == 0
    }
}

fn reveal_selected() {
    request_animation_frame(|| {
        if let Some(el) = document()
            .query_selector(".palrow[data-sel=\"true\"]")
            .ok()
            .flatten()
        {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
            el.scroll_into_view_with_scroll_into_view_options(&opts);
        }
    });
}

async fn probe(name: String) {
    match api::send::<Value>(
        "POST",
        &format!("/api/nodes/{}/probe", api::enc(&name)),
        &json!({}),
    )
    .await
    {
        Ok(_) => toast(format!("已检查 {}", fmt::machine(&name))),
        Err(e) => toast(format!("检查失败：{e}")),
    }
}

#[component]
pub fn Palette() -> impl IntoView {
    let app = use_app();
    let q = RwSignal::new(String::new());
    let aq = RwSignal::new(String::new());
    let sel = RwSignal::new(0usize);
    let focus = RwSignal::new(None::<Item>);
    let keys_rev = RwSignal::new(0u32);
    let watch_keys = crate::shortcuts::on_change(move || {
        keys_rev.try_update(|n| *n += 1);
    });
    let nav = StoredValue::new(use_navigate());
    let location = use_location();
    Effect::new(move |_| {
        let path = location.pathname.get();
        if path.starts_with("/w/") || (path != "/" && PAGES.iter().any(|p| p.1 == path)) {
            remember(&path);
        }
    });

    let go = move |path: &str| {
        let path = path.to_owned();
        spawn_local(async move {
            if crate::files_js::confirm_leave().await {
                nav.with_value(|n| n(&path, Default::default()));
            }
        });
    };
    let new_chat = move || {
        if app.current_ws.get_untracked().is_some() {
            crate::shortcuts::fire("act:newchat");
            return;
        }
        go("/chats/new");
    };
    let run_cmd = move |action: &str| -> bool {
        match action {
            "palette" => app.palette.update(|p| *p = !*p),
            "side" => app.side_collapsed.update(|p| *p = !*p),
            "console" => go("/"),
            "workspaces" => go("/workspaces"),
            "newws" => app.new_ws.set(true),
            "inbox" => go("/?inbox=all"),
            "tasks" => go("/tasks"),
            "settings" => go("/settings"),
            "alerts" => app.alerts_open.set(true),
            "theme-light" => set_theme("light"),
            "theme-dark" => set_theme("dark"),
            "theme-system" => set_theme("system"),
            "newchat" if app.current_ws.get_untracked().is_none() => new_chat(),
            _ => return false,
        }
        true
    };
    let keys = window_event_listener(ev::keydown, move |e| {
        if let Some(action) = crate::shortcuts::action(&e)
            && run_cmd(action)
        {
            e.prevent_default();
        }
    });
    let menu = crate::shortcuts::on_menu(move |id| {
        if let Some(action) = id.strip_prefix("act:") {
            run_cmd(action);
        } else if let Some(path) = id.strip_prefix("go:") {
            go(path);
        } else if let Some(message) = id.strip_prefix("toast:") {
            toast(message);
        } else if id == "new-workspace" {
            app.new_ws.set(true);
        }
    });
    on_cleanup(move || {
        keys.remove();
        menu.remove();
        watch_keys.remove();
    });
    crate::shortcuts::sync_menu();
    Effect::new(move |_| {
        if app.palette.get() {
            q.set(String::new());
            aq.set(String::new());
            sel.set(0);
            focus.set(None);
        }
    });

    let sections = move || -> Vec<(Group, Vec<Item>)> {
        keys_rev.track();
        let query = q.get();
        let state = app.state.get();
        if query.trim().is_empty() {
            let mut recent_items = Vec::new();
            for href in recent() {
                if let Some(id) = href.strip_prefix("/w/") {
                    if let Some(w) = app.workspace(id) {
                        recent_items.push(Item::workspace(&w));
                    }
                } else if let Some(p) = PAGES.iter().find(|p| p.1 == href) {
                    recent_items.push(Item::page(p));
                }
            }
            let suggested = COMMANDS
                .iter()
                .filter(|c| matches!(c.act, "newchat" | "newws" | "help"))
                .map(Item::command)
                .chain([Item::page(&PAGES[0]), Item::page(&PAGES[PAGES.len() - 1])])
                .collect();
            return vec![(Group::Recent, recent_items), (Group::Suggested, suggested)]
                .into_iter()
                .filter(|(_, items)| !items.is_empty())
                .collect();
        }
        let in_ws = app.current_ws.get().is_some();
        let available = |scope: Scope| match scope {
            Scope::Any => true,
            Scope::Workspace => in_ws,
            Scope::Composer => document()
                .query_selector("textarea.prompt")
                .ok()
                .flatten()
                .is_some(),
        };
        let rank = |items: Vec<Item>| -> Vec<Item> {
            let mut scored: Vec<(u32, Item)> = items
                .into_iter()
                .filter_map(|it| score(&query, &it.title, &it.keys).map(|s| (s, it)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored.into_iter().map(|(_, it)| it).take(8).collect()
        };
        let workspaces = state
            .as_ref()
            .map(|s| {
                s.workspaces
                    .iter()
                    .filter(|w| crate::app_state::is_top(w, &s.workspaces))
                    .map(Item::workspace)
                    .collect()
            })
            .unwrap_or_default();
        let nodes = state
            .as_ref()
            .map(|s| s.nodes.iter().map(Item::node).collect())
            .unwrap_or_default();
        let chats = app
            .chats
            .with(|a| a.threads.iter().map(Item::chat).collect());
        vec![
            (Group::Workspace, rank(workspaces)),
            (Group::Chat, rank(chats)),
            (Group::Page, rank(PAGES.iter().map(Item::page).collect())),
            (
                Group::Command,
                rank(
                    COMMANDS
                        .iter()
                        .filter(|c| available(c.scope))
                        .map(Item::command)
                        .collect(),
                ),
            ),
            (Group::Node, rank(nodes)),
        ]
        .into_iter()
        .filter(|(_, items)| !items.is_empty())
        .collect()
    };
    let flat = move || -> Vec<Item> {
        sections()
            .into_iter()
            .flat_map(|(_, items)| items)
            .collect()
    };

    let actions_of = move |item: &Item| -> Vec<Action> {
        match &item.act {
            Act::Ws(id) => {
                let w = app.workspace(id);
                let path = w.as_ref().map(|w| w.path.clone()).unwrap_or_default();
                let local = w.as_ref().is_some_and(|w| w.node == "local");
                let (href, i1, i2, i3) = (format!("/w/{id}"), id.clone(), id.clone(), id.clone());
                let (p1, p2) = (path.clone(), path);
                let mut list = vec![
                    Action::new("打开", icon::ARROW, move || go(&href))
                        .hint(crate::shortcuts::label("Enter")),
                    Action::new("简介", icon::INFO, move || {
                        app.inspect_ws.set(Some(i1.clone()))
                    })
                    .hint(crate::shortcuts::label(
                        &crate::shortcuts::current("inspect"),
                    )),
                    Action::new("改名…", icon::PENCIL, move || {
                        crate::pages::workspaces::rename_workspace(app, i2.clone())
                    }),
                    Action::new("复制路径", icon::COPY, move || {
                        spawn_local(crate::pages::work_shared::copy(p1.clone()))
                    }),
                ];
                if local && crate::desktop::available() {
                    list.push(Action::new("在访达中显示", icon::EYE, move || {
                        let path = p2.clone();
                        spawn_local(async move {
                            if let Err(e) =
                                crate::desktop::invoke("reveal_path", &json!({ "path": path }))
                                    .await
                            {
                                toast(format!("没能在访达中显示：{e}"));
                            }
                        });
                    }));
                }
                list.push(
                    Action::new("移除…", icon::TRASH, move || {
                        crate::pages::workspaces::remove_workspace(app, i3.clone())
                    })
                    .hint(crate::shortcuts::label(&crate::shortcuts::current(
                        "delete",
                    )))
                    .danger(),
                );
                list
            }
            Act::Node(name) => {
                let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
                vec![
                    Action::new("打开", icon::ARROW, move || {
                        go(&format!("/nodes/{}", api::enc(&n1)));
                    })
                    .hint(crate::shortcuts::label("Enter")),
                    Action::new("新建工作区", icon::PLUS, move || {
                        app.new_ws_node.set(Some(n2.clone()));
                        app.new_ws.set(true);
                    }),
                    Action::new("检查", icon::ACTIVITY, move || {
                        spawn_local(probe(n3.clone()))
                    }),
                ]
            }
            _ => Vec::new(),
        }
    };
    let shown_actions = move || -> Vec<Action> {
        let Some(item) = focus.get() else {
            return Vec::new();
        };
        let needle = aq.get().trim().to_lowercase();
        actions_of(&item)
            .into_iter()
            .filter(|a| needle.is_empty() || a.label.to_lowercase().contains(&needle))
            .collect()
    };

    let run_item = move |it: &Item| {
        app.palette.set(false);
        match &it.act {
            Act::Go(href) => go(href),
            Act::Ws(id) => go(&format!("/w/{id}")),
            Act::Chat(id) => go(&format!("/chats/{id}")),
            Act::Node(name) => go(&format!("/nodes/{}", api::enc(name))),
            Act::Cmd("newchat") => new_chat(),
            Act::Cmd(action) => {
                if !run_cmd(action) {
                    // The page that owns it listens for the same event the native menu sends;
                    // wait a frame so the palette has handed focus back first.
                    let id = format!("act:{action}");
                    request_animation_frame(move || crate::shortcuts::fire(&id));
                }
            }
        }
    };
    let run_action = move |a: &Action| {
        app.palette.set(false);
        a.run.run(());
    };
    let enter_actions = move || -> bool {
        let Some(item) = flat().get(sel.get_untracked()).cloned() else {
            return false;
        };
        if !matches!(item.act, Act::Ws(_) | Act::Node(_)) {
            return false;
        }
        focus.set(Some(item));
        aq.set(String::new());
        sel.set(0);
        true
    };
    let leave_actions = move || {
        let back = focus
            .get_untracked()
            .and_then(|it| flat().iter().position(|x| x.act == it.act))
            .unwrap_or(0);
        focus.set(None);
        aq.set(String::new());
        sel.set(back);
        reveal_selected();
    };
    let count = move || {
        if focus.with_untracked(Option::is_some) {
            shown_actions().len()
        } else {
            flat().len()
        }
    };
    let on_key = move |e: ev::KeyboardEvent| {
        let meta = crate::shortcuts::mod_key(&e);
        let in_actions = focus.with_untracked(Option::is_some);
        let on_input = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            .is_some();
        match e.key().as_str() {
            "ArrowDown" | "ArrowUp" => {
                e.prevent_default();
                let n = count().max(1);
                let down = e.key() == "ArrowDown";
                sel.update(|s| *s = if down { (*s + 1) % n } else { (*s + n - 1) % n });
                reveal_selected();
            }
            "Enter" if on_input && !e.is_composing() => {
                e.prevent_default();
                if in_actions {
                    if let Some(a) = shown_actions().get(sel.get_untracked()) {
                        run_action(a);
                    }
                } else if let Some(it) = flat().get(sel.get_untracked()) {
                    run_item(it);
                }
            }
            "Escape" if in_actions => {
                e.prevent_default();
                e.stop_propagation();
                leave_actions();
            }
            "ArrowLeft" if in_actions && on_input && caret_at(&e, false) => {
                e.prevent_default();
                leave_actions();
            }
            "ArrowRight" if !in_actions && on_input && caret_at(&e, true) => {
                if enter_actions() {
                    e.prevent_default();
                }
            }
            _ if meta && e.code() == "KeyK" => {
                e.prevent_default();
                if in_actions {
                    leave_actions();
                } else {
                    enter_actions();
                }
            }
            _ if meta && e.code() == "KeyI" && !in_actions => {
                if let Some(Item {
                    act: Act::Ws(id), ..
                }) = flat().get(sel.get_untracked())
                {
                    e.prevent_default();
                    app.palette.set(false);
                    app.inspect_ws.set(Some(id.clone()));
                }
            }
            _ => {}
        }
    };

    let input_el = NodeRef::<html::Input>::new();
    view! {
        <Show when=move || app.palette.get()>
            <Modal
                label="命令面板"
                class="palette"
                mask_class="dlg-mask top"
                on_close=Callback::new(move |_| app.palette.set(false))
            >
                <div class="pal" on:keydown=on_key>
                    <div class="pal-search">
                        <span class="ic" inner_html=icon::svg(icon::SEARCH)></span>
                        <input
                            node_ref=input_el
                            data-modal-initial-focus=""
                            aria-label="搜索工作区、对话、页面、命令和机器"
                            placeholder=move || {
                                match focus.get() {
                                    Some(it) => format!("「{}」的操作…", it.title),
                                    None => {
                                        "搜索工作区、对话、页面、命令和机器…"
                                            .to_owned()
                                    }
                                }
                            }
                            autocomplete="off"
                            spellcheck="false"
                            prop:value=move || {
                                if focus.with(Option::is_some) { aq.get() } else { q.get() }
                            }
                            on:input=move |e| {
                                let v = event_target_value(&e);
                                if focus.with_untracked(Option::is_some) {
                                    aq.set(v);
                                } else {
                                    q.set(v);
                                }
                                sel.set(0);
                            }
                        />
                    </div>
                    <div class="pal-list">
                        <div class="pal-flow">
                            {move || {
                                if let Some(item) = focus.get() {
                                    let list = shown_actions();
                                    if list.is_empty() {
                                        return view! {
                                            <div class="pal-empty">"没有匹配的结果"</div>
                                        }
                                            .into_any();
                                    }
                                    return view! {
                                        <div class="pal-group">{item.title.clone()}</div>
                                        {list
                                            .into_iter()
                                            .enumerate()
                                            .map(|(i, a)| {
                                                let a2 = a.clone();
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="palrow"
                                                        class:danger=a.danger
                                                        tabindex="-1"
                                                        data-sel=move || (sel.get() == i).to_string()
                                                        on:click=move |_| run_action(&a2)
                                                    >
                                                        <span class="ic" inner_html=icon::svg(a.icon)></span>
                                                        <span class="t">{a.label}</span>
                                                        <span class="s"></span>
                                                        {a.hint.map(|h| view! { <kbd>{h}</kbd> })}
                                                    </button>
                                                }
                                            })
                                            .collect_view()}
                                    }
                                        .into_any();
                                }
                                let sections = sections();
                                if sections.is_empty() {
                                    return view! {
                                        <div class="pal-empty">"没有匹配的结果"</div>
                                    }
                                        .into_any();
                                }
                                let mut n = 0usize;
                                sections
                                    .into_iter()
                                    .map(|(group, items)| {
                                        let rows = items
                                            .into_iter()
                                            .map(|it| {
                                                let i = n;
                                                n += 1;
                                                let it2 = it.clone();
                                                let right = it.right();
                                                view! {
                                                    <button
                                                        type="button"
                                                        class="palrow"
                                                        tabindex="-1"
                                                        data-sel=move || (sel.get() == i).to_string()
                                                        on:click=move |_| run_item(&it2)
                                                    >
                                                        <span class="ic" inner_html=icon::svg(it.icon)></span>
                                                        <span class="t">{it.title.clone()}</span>
                                                        <span class="s">{it.sub.clone()}</span>
                                                        {match right {
                                                            Some(k) => view! { <kbd>{k}</kbd> }.into_any(),
                                                            None => {
                                                                view! { <span class="c">{it.kind()}</span> }.into_any()
                                                            }
                                                        }}
                                                    </button>
                                                }
                                            })
                                            .collect_view();
                                        view! {
                                            <div class="pal-group">{group.title()}</div>
                                            {rows}
                                        }
                                    })
                                    .collect_view()
                                    .into_any()
                            }}
                        </div>
                    </div>
                    <div class="pal-foot">
                        <span class="pal-mode">
                            {move || {
                                if focus.with(Option::is_some) { "操作" } else { "Blazar" }
                            }}
                        </span>
                        <span class="grow"></span>
                        <span class="pal-key">
                            {move || if focus.with(Option::is_some) { "执行" } else { "打开" }}
                            <kbd>{crate::shortcuts::label("Enter")}</kbd>
                        </span>
                        <span class="pal-key">
                            {move || if focus.with(Option::is_some) { "返回" } else { "操作" }}
                            <kbd>{crate::shortcuts::label("Mod+K")}</kbd>
                        </span>
                    </div>
                </div>
            </Modal>
        </Show>
    }
}

#[component]
pub fn AlertsDialog() -> impl IntoView {
    use crate::alerts::{self, SoundPrefs};
    use crate::components::toast::toast;
    let app = use_app();
    let prefs = RwSignal::new(alerts::sound_prefs());
    let sys = RwSignal::new(alerts::notify_on());
    let save = move |p: SoundPrefs| {
        alerts::set_sound_prefs(&p);
        prefs.set(p);
        app.alerts_rev.update(|n| *n += 1);
    };
    let toggle_sys = move |_| {
        leptos::task::spawn_local(async move {
            if alerts::notify_on() {
                alerts::set_notify(false);
                sys.set(false);
            } else if !alerts::notify_supported() {
                toast("这个浏览器不支持系统通知");
            } else if alerts::request_permission().await == "granted" {
                alerts::set_notify(true);
                sys.set(true);
                toast("已开启：跑完、出错、等你批准时会弹系统通知");
            } else {
                toast("浏览器没有给通知权限，可以在地址栏左侧的站点设置里打开");
            }
            app.alerts_rev.update(|n| *n += 1);
        });
    };
    view! {
        <Show when=move || app.alerts_open.get()>
            <Modal label="提醒方式" on_close=Callback::new(move |_| app.alerts_open.set(false))>
                <h3>"提醒方式"</h3>
                <label class="chk block">
                    <input type="checkbox" prop:checked=move || sys.get() on:change=toggle_sys />
                    <span>
                        <b>"系统通知"</b>
                        <span class="muted small">" 完成或需要处理时通知你"</span>
                    </span>
                </label>
                <label class="chk block">
                    <input
                        type="checkbox"
                        prop:checked=move || prefs.get().on
                        on:change=move |_| {
                            let mut p = prefs.get_untracked();
                            p.on = !p.on;
                            let on = p.on;
                            save(p);
                            if on {
                                alerts::play("done", true);
                            }
                        }
                    />
                    <span>
                        <b>"提示音"</b>
                        <span class="muted small">" 区分完成和待处理"</span>
                    </span>
                </label>
                <div class="row-actions">
                    <span class="muted small">"音色"</span>
                    <span class="seg">
                        {[("soft", "柔和"), ("bright", "清脆"), ("wood", "木质")]
                            .into_iter()
                            .map(|(k, l)| {
                                view! {
                                    <button
                                        type="button"
                                        data-on=move || (prefs.get().tone == k).to_string()
                                        on:click=move |_| {
                                            let mut p = prefs.get_untracked();
                                            p.tone = k.into();
                                            save(p);
                                            alerts::play("done", true);
                                        }
                                    >
                                        {l}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </span>
                    <span class="muted small">"音量"</span>
                    <input
                        aria-label="提示音音量"
                        type="range"
                        min="0.05"
                        max="1"
                        step="0.05"
                        prop:value=move || prefs.get().volume.to_string()
                        on:change=move |e| {
                            let mut p = prefs.get_untracked();
                            p.volume = event_target_value(&e).parse().unwrap_or(0.5);
                            save(p);
                            alerts::play("done", true);
                        }
                    />
                </div>
                <div class="row-actions">
                    <button
                        type="button"
                        class="btn small"
                        on:click=move |_| alerts::play("done", true)
                    >
                        "试听完成提示"
                    </button>
                    <button
                        type="button"
                        class="btn small"
                        on:click=move |_| alerts::play("attention", true)
                    >
                        "试听待处理提示"
                    </button>
                </div>
                <div class="dlg-foot">
                    <button
                        type="button"
                        class="btn primary"
                        on:click=move |_| app.alerts_open.set(false)
                    >
                        "完成"
                    </button>
                </div>
            </Modal>
        </Show>
    }
}
