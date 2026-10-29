use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::{Value, json};

use crate::api;
use crate::app_state::use_app;
use crate::components::icon;
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::pages::runtimes::{active_account, install_here, login, runtime_status};

#[derive(Clone, PartialEq)]
enum LineAction {
    Link(&'static str, String),
    Login(api::Account, &'static str),
    Install(String),
}

#[derive(Clone, PartialEq)]
struct RuntimeLine {
    ready: bool,
    text: String,
    action: Option<LineAction>,
    attention: bool,
}

fn runtime_line(r: &api::Runtime, accounts: Option<&api::Accounts>) -> RuntimeLine {
    let href = format!("/runtimes/{}", r.id);
    if !r.installed {
        return RuntimeLine {
            ready: false,
            text: format!("{} 未安装", r.label),
            action: Some(LineAction::Install(r.id.clone())),
            attention: false,
        };
    }
    let account = accounts.and_then(|a| active_account(a, &r.id));
    let (status, _) = runtime_status(r, account.as_ref());
    let name = format!(
        "{}{}",
        r.label,
        r.version
            .as_deref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default()
    );
    if status == "已登录" {
        return RuntimeLine {
            ready: true,
            text: name,
            action: None,
            attention: false,
        };
    }
    // Installed but not signed in (never, or the login lapsed) is the one thing left to do
    // on this step, so it gets the primary button that opens the login dialog directly.
    let action = match account {
        Some(a) if status == "未登录" && a.kind != "token" => LineAction::Login(a, "登录…"),
        Some(a) if a.kind != "token" => LineAction::Login(a, "重新登录"),
        _ => LineAction::Link("设置", href),
    };
    RuntimeLine {
        ready: false,
        attention: true,
        text: format!("{name} · {status}"),
        action: Some(action),
    }
}

pub fn looks_like_git_url(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) {
        return false;
    }
    let scheme = ["https://", "http://", "ssh://", "git://", "git@"]
        .iter()
        .any(|p| t.starts_with(p));
    (scheme && t.len() > 12) || t.ends_with(".git")
}

fn dropped_path(dt: &web_sys::DataTransfer) -> Option<String> {
    let uris = dt.get_data("text/uri-list").unwrap_or_default();
    let first = uris
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned);
    let plain = dt.get_data("text/plain").unwrap_or_default();
    let candidate = first.unwrap_or_else(|| plain.trim().to_owned());
    if let Some(rest) = candidate.strip_prefix("file://") {
        let path = rest.trim_start_matches("localhost");
        let decoded = js_sys::decode_uri_component(path)
            .map(String::from)
            .unwrap_or_else(|_| path.to_owned());
        return Some(decoded.trim_end_matches('/').to_owned()).filter(|p| p.starts_with('/'));
    }
    candidate
        .starts_with('/')
        .then(|| candidate.trim_end_matches('/').to_owned())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Home {
    Wait,
    Start,
    Console,
}

/// `/`：一个工作区都没有时直接显示「开始」，建好第一个之后才是中控台。
/// 等工作区列表回来之前什么都不画，免得中控台先闪一下再换成开始清单。
#[component]
pub fn HomePage() -> impl IntoView {
    let app = use_app();
    let location = leptos_router::hooks::use_location();
    let home = Memo::new(move |_| {
        if location.pathname.with(|p| p != "/") {
            return Home::Console;
        }
        match app.state.with(Option::is_some) {
            false if app.state_error.with(Option::is_none) => Home::Wait,
            true if app.top_workspaces().is_empty() => Home::Start,
            _ => Home::Console,
        }
    });
    move || match home.get() {
        Home::Wait => view! { <LoadingState text="正在加载…" /> }.into_any(),
        Home::Start => view! {
            <div class="page home-page home-start">
                <Start />
            </div>
        }
        .into_any(),
        Home::Console => view! { <crate::pages::ConsolePage /> }.into_any(),
    }
}

#[component]
pub fn Start() -> impl IntoView {
    let app = use_app();
    let go = StoredValue::new_local(use_navigate());
    let runtimes = LocalResource::new(|| api::get::<api::Runtimes>("/api/runtimes"));
    let accounts = LocalResource::new(|| api::get::<api::Accounts>("/api/accounts"));
    let busy = RwSignal::new(None::<String>);
    let failed = RwSignal::new(None::<(String, Option<String>)>);
    let installing = RwSignal::new(None::<String>);
    let install_failed = RwSignal::new(None::<(String, Option<String>)>);
    let url = RwSignal::new(String::new());
    let over = RwSignal::new(false);
    let open = move |id: String| {
        app.load_state();
        go.with_value(|n| n(&format!("/w/{id}"), Default::default()));
    };
    let install = move |id: String| {
        if installing.get_untracked().is_some() {
            return;
        }
        installing.set(Some(id.clone()));
        install_failed.set(None);
        spawn_local(async move {
            let r = install_here(&id).await;
            let _ = installing.try_set(None);
            match r {
                Ok(_) => {
                    toast(format!("{} 装好了", crate::vocab::term(&id)));
                    runtimes.refetch();
                    accounts.refetch();
                }
                Err(e) => {
                    let _ = install_failed.try_set(Some(e.explained()));
                }
            }
        });
    };
    let add_folder = move |path: String| {
        if busy.get_untracked().is_some() {
            return;
        }
        busy.set(Some(format!("正在添加 {path}…")));
        failed.set(None);
        spawn_local(async move {
            let r = api::send::<Value>(
                "POST",
                "/api/workspaces",
                &json!({ "node": "local", "path": path }),
            )
            .await;
            let _ = busy.try_set(None);
            match r {
                Ok(v) => {
                    if let Some(id) = v["id"].as_str() {
                        open(id.to_owned());
                    }
                }
                Err(e) => {
                    let _ = failed.try_set(Some(e.explained()));
                }
            }
        });
    };
    let clone_repo = move |u: String| {
        let u = u.trim().to_owned();
        if !looks_like_git_url(&u) {
            toast("这不像一个 Git 地址");
            return;
        }
        if busy.get_untracked().is_some() {
            return;
        }
        busy.set(Some("正在克隆，大仓库要等一会儿…".into()));
        failed.set(None);
        spawn_local(async move {
            let r = api::send::<Value>("POST", "/api/workspaces/clone", &json!({ "url": u })).await;
            let _ = busy.try_set(None);
            match r {
                Ok(v) => {
                    if let Some(p) = v["path"].as_str() {
                        toast(format!("已克隆到 {p}"));
                    }
                    if let Some(id) = v["id"].as_str() {
                        open(id.to_owned());
                    }
                }
                Err(e) => {
                    let _ = failed.try_set(Some(e.explained()));
                }
            }
        });
    };
    let on_drop = move |e: ev::DragEvent| {
        e.prevent_default();
        over.set(false);
        let Some(dt) = e.data_transfer() else {
            return;
        };
        let plain = dt.get_data("text/plain").unwrap_or_default();
        if looks_like_git_url(&plain) {
            clone_repo(plain);
            return;
        }
        match dropped_path(&dt) {
            Some(p) => add_folder(p),
            None => toast("拿不到文件夹的位置，请用「选择项目文件夹…」"),
        }
    };
    let keys = crate::desktop::available().then(|| {
        view! { <kbd>{crate::shortcuts::label(&crate::shortcuts::current("newws"))}</kbd> }
    });
    let runtime_step = move || match (runtimes.get(), accounts.get()) {
        (None, _) | (_, None) => (
            false,
            view! { <p class="start-note">"正在检测 Claude Code 和 Codex…"</p> }.into_any(),
        ),
        (Some(Err(e)), _) => {
            let (message, detail) = e.explained();
            (
                false,
                view! {
                    <InlineError
                        message=format!("检测失败：{message}")
                        detail
                        retry=Callback::new(move |_| {
                            runtimes.refetch();
                            accounts.refetch();
                        })
                    />
                }
                .into_any(),
            )
        }
        (Some(Ok(r)), Some(accounts)) => {
            let accounts = accounts.ok();
            let lines: Vec<RuntimeLine> = r
                .runtimes
                .iter()
                .filter(|x| matches!(x.id.as_str(), "claude" | "codex"))
                .map(|x| runtime_line(x, accounts.as_ref()))
                .collect();
            let ready = lines.iter().any(|l| l.ready) && !lines.iter().any(|l| l.attention);
            if lines.is_empty() {
                return (
                    false,
                    view! {
                        <p class="start-note">
                            "没有检测到 Claude Code 或 Codex。"
                            <a href="/runtimes">"去看看"</a>
                        </p>
                    }
                    .into_any(),
                );
            }
            (
                ready,
                view! {
                    <ul class="start-lines">
                        {lines
                            .into_iter()
                            .map(|l| {
                                view! {
                                    <li
                                        data-ok=l.ready.to_string()
                                        data-attn=l.attention.to_string()
                                    >
                                        <span>{l.text}</span>
                                        {l
                                            .action
                                            .map(|action| match action {
                                                LineAction::Link(label, href) => {
                                                    view! {
                                                        <a class="linkbtn" href=href>
                                                            {label}
                                                        </a>
                                                    }
                                                        .into_any()
                                                }
                                                LineAction::Login(a, label) => {
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class="btn small primary"
                                                            on:click=move |_| login(&a)
                                                        >
                                                            {label}
                                                        </button>
                                                    }
                                                        .into_any()
                                                }
                                                LineAction::Install(id) => {
                                                    view! {
                                                        <button
                                                            type="button"
                                                            class="btn small"
                                                            disabled=move || { installing.get().is_some() }
                                                            on:click=move |_| install(id.clone())
                                                        >
                                                            "安装"
                                                        </button>
                                                    }
                                                        .into_any()
                                                }
                                            })}
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                    {move || {
                        installing
                            .get()
                            .map(|id| {
                                view! {
                                    <p class="start-note" role="status">
                                        {format!(
                                            "正在下载安装 {}，要几分钟…",
                                            crate::vocab::term(&id),
                                        )}
                                    </p>
                                }
                            })
                    }}
                    {move || {
                        install_failed
                            .get()
                            .map(|(message, detail)| view! { <InlineError message detail /> })
                    }}
                }
                .into_any(),
            )
        }
    };
    view! {
        <section class="start" aria-label="开始">
            <h1 class="start-title">"开始"</h1>
            <p class="start-lead">"两步，智能体就能在你的代码上干活。"</p>
            <ol class="start-steps">
                {move || {
                    let (done, body) = runtime_step();
                    view! {
                        <li
                            class="start-step"
                            data-done=done.to_string()
                            data-current=(!done).to_string()
                        >
                            <span class="start-mark" aria-hidden="true">
                                {if done {
                                    view! { <span inner_html=icon::svg(icon::TICK)></span> }
                                        .into_any()
                                } else {
                                    "1".into_any()
                                }}
                            </span>
                            <div class="start-body">
                                <b>"装好 Claude Code 或 Codex"</b>
                                {body}
                            </div>
                        </li>
                    }
                }}
                <li
                    class="start-step"
                    data-done="false"
                    data-current=move || runtime_step().0.to_string()
                    data-over=move || over.get().to_string()
                    on:dragenter=move |e: ev::DragEvent| {
                        e.prevent_default();
                        over.set(true);
                    }
                    on:dragover=move |e: ev::DragEvent| {
                        e.prevent_default();
                        over.set(true);
                    }
                    on:dragleave=move |_| over.set(false)
                    on:drop=on_drop
                >
                    <span class="start-mark" aria-hidden="true">
                        "2"
                    </span>
                    <div class="start-body">
                        <b>"选项目文件夹"</b>
                        <div class="start-actions">
                            <button
                                type="button"
                                class="btn primary"
                                aria-haspopup="dialog"
                                disabled=move || busy.get().is_some()
                                on:click=move |_| app.new_ws.set(true)
                            >
                                "选择项目文件夹…"
                                {keys}
                            </button>
                            <span class="start-or">"或把文件夹拖到这里"</span>
                        </div>
                        <input
                            class="start-url"
                            aria-label="Git 地址"
                            placeholder="粘贴 Git 地址，回车克隆"
                            disabled=move || busy.get().is_some()
                            prop:value=move || url.get()
                            on:input=move |e| url.set(event_target_value(&e))
                            on:paste=move |e: ev::ClipboardEvent| {
                                let text = e
                                    .clipboard_data()
                                    .and_then(|d| d.get_data("text").ok())
                                    .unwrap_or_default();
                                if looks_like_git_url(&text) {
                                    e.prevent_default();
                                    url.set(text.trim().to_owned());
                                    clone_repo(text);
                                }
                            }
                            on:keydown=move |e: ev::KeyboardEvent| {
                                if e.key() == "Enter" && !e.is_composing() {
                                    e.prevent_default();
                                    clone_repo(url.get_untracked());
                                }
                            }
                        />
                        {move || {
                            busy.get()
                                .map(|b| {
                                    view! {
                                        <p class="start-note" role="status">
                                            {b}
                                        </p>
                                    }
                                })
                        }}
                        {move || {
                            failed
                                .get()
                                .map(|(message, detail)| view! { <InlineError message detail /> })
                        }}
                    </div>
                </li>
            </ol>
            <p class="start-skip">
                <a class="linkbtn" href="/chats/new">
                    "先去中控台看看"
                </a>
            </p>
        </section>
    }
}
