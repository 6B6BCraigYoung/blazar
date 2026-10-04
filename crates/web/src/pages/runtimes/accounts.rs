use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use crate::api::{self, Account};
use crate::components::dialog::{self, Choice};
use crate::components::menu::menu_keydown;
use crate::components::toast::toast;
use crate::fmt;

use super::{Form, WITH_ACCOUNTS, login, patch};

pub(super) fn provider_label(p: &str) -> &str {
    WITH_ACCOUNTS.iter().find(|x| x.0 == p).map_or(p, |x| x.1)
}

pub(super) fn plan_label(p: &str) -> &str {
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

fn status(a: &Account) -> impl IntoView + use<> {
    let tok = a.kind == "token";
    let (cls, text, title) = if a.disabled {
        ("state muted", "已停用", "")
    } else if a.status == "ok" {
        (
            "state ok",
            if tok { "已配置" } else { "已登录" },
            if tok {
                "首次运行时验证凭据；失效后会提示重新配置"
            } else {
                ""
            },
        )
    } else if a.status == "logged_out" {
        ("state bad", if tok { "需更新" } else { "未登录" }, "")
    } else {
        ("state muted", "待检查", "暂未获取登录状态")
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
        return view! { <span class="muted small" title=hint>"暂无数据"</span> }.into_any();
    }
    let summary = a
        .windows
        .first()
        .map(|w| {
            format!(
                "{} · 已用 {:.0}%",
                fmt::window_label(&w.name),
                (w.utilization * 100.0).clamp(0.0, 100.0)
            )
        })
        .unwrap_or_default();
    let bars = a
        .windows
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
        .collect_view();
    view! { <details class="runtime-quota"><summary>{summary}</summary><div class="runtime-quota-bars">{bars}</div></details> }.into_any()
}

#[component]
pub(super) fn AccountRow(
    a: Account,
    active: bool,
    menu: RwSignal<Option<String>>,
    form: RwSignal<Option<Form>>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let menu_root = NodeRef::<leptos::html::Div>::new();
    let menu_button = NodeRef::<leptos::html::Button>::new();
    let close_menu = Callback::new(move |_| {
        menu.set(None);
        if let Some(button) = menu_button.get_untracked() {
            let _ = button.focus();
        }
    });
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
        if busy.get_untracked() {
            return;
        }
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
            <button role="menuitem" class:danger=danger disabled=move || busy.get() on:click=move |_| {
                if busy.get_untracked() { return; }
                close_menu.run(());
                let a = a_menu.get_value();
                match act {
                    "login" => login(&a),
                    "token" => form.set(Some(Form::Token(a))),
                    "plan" => form.set(Some(Form::Plan(a))),
                    _ => { busy.set(true); spawn_local(async move {
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
                        let _ = busy.try_set(false);
                    }); },
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
                                    <button class="x" title="清除，下次照常尝试" aria-label=format!("重新尝试 {}", model_label(&b.model)) on:click=move |_| {
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
                            title=(!usable).then_some("请先登录或更新账号凭据")>{move || if busy.get() { "切换中…" } else { "使用" }}</button>
                    }.into_any()
                }}
                <span class="more">
                    <button class="btn ghost" node_ref=menu_button aria-label=format!("{}的更多操作", a.label) aria-haspopup="menu" aria-expanded=mine.clone() disabled=move || busy.get() on:click=open_menu>"⋯"</button>
                    <Show when=mine.clone()>
                        <div class="menu" node_ref=menu_root role="menu" aria-label="账号操作" on:keydown=move |event| { if let Some(root) = menu_root.get_untracked() { menu_keydown(&event, root.unchecked_ref(), close_menu); } }>
                            {(!tok).then(|| item(if a.status == "ok" { "重新登录" } else { "登录" }, false, "login"))}
                            {(tok || a.provider == "codex").then(|| item("刷新额度", false, "quota"))}
                            {(a.provider == "claude").then(|| item(if tok { "更换 token" } else { "改用长期 token" }, false, "token"))}
                            {tok.then(|| item("订阅类型", false, "plan"))}
                            {item("重命名", false, "rename")}
                            {item(if a.disabled { "启用" } else { "停用" }, false, "toggle")}
                            {(!a.builtin).then(|| view! { <div class="menu-sep"></div> })}
                            {(!a.builtin).then(|| item("删除账号", true, "delete"))}
                        </div>
                    </Show>
                </span>
            </span>
        </div>
    }
}
