use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{self, Accounts, Runtimes};
use crate::components::dialog::{self, Choice};
use crate::components::page_head::PageHead;
use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow};
use crate::components::status::{Checking, InlineError, Status};
use crate::components::toast::{flash, toast};
use crate::realtime::use_bus;
use crate::rt_logo;
use crate::runtime_config::{AgentConfig, ConfigDraft, parse_args, parse_env};

use super::accounts::provider_label;
use super::{AccountsSection, Form, WITH_ACCOUNTS, active_account, runtime_status};

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
pub(super) fn RuntimeDetail(id: String) -> impl IntoView {
    let bus = use_bus();
    let app = crate::app_state::use_app();
    let multi = WITH_ACCOUNTS.iter().any(|x| x.0 == id);
    let rev = RwSignal::new(0u32);
    let checking = RwSignal::new(false);
    crate::refocus::on_refocus(move || rev.update(|n| *n += 1));
    let id_s = StoredValue::new(id.clone());
    let spec = LocalResource::new(move || {
        rev.track();
        let id = id_s.get_value();
        async move {
            api::get::<Vec<AgentSpec>>("/api/agents")
                .await
                .map(|l| l.into_iter().find(|a| a.id == id))
        }
    });
    let runtime = LocalResource::new(move || {
        rev.track();
        checking.set(true);
        let id = id_s.get_value();
        async move {
            let r = api::get::<Runtimes>("/api/runtimes")
                .await
                .map(|r| r.runtimes.into_iter().find(|x| x.id == id));
            checking.try_set(false);
            r
        }
    });
    let configs = LocalResource::new(move || {
        rev.track();
        let id = id_s.get_value();
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
    let known = RwSignal::new(Vec::<AgentConfig>::new());
    Effect::new(move |_| {
        if let Some(Ok(l)) = configs.get() {
            known.set(l);
        }
    });
    let accounts = LocalResource::new(move || {
        bus.accounts.track();
        rev.track();
        api::get::<Accounts>("/api/accounts")
    });
    let form = RwSignal::new(None::<Form>);
    let node = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let nodes = move || {
        let mut v = vec![String::new()];
        v.extend(app.state.with(|s| {
            s.as_ref()
                .map(|s| s.nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        }));
        v
    };
    let draft = Memo::new(move |_| ConfigDraft::for_node(&known.get(), &node.get()));
    let found = move || spec.get().and_then(Result::ok).flatten();
    let rt = move || runtime.get().and_then(Result::ok).flatten();
    let label = Signal::derive(move || {
        found()
            .map(|s| s.label)
            .unwrap_or_else(|| provider_label(&id_s.get_value()).to_owned())
    });
    let program = Signal::derive(move || found().map(|s| s.program).unwrap_or_default());
    let save = move |f: Box<dyn FnOnce(&mut AgentConfig)>| {
        if saving.get_untracked() {
            return;
        }
        let node = node.get_untracked();
        let mut c = known
            .get_untracked()
            .into_iter()
            .find(|c| c.node.clone().unwrap_or_default() == node)
            .unwrap_or(AgentConfig {
                max_concurrent: 2,
                enabled: true,
                ..AgentConfig::default()
            });
        c.agent_id = id_s.get_value();
        c.node = (!node.is_empty()).then_some(node);
        c.enabled = true;
        f(&mut c);
        saving.set(true);
        spawn_local(async move {
            let body = json!({
                "node": c.node, "agent_id": c.agent_id, "program_path": c.program_path, "model": c.model,
                "permission_mode": c.permission_mode, "custom_args": c.custom_args, "custom_env": c.custom_env,
                "max_concurrent": c.max_concurrent.max(1), "enabled": true,
            });
            match api::send::<Value>("POST", "/api/agent-configs", &body).await {
                Ok(_) => {
                    flash("已保存");
                    let _ = rev.try_update(|n| *n += 1);
                }
                Err(e) => toast(format!("没保存上：{e}")),
            }
            let _ = saving.try_set(false);
        });
    };
    let save = StoredValue::new_local(save);
    let remove = move |cid: String| {
        if saving.get_untracked() {
            return;
        }
        saving.set(true);
        spawn_local(async move {
            if dialog::ask(
                "删除配置",
                "删除这项已保存的配置？此操作无法撤销。",
                vec![Choice::plain("保留配置"), Choice::danger("删除配置")],
            )
            .await
                != Some(1)
            {
                let _ = saving.try_set(false);
                return;
            }
            match api::send::<Value>("DELETE", &format!("/api/agent-configs/{cid}"), &json!({}))
                .await
            {
                Ok(_) => {
                    toast("已删除");
                    let _ = rev.try_update(|n| *n += 1);
                }
                Err(e) => toast(format!("删除失败：{e}")),
            }
            let _ = saving.try_set(false);
        });
    };
    let current = move || {
        accounts
            .get()
            .and_then(Result::ok)
            .and_then(|a| active_account(&a, &id_s.get_value()))
    };
    let signed_out = move || current().is_some_and(|a| !a.disabled && a.status == "logged_out");
    let sign_in = move |_| {
        if let Some(a) = current() {
            if a.kind == "token" {
                form.set(Some(Form::Token(a)));
            } else {
                super::login(&a);
            }
        }
    };
    let modes = crate::pages::workspace::chat::state::modes_for(&id);
    let node_options = move || {
        nodes()
            .into_iter()
            .map(|n| {
                let l = if n.is_empty() {
                    "（全局默认）".to_owned()
                } else {
                    n.clone()
                };
                let selected = n == node.get_untracked();
                view! {
                    <option value=n selected=selected>
                        {l}
                    </option>
                }
            })
            .collect_view()
    };

    view! {
        <div class="page runtimes-page detail-page">
            <PageHead
                parent=("运行时", "/runtimes")
                icon=move || view! { <span inner_html=rt_logo::mark(&id_s.get_value())></span> }
                title=label
                status=move || {
                    view! {
                        {move || {
                            rt()
                                .map(|r| {
                                    let a = accounts
                                        .get()
                                        .and_then(Result::ok)
                                        .and_then(|a| active_account(&a, &id_s.get_value()));
                                    let (text, tone) = runtime_status(&r, a.as_ref());
                                    view! { <Status tone label=text title=a.map(|a| a.label) /> }
                                })
                        }}
                        <Checking busy=checking />
                    }
                }
            >
                {multi
                    .then(|| {
                        view! {
                            <Show when=signed_out>
                                <button
                                    type="button"
                                    class="btn primary"
                                    aria-haspopup="dialog"
                                    on:click=sign_in
                                >
                                    "登录…"
                                </button>
                            </Show>
                            <button
                                type="button"
                                class="btn"
                                class:primary=move || !signed_out()
                                aria-haspopup="dialog"
                                on:click=move |_| form.set(Some(Form::Add))
                            >
                                "添加账号"
                            </button>
                        }
                    })}
            </PageHead>
            {move || {
                spec.get()
                    .and_then(Result::err)
                    .map(|error| {
                        view! {
                            <InlineError
                                message=format!("无法加载运行时：{error}")
                                retry=Callback::new(move |_| rev.update(|n| *n += 1))
                            />
                        }
                    })
            }}
            <div class="detail-body">
                {multi
                    .then(|| {
                        view! {
                            <PrefGroup
                                title="账号"
                                note="每个账号是一份独立的登录身份。有多个账号时，可以在这里切换这个运行时用哪一个；对话里也可以从运行时选择器单独换。"
                            >
                                <div class="pref-list acc-list">
                                    <AccountsSection
                                        provider=id_s.get_value()
                                        accounts
                                        form
                                        head=false
                                    />
                                </div>
                                <PrefAdvanced title="账号与凭据">
                                    <p>
                                        "浏览器登录的账号由官方 CLI 自己保管凭据，Blazar 不读取；长期 token 存在账号目录里只有你能读的文件中，启动时才交给 CLI。设置、技能和会话历史在各账号之间共享，换账号后上下文不变。"
                                    </p>
                                    {move || {
                                        accounts
                                            .get()
                                            .and_then(Result::ok)
                                            .map(|a| {
                                                view! {
                                                    <PrefRow label="账号目录">
                                                        <code>{a.root}</code>
                                                    </PrefRow>
                                                }
                                            })
                                    }}
                                </PrefAdvanced>
                            </PrefGroup>
                        }
                    })}
                <PrefGroup
                    title="配置"
                    note="按机器单独设置，或保留全局默认。改动随手保存。"
                >
                    <PrefRow label="适用机器" for_id="rt-node">
                        <select
                            id="rt-node"
                            class="settings-input"
                            disabled=move || saving.get()
                            prop:value=move || node.get()
                            on:change=move |e| node.set(event_target_value(&e))
                        >
                            {node_options}
                        </select>
                    </PrefRow>
                    <PrefRow
                        label="程序路径"
                        detail="安装在自定义位置时填写。"
                        for_id="rt-path"
                    >
                        <input
                            id="rt-path"
                            class="settings-input"
                            autocomplete="off"
                            spellcheck="false"
                            placeholder=move || program.get()
                            disabled=move || saving.get()
                            prop:value=move || draft.get().path
                            on:change=move |e| {
                                let next = event_target_value(&e).trim().to_owned();
                                if next == draft.get_untracked().path {
                                    return;
                                }
                                save.with_value(|f| f(
                                    Box::new(move |x| {
                                        x.program_path = Some(next).filter(|s| !s.is_empty());
                                    }),
                                ));
                            }
                        />
                    </PrefRow>
                    <PrefRow
                        label="模型"
                        detail="留空用运行时自己的默认模型。"
                        for_id="rt-model"
                    >
                        <input
                            id="rt-model"
                            class="settings-input"
                            autocomplete="off"
                            spellcheck="false"
                            placeholder="默认"
                            disabled=move || saving.get()
                            prop:value=move || draft.get().model
                            on:change=move |e| {
                                let next = event_target_value(&e).trim().to_owned();
                                if next == draft.get_untracked().model {
                                    return;
                                }
                                save.with_value(|f| f(
                                    Box::new(move |x| {
                                        x.model = Some(next).filter(|s| !s.is_empty());
                                    }),
                                ));
                            }
                        />
                    </PrefRow>
                    <PrefRow label="权限模式" for_id="rt-permission">
                        {if modes.is_empty() {
                            view! {
                                <span class="pref-text">"此运行时不支持权限模式"</span>
                            }
                                .into_any()
                        } else {
                            view! {
                                <select
                                    id="rt-permission"
                                    class="settings-input"
                                    disabled=move || saving.get()
                                    prop:value=move || draft.get().permission
                                    on:change=move |e| {
                                        let next = event_target_value(&e);
                                        if next == draft.get_untracked().permission {
                                            return;
                                        }
                                        save.with_value(|f| f(
                                            Box::new(move |x| {
                                                x.permission_mode = Some(next).filter(|s| !s.is_empty());
                                            }),
                                        ));
                                    }
                                >
                                    <option
                                        value=""
                                        selected=move || draft.get().permission.is_empty()
                                    >
                                        "默认"
                                    </option>
                                    {modes
                                        .iter()
                                        .map(|m| {
                                            view! {
                                                <option
                                                    value=m.0
                                                    selected=move || draft.get().permission == m.0
                                                >
                                                    {m.1}
                                                </option>
                                            }
                                        })
                                        .collect_view()}
                                </select>
                            }
                                .into_any()
                        }}
                    </PrefRow>
                    <PrefRow
                        label="最大并发"
                        detail="1 到 16 个同时运行的会话。"
                        for_id="rt-concurrency"
                    >
                        <input
                            id="rt-concurrency"
                            class="settings-input"
                            type="number"
                            min="1"
                            max="16"
                            disabled=move || saving.get()
                            prop:value=move || draft.get().concurrency
                            on:change=move |e| {
                                let raw = event_target_value(&e);
                                let Some(n) = raw
                                    .trim()
                                    .parse::<i64>()
                                    .ok()
                                    .filter(|n| (1..=16).contains(n)) else {
                                    toast("最大并发需为 1 到 16 的整数");
                                    event_target::<web_sys::HtmlInputElement>(&e)
                                        .set_value(&draft.get_untracked().concurrency);
                                    return;
                                };
                                if n.to_string() == draft.get_untracked().concurrency {
                                    return;
                                }
                                save.with_value(|f| f(Box::new(move |x| x.max_concurrent = n)));
                            }
                        />
                    </PrefRow>
                    <PrefAdvanced title="高级配置" wide=true>
                        <PrefRow label="程序">
                            <code>{move || program.get()}</code>
                        </PrefRow>
                        {move || {
                            rt()
                                .map(|r| {
                                    view! {
                                        <PrefRow label="版本">
                                            <code>{r.version.unwrap_or_else(|| "—".into())}</code>
                                        </PrefRow>
                                        <PrefRow label="路径">
                                            <code>{r.path.unwrap_or_else(|| "—".into())}</code>
                                        </PrefRow>
                                        <PrefRow label="登录方式">
                                            <span class="pref-text">
                                                {r.auth_hint.unwrap_or_else(|| "—".into())}
                                            </span>
                                        </PrefRow>
                                    }
                                })
                        }}
                        {move || {
                            found()
                                .map(|s| {
                                    view! {
                                        <PrefRow label="输出">
                                            <span class="pref-text">
                                                {if s.structured { "结构化事件" } else { "纯文本" }}
                                            </span>
                                        </PrefRow>
                                        <PrefRow label="续接对话">
                                            <span class="pref-text">
                                                {if s.resume { "支持" } else { "不支持" }}
                                            </span>
                                        </PrefRow>
                                    }
                                })
                        }}
                        <label class="field">
                            "环境变量：每行一个 KEY=value，需要代理时可填写 HTTPS_PROXY。请勿在这里保存账号凭据。"
                            <textarea
                                class="mono"
                                rows="6"
                                placeholder="HTTPS_PROXY=http://127.0.0.1:9527"
                                disabled=move || saving.get()
                                prop:value=move || draft.get().env
                                on:change=move |e| {
                                    let env = parse_env(&event_target_value(&e));
                                    save.with_value(|f| f(Box::new(move |x| x.custom_env = env)));
                                }
                            ></textarea>
                        </label>
                        <label class="field">
                            "自定义参数：每行一个，启动时追加到命令行。"
                            <textarea
                                class="mono"
                                rows="4"
                                placeholder="--verbose"
                                disabled=move || saving.get()
                                prop:value=move || draft.get().args
                                on:change=move |e| {
                                    let args = parse_args(&event_target_value(&e));
                                    save.with_value(|f| f(Box::new(move |x| x.custom_args = args)));
                                }
                            ></textarea>
                        </label>
                        {move || {
                            let list = known.get();
                            (!list.is_empty())
                                .then(|| {
                                    view! {
                                        <p class="sub-h">"已保存的配置"</p>
                                        {list
                                            .into_iter()
                                            .map(|c| {
                                                let cid = c.id.clone();
                                                let detail = [
                                                    c.program_path.clone(),
                                                    c.model.clone(),
                                                    Some(format!("并发 {}", c.max_concurrent)),
                                                ]
                                                    .into_iter()
                                                    .flatten()
                                                    .collect::<Vec<_>>()
                                                    .join(" · ");
                                                view! {
                                                    <PrefRow
                                                        label=c
                                                            .node
                                                            .clone()
                                                            .unwrap_or_else(|| "（全局默认）".into())
                                                        detail=detail
                                                    >
                                                        <button
                                                            type="button"
                                                            class="btn danger"
                                                            disabled=move || saving.get()
                                                            on:click=move |_| remove(cid.clone())
                                                        >
                                                            "删除"
                                                        </button>
                                                    </PrefRow>
                                                }
                                            })
                                            .collect_view()}
                                    }
                                })
                        }}
                    </PrefAdvanced>
                </PrefGroup>
            </div>
        </div>
    }
}
