use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{self, Account, Accounts, Runtime, Runtimes};
use crate::components::dialog::{self, Choice};
use crate::components::term_dialog::{self, TermLogin};
use crate::components::toast::toast;
use crate::fmt;
use crate::realtime::use_bus;
use crate::rt_logo;
use crate::runtime_config::{AgentConfig, ConfigDrafts, DraftField};

pub const WITH_ACCOUNTS: [(&str, &str); 2] = [("claude", "Claude Code"), ("codex", "Codex")];

fn provider_label(p: &str) -> &str {
    WITH_ACCOUNTS.iter().find(|x| x.0 == p).map_or(p, |x| x.1)
}

const TOKEN_HELP: &str = "在浏览器里登录<b>这个</b>账号，然后在终端运行 <span class=\"mono\">claude setup-token</span>，把最后输出的 <span class=\"mono\">sk-ant-oat01-…</span> 整段复制过来。每个账号各生成一次即可，有效期约一年。";

fn plan_label(p: &str) -> &str {
    match p {
        "pro" => "Pro",
        "max" => "Max",
        "max5x" => "Max 5x",
        "max20x" => "Max 20x",
        "team" => "Team",
        "enterprise" => "Enterprise",
        "plus" => "Plus",
        "free" => "Free",
        "business" => "Business",
        "edu" => "Edu",
        other => other,
    }
}

fn model_label(m: &str) -> String {
    let x: Vec<&str> = m.trim_start_matches("claude-").split('-').collect();
    match x.split_first() {
        Some((first, rest)) if !first.is_empty() => {
            let mut c = first.chars();
            let head: String = c
                .next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default();
            if rest.is_empty() {
                head
            } else {
                format!("{head} {}", rest.join("."))
            }
        }
        _ => m.to_owned(),
    }
}

#[component]
pub fn RuntimesPage() -> impl IntoView {
    let bus = use_bus();
    let rescan = RwSignal::new(0u32);
    let runtimes = LocalResource::new(move || {
        rescan.track();
        api::get::<Runtimes>("/api/runtimes")
    });
    let accounts = LocalResource::new(move || {
        bus.accounts.track();
        rescan.track();
        api::get::<Accounts>("/api/accounts")
    });
    spawn_local(async move {
        let _ = api::send::<Value>("POST", "/api/accounts/check", &json!({})).await;
    });

    view! {
        <div class="page">
            <div class="page-head">
                <h1>"运行时"</h1>
                <span class="sub">
                    {move || runtimes.get().and_then(Result::ok).map(|r| format!("{} · 本机 · Blazar v{} · {} {}", r.machine.hostname, r.machine.blazar_version, r.machine.os, r.machine.arch))}
                </span>
                <span class="grow"></span>
                <button class="btn" on:click=move |_| rescan.update(|n| *n += 1)>"重新扫描"</button>
            </div>
            <GlobalSync accounts/>
            {move || match runtimes.get() {
                None => view! { <div class="empty">"扫描本机的 agent CLI…"</div> }.into_any(),
                Some(Err(e)) => view! { <div class="empty err-line">{e.to_string()}</div> }.into_any(),
                Some(Ok(r)) => {
                    let (inst, missing): (Vec<Runtime>, Vec<Runtime>) = r.runtimes.into_iter().partition(|x| x.installed);
                    let brains = inst.iter().any(|x| x.remote_hands && x.authed != Some(false));
                    view! {
                        {inst.into_iter().map(|rt| view! { <RuntimeCard rt accounts/> }).collect_view()}
                        {(!brains).then(|| view! {
                            <div class="card notice">"本机还没有可用的 Claude Code / Codex。在终端运行 claude 或 codex login 登录一次，或者在上面添加一个账号。"</div>
                        })}
                        {(!missing.is_empty()).then(|| view! {
                            <div class="muted small">{format!("还能识别但本机没装：{}", missing.iter().map(|x| x.label.as_str()).collect::<Vec<_>>().join("、"))}</div>
                        })}
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn RuntimeCard(
    rt: Runtime,
    accounts: LocalResource<Result<Accounts, api::ApiError>>,
) -> impl IntoView {
    let id = rt.id.clone();
    let multi = WITH_ACCOUNTS.iter().any(|x| x.0 == id);
    let acc = move || accounts.get().and_then(Result::ok);
    let pid = id.clone();
    let active = move || {
        let a = acc()?;
        let on = active_id(&a, &pid)?;
        a.accounts.into_iter().find(|x| x.id == on)
    };
    let authed = rt.authed;
    let title = [
        rt.version.clone().map(|v| format!("v{v}")),
        rt.path.clone(),
        Some(if rt.remote_hands {
            "可用于任意工作区".to_owned()
        } else {
            "只能用于本机工作区".to_owned()
        }),
        (rt.cost_7d > 0.0).then(|| format!("近 7 天 ${:.2}", rt.cost_7d)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");

    view! {
        <section class="card" aria-label=rt.label.clone()>
            <div class="card-head">
                <span inner_html=rt_logo::mark(&id)></span>
                <b title=title>{rt.label.clone()}</b>
                <span class="muted small">{rt.version.clone()}</span>
                {move || if multi {
                    match active() {
                        Some(a) => view! { <span class=if a.usable() { "state ok" } else { "state bad" }>{format!("在用：{}", a.label)}</span> }.into_any(),
                        None => view! { <span class="muted small">"没选账号"</span> }.into_any(),
                    }
                } else {
                    match authed {
                        Some(true) => view! { <span class="state ok">"已登录"</span> }.into_any(),
                        Some(false) => view! { <span class="state bad">"未登录"</span> }.into_any(),
                        None => view! { <span class="muted small">"无法判断"</span> }.into_any(),
                    }
                }}
                <span class="grow"></span>
                <a class="btn ghost" href=format!("/runtimes/{id}")>"设置"</a>
            </div>
            {multi.then(|| view! { <AccountsSection provider=id.clone() accounts/> })}
        </section>
    }
}

fn active_id(a: &Accounts, p: &str) -> Option<String> {
    a.active(p)
}

#[derive(Clone, PartialEq)]
enum Form {
    Add,
    Token(Account),
    Plan(Account),
}

#[component]
pub fn AccountsSection(
    provider: String,
    accounts: LocalResource<Result<Accounts, api::ApiError>>,
) -> impl IntoView {
    let form = RwSignal::new(None::<Form>);
    let menu = RwSignal::new(None::<String>);
    let p2 = provider.clone();
    view! {
        <div class="acc-row head">
            <span>"账号"</span><span>"状态"</span><span>"额度"</span><span>"最近使用"</span>
            <span style="text-align:right"><button class="btn small" on:click=move |_| form.set(Some(Form::Add))>"＋ 添加账号"</button></span>
        </div>
        {move || {
            let Some(Ok(a)) = accounts.get() else { return ().into_any() };
            let on = a.active(&p2);
            a.accounts.into_iter()
                .filter(|x| x.provider == p2)
                .map(|x| {
                    let active = on.as_deref() == Some(x.id.as_str());
                    view! { <AccountRow a=x active menu form/> }
                })
                .collect_view()
                .into_any()
        }}
        {move || form.get().map(|f| view! { <AccountForm f provider=provider.clone() on_close=move || form.set(None)/> })}
    }
}

fn status(a: &Account) -> impl IntoView + use<> {
    let tok = a.kind == "token";
    let (cls, text, title) = if a.disabled {
        ("state muted", "已停用", "")
    } else if a.status == "ok" {
        (
            "state ok",
            if tok { "已配置 token" } else { "已登录" },
            if tok {
                "CLI 只能确认 token 在；是否有效要等第一次运行，认证失败会自动标成无效"
            } else {
                ""
            },
        )
    } else if a.status == "logged_out" {
        ("state bad", if tok { "token 无效" } else { "未登录" }, "")
    } else {
        ("state muted", "无法判断", "")
    };
    view! { <span class=cls title=title>{text}</span> }
}

fn quota(a: &Account) -> AnyView {
    if a.windows.is_empty() {
        let hint = if a.kind == "token" || a.provider == "codex" {
            "还没查过 —— 「⋯」里点「刷新额度」"
        } else {
            "还没有数据 —— 用它跑一次会话就有了"
        };
        return view! { <span class="muted small">{hint}</span> }.into_any();
    }
    a.windows
        .iter()
        .map(|w| {
            let pct = (w.utilization * 100.0).round().clamp(0.0, 100.0);
            let lvl = if pct >= 90.0 {
                "bad"
            } else if pct >= 70.0 {
                "warn"
            } else {
                "ok"
            };
            view! {
                <div class="qbar" data-lvl=lvl>
                    <span class="l">{fmt::window_label(&w.name).to_owned()}</span>
                    <span class="t"><i style=format!("width:{pct}%")></i></span>
                    <span class="v">{format!("{pct}%")}</span>
                    <span class="r">{fmt::resets(w.resets_at.as_deref())}</span>
                </div>
            }
        })
        .collect_view()
        .into_any()
}

async fn patch(a: &Account, body: Value) -> Result<Value, api::ApiError> {
    api::send::<Value>("PUT", &format!("/api/accounts/{}", api::enc(&a.id)), &body).await
}

pub fn login(a: &Account) {
    let id = a.id.clone();
    let cmd = if a.provider == "claude" {
        "claude auth login"
    } else {
        "codex login"
    };
    term_dialog::open(TermLogin {
        title: format!("登录 {}", a.label),
        hint: format!(
            "正在运行 {cmd}{}。浏览器没自动打开的话，复制终端里的链接去登录。",
            if a.config_dir.is_some() {
                "（独立配置目录）"
            } else {
                "（默认登录）"
            }
        ),
        path: format!("/api/accounts/{}/login/ws", api::enc(&a.id)),
        after: Callback::new(move |()| {
            let id = id.clone();
            spawn_local(async move {
                match api::send::<Value>(
                    "POST",
                    &format!("/api/accounts/{}/check", api::enc(&id)),
                    &json!({}),
                )
                .await
                {
                    Ok(r) => {
                        let label = r["label"].as_str().unwrap_or("");
                        toast(if r["status"] == "ok" {
                            format!(
                                "「{label}」已登录{}",
                                r["email"]
                                    .as_str()
                                    .map(|e| format!("：{e}"))
                                    .unwrap_or_default()
                            )
                        } else {
                            format!("「{label}」还没有登录成功")
                        });
                    }
                    Err(e) => toast(e.to_string()),
                }
            });
        }),
    });
}

pub fn node_login(node: String, after: Callback<()>) {
    term_dialog::open(TermLogin {
        title: format!("在 {node} 上登录 Codex"),
        hint: format!(
            "正在 {node} 上运行 codex login --device-auth。在本地浏览器打开下面的链接、输入验证码即可，凭据只保存在 {node} 上。"
        ),
        path: format!("/api/nodes/{}/login/codex/ws", api::enc(&node)),
        after,
    });
}

#[component]
fn AccountRow(
    a: Account,
    active: bool,
    menu: RwSignal<Option<String>>,
    form: RwSignal<Option<Form>>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let usable = a.usable();
    let tok = a.kind == "token";
    let title = [
        a.email.clone().unwrap_or_else(|| {
            if a.builtin {
                if a.provider == "claude" {
                    "~/.claude".into()
                } else {
                    "~/.codex".into()
                }
            } else {
                String::new()
            }
        }),
        a.plan
            .clone()
            .map(|p| plan_label(&p).to_owned())
            .unwrap_or_default(),
        if tok {
            "长期 token".into()
        } else {
            "浏览器登录".into()
        },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    let a_use = a.clone();
    let on_use = move |_| {
        let a = a_use.clone();
        busy.set(true);
        spawn_local(async move {
            match api::use_account(&a).await {
                Ok(r) => toast(match (r["global"].as_bool(), r["global_error"].as_str()) {
                    (_, Some(e)) => format!("Blazar 已换成「{}」，但终端没跟上：{e}", a.label),
                    (Some(true), _) => format!(
                        "{} 现在用「{}」，终端和 Cursor 也换过去了",
                        provider_label(&a.provider),
                        a.label
                    ),
                    _ => format!("{} 现在用「{}」", provider_label(&a.provider), a.label),
                }),
                Err(e) => toast(format!("换不过去：{e}")),
            }
            let _ = busy.try_set(false);
        });
    };
    let open_menu = {
        let id = a.id.clone();
        move |_| {
            menu.update(|m| {
                *m = if m.as_deref() == Some(id.as_str()) {
                    None
                } else {
                    Some(id.clone())
                }
            })
        }
    };
    let mine = {
        let id = a.id.clone();
        move || menu.get().as_deref() == Some(id.as_str())
    };
    let a_menu = StoredValue::new(a.clone());
    let item = move |label: &'static str, danger: bool, act: &'static str| {
        view! {
            <button class:danger=danger on:click=move |_| {
                menu.set(None);
                let a = a_menu.get_value();
                match act {
                    "login" => login(&a),
                    "token" => form.set(Some(Form::Token(a))),
                    "plan" => form.set(Some(Form::Plan(a))),
                    _ => spawn_local(async move {
                        let r: Result<(), api::ApiError> = async {
                            match act {
                                "quota" => { api::refresh_quota(&a.id).await?; }
                                "rename" => {
                                    let Some(n) = window().prompt_with_message_and_default("新的名字", &a.label).ok().flatten().map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()) else { return Ok(()) };
                                    patch(&a, json!({ "label": n })).await?;
                                }
                                "toggle" => { patch(&a, json!({ "disabled": !a.disabled })).await?; }
                                "delete" => {
                                    let how = if a.kind == "token" { "会删掉保存的 token 和它的配置目录。token 本身在 Anthropic 那边仍然有效，不用了请到 claude.ai 的设置里吊销。" } else { "会先让 CLI 退出登录，再删掉它的配置目录。" };
                                    if dialog::ask("删除账号", &format!("删除账号「{}」？\n{how}共享的设置和会话历史不受影响。", a.label), vec![Choice::plain("取消"), Choice::danger("删除")]).await != Some(1) {
                                        return Ok(());
                                    }
                                    api::send::<Value>("DELETE", &format!("/api/accounts/{}", api::enc(&a.id)), &json!({})).await?;
                                    toast("已删除");
                                }
                                _ => {}
                            }
                            Ok(())
                        }.await;
                        if let Err(e) = r { toast(e.to_string()); }
                    }),
                }
            }>{label}</button>
        }
    };
    let blocks = a.model_blocks.clone();
    let aid = a.id.clone();
    view! {
        <div class="acc-row" class:on=active>
            <span class="name">
                <b title=title>{a.label.clone()}</b>
                {(!blocks.is_empty()).then(|| view! {
                    <div class="acc-mb">"用不了："
                        {blocks.into_iter().map(|b| {
                            let id = aid.clone();
                            let m = b.model.clone();
                            view! {
                                <span class="gchip warn" title=format!("{}被拒，7 天后自动重试", fmt::ago(&b.observed_at))>{model_label(&b.model)}
                                    <button class="x" title="清除，下次照常尝试" on:click=move |_| {
                                        let (id, m) = (id.clone(), m.clone());
                                        spawn_local(async move {
                                            if let Err(e) = api::send::<Value>("DELETE", &format!("/api/accounts/{}/models/{}", api::enc(&id), api::enc(&m)), &json!({})).await { toast(e.to_string()); }
                                        });
                                    }>"×"</button>
                                </span>
                            }
                        }).collect_view()}
                    </div>
                })}
            </span>
            {status(&a)}
            <div class="quota">{quota(&a)}</div>
            <span class="last">{a.last_used_at.as_deref().map_or_else(|| "—".to_owned(), fmt::ago)}</span>
            <span class="act">
                {if active {
                    view! { <span class="pill-on">"使用中"</span> }.into_any()
                } else {
                    view! {
                        <button class="btn small" on:click=on_use disabled=move || busy.get() || !usable
                            title=(!usable).then_some("这个账号现在不可用")>"使用"</button>
                    }.into_any()
                }}
                <span class="more">
                    <button class="btn ghost" aria-label="更多" on:click=open_menu>"⋯"</button>
                    <Show when=mine.clone()>
                        <div class="menu" on:mouseleave=move |_| menu.set(None)>
                            {(!tok).then(|| item(if a.status == "ok" { "重新登录" } else { "登录" }, false, "login"))}
                            {(tok || a.provider == "codex").then(|| item("刷新额度", false, "quota"))}
                            {(a.provider == "claude").then(|| item(if tok { "更换 token" } else { "改用长期 token" }, false, "token"))}
                            {tok.then(|| item(if a.plan.is_some() { "改订阅类型" } else { "标订阅类型" }, false, "plan"))}
                            {item("改名", false, "rename")}
                            {item(if a.disabled { "启用" } else { "停用" }, false, "toggle")}
                            {(!a.builtin).then(|| view! { <div class="menu-sep"></div> })}
                            {(!a.builtin).then(|| item("删除", true, "delete"))}
                        </div>
                    </Show>
                </span>
            </span>
        </div>
    }
}

#[component]
fn AccountForm(
    f: Form,
    provider: String,
    on_close: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let gate = RwSignal::new(crate::action_gate::ActionGate::default());
    let close = move || {
        if !gate.with_untracked(|g| g.busy("form")) {
            on_close();
        }
    };
    let name = RwSignal::new(String::new());
    let how = RwSignal::new("token".to_owned());
    let token = RwSignal::new(String::new());
    let plan = RwSignal::new(match &f {
        Form::Plan(a) => a.plan.clone().unwrap_or_default(),
        _ => String::new(),
    });
    let claude = provider == "claude";
    let f2 = f.clone();
    let p2 = provider.clone();
    let submit = move || {
        if !gate.write().begin("form") {
            return;
        }
        let f = f2.clone();
        let provider = p2.clone();
        spawn_local(async move {
            let mut saved = false;
            let r: Result<(), api::ApiError> = async {
                match &f {
                    Form::Add => {
                        let n = name.get_untracked().trim().to_owned();
                        if n.is_empty() { toast("名字必填"); return Ok(()); }
                        let use_token = claude && how.get_untracked() == "token";
                        let t = token.get_untracked().trim().to_owned();
                        if use_token && t.is_empty() { toast("把 claude setup-token 生成的 token 粘贴进来"); return Ok(()); }
                        let a: Account = api::send("POST", "/api/accounts", &json!({ "provider": provider, "label": n, "token": if use_token { Some(t) } else { None } })).await?;
                        saved = true;
                        if use_token {
                            toast(if a.status == "ok" { format!("「{}」已添加，第一次运行时会验证 token", a.label) } else { format!("「{}」已添加，但 CLI 没认出这个 token", a.label) });
                        } else {
                            login(&a);
                        }
                    }
                    Form::Token(a) => {
                        let t = token.get_untracked().trim().to_owned();
                        if t.is_empty() { return Ok(()); }
                        let r = api::send::<Value>("PUT", &format!("/api/accounts/{}/token", api::enc(&a.id)), &json!({ "token": t })).await?;
                        saved = true;
                        toast(if r["status"] == "ok" { "已保存，第一次运行时会验证 token" } else { "已保存，但 CLI 没认出这个 token" });
                    }
                    Form::Plan(a) => {
                        patch(a, json!({ "plan": plan.get_untracked() })).await?;
                        saved = true;
                    }
                }
                Ok(())
            }.await;
            let mut completed = false;
            let _ = gate.try_update(|g| completed = g.complete("form", r.is_ok() && saved));
            if let Err(e) = r {
                toast(e.to_string());
            }
            if completed {
                on_close();
            }
        });
    };
    let submit2 = submit.clone();
    let (title, ok) = match &f {
        Form::Add => (
            format!("给 {} 添加账号", provider_label(&provider)),
            "创建".to_owned(),
        ),
        Form::Token(a) => (
            format!(
                "{}长期 token · {}",
                if a.kind == "token" {
                    "更换"
                } else {
                    "改用"
                },
                a.label
            ),
            "保存".to_owned(),
        ),
        Form::Plan(a) => (format!("订阅类型 · {}", a.label), "保存".to_owned()),
    };
    let token_note = match &f {
        Form::Token(a) if a.kind != "token" && a.builtin => {
            "<br>改用 token 后，Blazar 里这个账号就用 token 运行（远端工作区也能用）；本机终端里 claude 的浏览器登录不受影响。"
        }
        Form::Token(a) if a.kind != "token" => {
            "<br>改用 token 后，这个账号原来的浏览器登录就不再使用了。"
        }
        _ => "",
    };
    let body = match f {
        Form::Add => view! {
            <label class="field">"名字"<input disabled=move || gate.with(|g| g.busy("form")) maxlength="40" placeholder="例如：工作号、个人 Max" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
            {claude.then(|| view! {
                <label class="field">"接入方式"
                    <select disabled=move || gate.with(|g| g.busy("form")) on:change=move |e| how.set(event_target_value(&e))>
                        <option value="token" selected=true>"粘贴长期 token（claude setup-token）"</option>
                        <option value="login">"在浏览器里登录"</option>
                    </select>
                </label>
            })}
            <Show when=move || claude && how.get() == "token" fallback=|| view! { <div class="muted small">"创建后会打开一个终端运行官方登录命令，按提示在浏览器里登录这个账号即可。"</div> }>
                <label class="field">"长期 token"
                    <input disabled=move || gate.with(|g| g.busy("form")) class="mono" type="password" autocomplete="off" spellcheck="false" placeholder="sk-ant-oat01-…" prop:value=move || token.get() on:input=move |e| token.set(event_target_value(&e))/>
                </label>
                <div class="muted small" inner_html=TOKEN_HELP></div>
            </Show>
        }.into_any(),
        Form::Token(_) => view! {
            <label class="field"><input disabled=move || gate.with(|g| g.busy("form")) class="mono" type="password" autocomplete="off" spellcheck="false" placeholder="sk-ant-oat01-…"
                prop:value=move || token.get() on:input=move |e| token.set(event_target_value(&e))
                on:keydown=move |e| if e.key() == "Enter" { submit2() }/></label>
            <div class="muted small" inner_html=format!("{TOKEN_HELP}{token_note}")></div>
        }.into_any(),
        Form::Plan(_) => view! {
            <label class="field">
                <select disabled=move || gate.with(|g| g.busy("form")) on:change=move |e| plan.set(event_target_value(&e))>
                    <option value="" selected=move || plan.get().is_empty()>"不标"</option>
                    {["pro", "max5x", "max20x", "team", "enterprise"].into_iter().map(|x| view! { <option value=x selected=move || plan.get() == x>{plan_label(x)}</option> }).collect_view()}
                </select>
            </label>
            <div class="muted small">"只用于展示；能用哪些模型以实际运行结果为准。"</div>
        }.into_any(),
    };
    view! {
        <div class="dlg-mask" on:click=move |_| close()>
            <div class="dlg" on:click=|e| e.stop_propagation()>
                <h3>{title}</h3>
                {body}
                <div class="dlg-foot">
                    <button class="btn" disabled=move || gate.with(|g| g.busy("form")) on:click=move |_| close()>"取消"</button>
                    <button class="btn primary" disabled=move || gate.with(|g| g.busy("form")) on:click=move |_| submit()>
                        {move || if gate.with(|g| g.busy("form")) { "保存中…".to_owned() } else if ok == "创建" && !(claude && how.get() == "token") { "创建并登录".to_owned() } else { ok.clone() }}
                    </button>
                </div>
            </div>
        </div>
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct AgentSpec {
    id: String,
    label: String,
    #[serde(default)]
    program: String,
    #[serde(default)]
    structured: bool,
    #[serde(default)]
    resume: bool,
}

#[component]
pub fn RuntimeDetailPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();
    move || {
        let id = id();
        view! { <RuntimeDetail id/> }
    }
}

#[component]
fn RuntimeDetail(id: String) -> impl IntoView {
    let bus = use_bus();
    let app = crate::app_state::use_app();
    let multi = WITH_ACCOUNTS.iter().any(|x| x.0 == id);
    let view_tab = RwSignal::new(if multi { "accounts" } else { "general" });
    let rev = RwSignal::new(0u32);
    let sid = id.clone();
    let spec = LocalResource::new(move || {
        let id = sid.clone();
        async move {
            api::get::<Vec<AgentSpec>>("/api/agents")
                .await
                .map(|l| l.into_iter().find(|a| a.id == id))
        }
    });
    let cid = id.clone();
    let configs = LocalResource::new(move || {
        rev.track();
        let id = cid.clone();
        async move {
            api::get::<Vec<AgentConfig>>("/api/agent-configs")
                .await
                .map(|l| {
                    l.into_iter()
                        .filter(|c| c.agent_id == id)
                        .collect::<Vec<_>>()
                })
        }
    });
    let accounts = LocalResource::new(move || {
        bus.accounts.track();
        api::get::<Accounts>("/api/accounts")
    });
    let nodes = move || {
        let mut v = vec![String::new()];
        v.extend(app.state.with(|s| {
            s.as_ref()
                .map(|s| s.nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        }));
        v
    };
    let id_s = StoredValue::new(id.clone());
    let node = RwSignal::new(String::new());
    let drafts = StoredValue::new(ConfigDrafts::default());
    let saving = RwSignal::new(false);
    let save = move |node: String, f: Box<dyn FnOnce(&mut AgentConfig)>| {
        if saving.get_untracked() {
            return;
        }
        let Some(Ok(mine)) = configs.get_untracked() else {
            toast("配置尚未加载，请重试后保存");
            return;
        };
        let mut c = mine
            .into_iter()
            .find(|c| c.node.clone().unwrap_or_default() == node)
            .unwrap_or(AgentConfig {
                max_concurrent: 1,
                enabled: true,
                ..AgentConfig::default()
            });
        c.agent_id = id_s.get_value();
        c.node = (!node.is_empty()).then_some(node.clone());
        c.enabled = true;
        f(&mut c);
        let section = view_tab.get_untracked();
        saving.set(true);
        spawn_local(async move {
            let body = json!({
                "node": c.node, "agent_id": c.agent_id, "program_path": c.program_path, "model": c.model,
                "permission_mode": c.permission_mode, "custom_args": c.custom_args, "custom_env": c.custom_env,
                "max_concurrent": c.max_concurrent.max(1), "enabled": true,
            });
            let result = api::send::<Value>("POST", "/api/agent-configs", &body).await;
            match result {
                Ok(_) => {
                    let _ = drafts.try_update_value(|drafts| drafts.saved(&node, section));
                    let _ = configs.try_set(None);
                    toast("已保存");
                    let _ = rev.try_update(|n| *n += 1);
                }
                Err(e) => toast(format!("保存失败：{e}")),
            }
            let _ = saving.try_set(false);
        });
    };
    let save = StoredValue::new_local(save);

    view! {
        <div class="page wide">
            <div class="page-head">
                <a href="/runtimes" class="crumb">"运行时"</a><span class="sep">"/"</span>
                {move || spec.get().and_then(Result::ok).flatten().map(|s| view! {
                    <h1>{format!("{} · 设置", s.label)}</h1>
                    <span class="gchip mono">{s.program.clone()}</span>
                    {(!s.structured).then(|| view! { <span class="gchip warn">"纯文本输出"</span> })}
                    {(!s.resume).then(|| view! { <span class="gchip">"不支持续接"</span> })}
                })}
            </div>
            <div class="vt-wrap">
                <nav class="vtabs">
                    {multi.then(|| view! { <button class="vtab" data-active=move || (view_tab.get() == "accounts").to_string() on:click=move |_| view_tab.set("accounts")>"账号"</button> })}
                    <button class="vtab" data-active=move || (view_tab.get() == "general").to_string() on:click=move |_| view_tab.set("general")>"配置"</button>
                    <button class="vtab" data-active=move || (view_tab.get() == "env").to_string() on:click=move |_| view_tab.set("env")>"环境变量"</button>
                    <button class="vtab" data-active=move || (view_tab.get() == "args").to_string() on:click=move |_| view_tab.set("args")>"自定义参数"</button>
                </nav>
                <div class="vt-body">
                    {move || {
                        let tab = view_tab.get();
                        if tab == "accounts" {
                            return view! {
                            <div class="card pad">
                                <h3>"账号"</h3>
                                <div class="muted small">
                                    "每个账号是一个独立的 CLI 配置目录。浏览器登录的账号由官方 CLI 自己保管凭据，Blazar 不读取；长期 token 存在账号目录里只有你能读的文件中，启动时才交给 CLI。点「使用」切换这个运行时用的账号；对话里也可以从运行时选择器单独换。设置、技能和会话历史在各账号之间共享，换账号后上下文不变。"
                                    {move || accounts.get().and_then(Result::ok).map(|a| format!(" 账号目录在 {}。", a.root))}
                                </div>
                            </div>
                            <section class="card"><AccountsSection provider=id_s.get_value() accounts/></section>
                        }.into_any();
                        }
                        let configs_value = match configs.get() {
                            None => return view! { <div class="muted">"加载配置…"</div> }.into_any(),
                            Some(Err(error)) => return view! {
                                <p class="err-line" role="alert">{error.to_string()}</p>
                                <button class="btn" on:click=move |_| rev.update(|n| *n += 1)>"重试"</button>
                            }.into_any(),
                            Some(Ok(configs)) => configs,
                        };
                        let selected_node = node.get();
                        let draft = RwSignal::new(drafts.with_value(|drafts| drafts.get(&configs_value, &selected_node, tab)));
                        let draft_key = StoredValue::new((selected_node, tab.to_owned()));
                        let change = Callback::new(move |(field, value): (DraftField, String)| {
                            draft.update(|draft| draft.set(field, value));
                            let (node, section) = draft_key.get_value();
                            drafts.update_value(|drafts| drafts.remember(node, section, draft.get_untracked()));
                        });
                        match tab {
                        "general" => {
                            let modes = crate::pages::workspace::chat::state::modes_for(&id_s.get_value());
                            let program = spec.get().and_then(Result::ok).flatten().map(|s| s.program).unwrap_or_default();
                            view! {
                                <div class="card pad">
                                    <h3>"配置"</h3>
                                    <div class="muted small">"节点级配置优先于全局默认。留空节点即为全局默认。"</div>
                                    <div class="grid2">
                                        <label class="field">"适用机器"<select disabled=move || saving.get() prop:value=move || node.get() on:change=move |e| node.set(event_target_value(&e))>
                                            {move || nodes().into_iter().map(|n| { let l = if n.is_empty() { "（全局默认）".to_owned() } else { n.clone() }; let selected = n == node.get_untracked(); view! { <option value=n selected=selected>{l}</option> } }).collect_view()}
                                        </select></label>
                                        <label class="field">"可执行文件路径（装在非标准位置时）"<input placeholder=program disabled=move || saving.get() prop:value=move || draft.get().path on:input=move |e| change.run((DraftField::Path, event_target_value(&e)))/></label>
                                        <label class="field">"模型"<input placeholder="默认" disabled=move || saving.get() prop:value=move || draft.get().model on:input=move |e| change.run((DraftField::Model, event_target_value(&e)))/></label>
                                        <label class="field">"最大并发"<input type="number" min="1" max="16" disabled=move || saving.get() prop:value=move || draft.get().concurrency on:input=move |e| change.run((DraftField::Concurrency, event_target_value(&e)))/></label>
                                    </div>
                                    <label class="field">"权限模式"
                                        <select disabled=move || saving.get() prop:value=move || draft.get().permission on:change=move |e| change.run((DraftField::Permission, event_target_value(&e)))>
                                            <option value="" selected=move || draft.get().permission.is_empty()>"默认"</option>
                                            {modes.iter().map(|m| view! { <option value=m.0 selected=move || draft.get().permission == m.0>{m.1}</option> }).collect_view()}
                                        </select>
                                        {modes.is_empty().then(|| view! { <span class="muted small">"这个运行时没有权限模式概念，设了也不会生效"</span> })}
                                    </label>
                                    <div class="row-actions"><button class="btn primary" disabled=move || saving.get() on:click=move |_| {
                                        let draft = draft.get_untracked();
                                        let Some(concurrency) = draft.concurrency.trim().parse::<i64>().ok().filter(|n| (1..=16).contains(n)) else {
                                            toast("最大并发需为 1 到 16 的整数");
                                            return;
                                        };
                                        save.with_value(|f| f(node.get_untracked(), Box::new(move |x| {
                                            x.program_path = Some(draft.path.trim().to_owned()).filter(|s| !s.is_empty());
                                            x.model = Some(draft.model.trim().to_owned()).filter(|s| !s.is_empty());
                                            x.permission_mode = Some(draft.permission).filter(|s| !s.is_empty());
                                            x.max_concurrent = concurrency;
                                        })));
                                    }>"保存"</button></div>
                                </div>
                                {move || configs.get().and_then(Result::ok).filter(|l| !l.is_empty()).map(|l| view! {
                                    <div class="card pad">
                                        <h3>"已有配置"</h3>
                                        <table class="tb">
                                            <thead><tr><th>"机器"</th><th>"路径"</th><th>"模型"</th><th>"并发"</th><th></th></tr></thead>
                                            <tbody>
                                                {l.into_iter().map(|c| {
                                                    let cid = c.id.clone();
                                                    view! {
                                                        <tr>
                                                            <td>{c.node.clone().unwrap_or_else(|| "（全局默认）".into())}</td>
                                                            <td class="mono">{c.program_path.clone().unwrap_or_else(|| "—".into())}</td>
                                                            <td class="mono">{c.model.clone().unwrap_or_else(|| "—".into())}</td>
                                                            <td class="mono">{c.max_concurrent}</td>
                                                            <td style="text-align:right"><button class="btn small danger" disabled=move || saving.get() on:click=move |_| {
                                                                let cid = cid.clone();
                                                                spawn_local(async move {
                                                                    match api::send::<Value>("DELETE", &format!("/api/agent-configs/{cid}"), &json!({})).await {
                                                                        Ok(_) => { toast("已删除"); rev.update(|n| *n += 1); }
                                                                        Err(e) => toast(format!("删除失败：{e}")),
                                                                    }
                                                                });
                                                            }>"删除"</button></td>
                                                        </tr>
                                                    }
                                                }).collect_view()}
                                            </tbody>
                                        </table>
                                    </div>
                                })}
                            }.into_any()
                        }
                        "env" => {
                            view! {
                                <div class="card pad">
                                    <h3>"环境变量"</h3>
                                    <div class="muted small">"注入 agent 进程。出口代理走这里 —— 连不上 AI API 的机器需要 HTTPS_PROXY。凭据请用机器本机的登录态，不要写在这里。"</div>
                                    <label class="field">"每行一个 KEY=value"<textarea class="mono" rows="8" placeholder="HTTPS_PROXY=http://127.0.0.1:9527" disabled=move || saving.get() prop:value=move || draft.get().env on:input=move |e| change.run((DraftField::Env, event_target_value(&e)))></textarea></label>
                                    <label class="field">"适用机器"<select disabled=move || saving.get() prop:value=move || node.get() on:change=move |e| node.set(event_target_value(&e))>
                                        {move || nodes().into_iter().map(|n| { let l = if n.is_empty() { "（全局默认）".to_owned() } else { n.clone() }; let sel = n == node.get_untracked(); view! { <option value=n selected=sel>{l}</option> } }).collect_view()}
                                    </select></label>
                                    <div class="row-actions"><button class="btn primary" disabled=move || saving.get() on:click=move |_| {
                                        let env: std::collections::BTreeMap<String, String> = draft.get_untracked().env.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
                                            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))).filter(|(k, _)| !k.is_empty()).collect();
                                        save.with_value(|f| f(node.get_untracked(), Box::new(move |x| x.custom_env = env)));
                                    }>"保存"</button></div>
                                </div>
                            }.into_any()
                        }
                        _ => {
                            view! {
                                <div class="card pad">
                                    <h3>"自定义参数"</h3>
                                    <div class="muted small">"追加到 CLI 命令行，每行一个。会插在提示词之前。"</div>
                                    <label class="field"><textarea class="mono" rows="6" placeholder="--verbose" disabled=move || saving.get() prop:value=move || draft.get().args on:input=move |e| change.run((DraftField::Args, event_target_value(&e)))></textarea></label>
                                    <label class="field">"适用机器"<select disabled=move || saving.get() prop:value=move || node.get() on:change=move |e| node.set(event_target_value(&e))>
                                        {move || nodes().into_iter().map(|n| { let l = if n.is_empty() { "（全局默认）".to_owned() } else { n.clone() }; let selected = n == node.get_untracked(); view! { <option value=n selected=selected>{l}</option> } }).collect_view()}
                                    </select></label>
                                    <div class="row-actions"><button class="btn primary" disabled=move || saving.get() on:click=move |_| {
                                        let args: Vec<String> = draft.get_untracked().args.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
                                        save.with_value(|f| f(node.get_untracked(), Box::new(move |x| x.custom_args = args)));
                                    }>"保存"</button></div>
                                </div>
                            }.into_any()
                        }
                    }}}
                </div>
            </div>
        </div>
    }
}

#[component]
fn GlobalSync(accounts: LocalResource<Result<Accounts, api::ApiError>>) -> impl IntoView {
    let bus = use_bus();
    let state = LocalResource::new(move || {
        bus.accounts.track();
        api::get::<Value>("/api/accounts/global")
    });
    let busy = RwSignal::new(false);
    let on = move || {
        state
            .get()
            .and_then(Result::ok)
            .is_some_and(|v| v["enabled"].as_bool() == Some(true))
    };
    let label_of = move |id: Option<&str>| -> String {
        match id {
            None => "原来的默认登录".into(),
            Some(id) => accounts
                .get()
                .and_then(Result::ok)
                .and_then(|a| a.accounts.into_iter().find(|x| x.id == id).map(|x| x.label))
                .unwrap_or_else(|| id.to_owned()),
        }
    };
    let toggle = move |_| {
        let enable = !on();
        spawn_local(async move {
            if enable {
                let body = "打开后，在 Blazar 里点「使用」切换账号时，这台电脑上终端里的 claude / codex 和 Cursor 的 Claude 插件也会换成同一个账号。\n\n\
                    · 会改：钥匙串里的 Claude Code 登录、~/.claude.json 里的账号信息、~/.codex/auth.json。项目设置、MCP 配置和登录不受影响。\n\
                    · 原来的登录会先备份；关掉这个开关就恢复原样。\n\
                    · 正在运行的 claude / codex 要重开一次才会用新账号。";
                if dialog::ask(
                    "同步到终端和 Cursor",
                    body,
                    vec![Choice::plain("取消"), Choice::plain("打开")],
                )
                .await
                    != Some(1)
                {
                    return;
                }
            }
            busy.set(true);
            match api::send::<Value>("PUT", "/api/accounts/global", &json!({ "enabled": enable }))
                .await
            {
                Ok(_) => toast(if enable {
                    "已打开：终端和 Cursor 现在跟着 Blazar 用同一个账号"
                } else {
                    "已关掉：终端和 Cursor 换回原来的登录"
                }),
                Err(e) => toast(format!("没切成：{e}")),
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <section class="card pad global-sync">
            <div class="card-title">
                <h3>"同步到终端和 Cursor"</h3>
                <span class=move || if on() { "gchip ok" } else { "gchip" }>{move || if on() { "已打开" } else { "关闭" }}</span>
                <span class="grow"></span>
                <button class=move || if on() { "btn" } else { "btn primary" } disabled=move || busy.get() on:click=toggle>
                    {move || if busy.get() { "切换中…" } else if on() { "关掉" } else { "打开" }}
                </button>
            </div>
            <div class="muted small">
                {move || if on() {
                    let v = state.get().and_then(Result::ok).unwrap_or(Value::Null);
                    format!("终端里的 claude 和 Cursor 插件现在用「{}」，codex 用「{}」。在下面点「使用」会一起换。",
                        label_of(v["current"]["claude"].as_str()), label_of(v["current"]["codex"].as_str()))
                } else {
                    "现在只换 Blazar 自己的对话。打开后，终端里的 claude / codex 和 Cursor 插件也跟着这里选的账号走。".to_owned()
                }}
            </div>
        </section>
    }
}
