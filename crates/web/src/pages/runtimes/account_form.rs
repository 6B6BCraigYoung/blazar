use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api::{self, Account, EndpointPreset};
use crate::components::dialog::{self, Choice};
use crate::components::modal::Modal;
use crate::components::status::InlineError;
use crate::components::toast::toast;
use crate::model_pick::split_models;

use super::accounts::{plan_label, provider_label};
use super::{Form, login, patch};

const TOKEN_HELP: &str = "先登录对应账号，再运行 <span class=\"mono\">claude setup-token</span>，粘贴完整的 <span class=\"mono\">sk-ant-oat01-…</span>。每个账号单独生成。";

const KEY_HELP: &str =
    "新密钥会先连一次接口，接口认了才保存。密钥只存在本机这个账号的目录里，仅你可读。";

const ENDPOINT_HELP: &str = "密钥只存在本机这个账号的目录里，仅你可读。按量计费，不参与自动分摊；对话里选的 Claude 型号会换成这里的默认型号。";

#[component]
pub(super) fn AccountForm(
    f: Form,
    provider: String,
    #[prop(optional)] presets: Vec<EndpointPreset>,
    on_close: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let gate = RwSignal::new(crate::action_gate::ActionGate::default());
    let form_error = RwSignal::new(None::<String>);
    let name = RwSignal::new(String::new());
    let how = RwSignal::new("token".to_owned());
    let token = RwSignal::new(String::new());
    let plan = RwSignal::new(match &f {
        Form::Plan(a) => a.plan.clone().unwrap_or_default(),
        _ => String::new(),
    });
    let presets = StoredValue::new(presets);
    let preset = RwSignal::new(String::new());
    let base_url = RwSignal::new(String::new());
    let models = RwSignal::new(String::new());
    let default_model = RwSignal::new(String::new());
    let tested = RwSignal::new(None::<Result<String, String>>);
    let testing = RwSignal::new(false);
    let initial_values = StoredValue::new((
        name.get_untracked(),
        how.get_untracked(),
        token.get_untracked(),
        plan.get_untracked(),
    ));
    let dirty = RwSignal::new(false);
    let confirming_close = RwSignal::new(false);
    Effect::new(move |_| {
        dirty.set((name.get(), how.get(), token.get(), plan.get()) != initial_values.get_value())
    });
    super::agents::guard_unsaved(dirty);
    let close = move || {
        if gate.with_untracked(|g| g.busy("form")) || confirming_close.get_untracked() {
            return;
        }
        if (
            name.get_untracked(),
            how.get_untracked(),
            token.get_untracked(),
            plan.get_untracked(),
        ) == initial_values.get_value()
        {
            on_close();
            return;
        }
        confirming_close.set(true);
        spawn_local(async move {
            if dialog::ask(
                "放弃修改",
                "账号设置尚未保存。关闭后，本次输入将丢失。",
                vec![Choice::plain("继续编辑"), Choice::danger("放弃修改")],
            )
            .await
                == Some(1)
                && confirming_close.try_get_untracked().is_some()
            {
                on_close();
            }
            confirming_close.try_set(false);
        });
    };
    let fill = move |id: String| {
        let p = presets.with_value(|ps| ps.iter().find(|p| p.id == id).cloned());
        preset.set(id);
        tested.set(None);
        match p {
            Some(p) => {
                base_url.set(p.base_url);
                models.set(p.models.join(", "));
                default_model.set(p.default_model);
            }
            None => {
                base_url.set(String::new());
                models.set(String::new());
                default_model.set(String::new());
            }
        }
    };
    let pick_how = move |v: String| {
        if v == "endpoint"
            && preset.get_untracked().is_empty()
            && base_url.get_untracked().is_empty()
        {
            fill(presets.with_value(|ps| ps.first().map(|p| p.id.clone()).unwrap_or_default()));
        }
        how.set(v);
    };
    let test = move || {
        if testing.get_untracked() {
            return;
        }
        let key = token.get_untracked().trim().to_owned();
        if key.is_empty() {
            tested.set(Some(Err("先填密钥".into())));
            return;
        }
        testing.set(true);
        tested.set(None);
        let body = json!({
            "endpoint": { "base_url": base_url.get_untracked(), "preset": preset.get_untracked() },
            "key": key,
        });
        spawn_local(async move {
            let out = match api::send::<Value>("POST", "/api/accounts/endpoint-test", &body).await {
                Ok(v) => {
                    let found: Vec<String> = v["models"]
                        .as_array()
                        .map(|l| {
                            l.iter()
                                .filter_map(|m| m.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    if !found.is_empty() && models.get_untracked().trim().is_empty() {
                        let _ = models.try_set(found.join(", "));
                    }
                    match v["status"].as_str() {
                        Some("ok") if found.is_empty() => Ok("能连上".to_owned()),
                        Some("ok") => Ok(format!("能连上 · 接口提供 {}", found.join("、"))),
                        _ => Err(v["error"].as_str().unwrap_or("连不上接口").to_owned()),
                    }
                }
                Err(e) => Err(crate::vocab::lead("测试失败", e.explained()).0),
            };
            let _ = tested.try_set(Some(out));
            let _ = testing.try_set(false);
        });
    };
    let claude = provider == "claude";
    let f2 = f.clone();
    let p2 = provider.clone();
    let submit = move || {
        if !gate.write().begin("form") {
            return;
        }
        form_error.set(None);
        let f = f2.clone();
        let provider = p2.clone();
        spawn_local(async move {
            let mut saved = false;
            let r: Result<(), api::ApiError> = async {
                match &f {
                    Form::Add if claude && how.get_untracked() == "endpoint" => {
                        let key = token.get_untracked().trim().to_owned();
                        if key.is_empty() { form_error.set(Some("请填写密钥".into())); return Ok(()); }
                        let list = split_models(&models.get_untracked());
                        if list.is_empty() { form_error.set(Some("至少填一个型号".into())); return Ok(()); }
                        let label = match name.get_untracked().trim() {
                            "" => presets.with_value(|ps| ps.iter().find(|p| p.id == preset.get_untracked()).map(|p| p.label.clone())).unwrap_or_else(|| "自定义接口".into()),
                            n => n.to_owned(),
                        };
                        let a: Account = api::send("POST", "/api/accounts", &json!({
                            "provider": provider,
                            "label": label,
                            "endpoint": {
                                "base_url": base_url.get_untracked(),
                                "models": list,
                                "default_model": default_model.get_untracked(),
                                "preset": preset.get_untracked(),
                            },
                            "key": key,
                        })).await?;
                        saved = true;
                        toast(if a.status == "ok" { format!("「{}」已添加，能连上", a.label) } else { format!("「{}」已添加，但暂时没连上接口", a.label) });
                    }
                    Form::Add => {
                        let n = name.get_untracked().trim().to_owned();
                        if n.is_empty() { form_error.set(Some("请输入账号名称".into())); return Ok(()); }
                        let use_token = claude && how.get_untracked() == "token";
                        let t = token.get_untracked().trim().to_owned();
                        if use_token && t.is_empty() { form_error.set(Some("请粘贴 claude setup-token 生成的完整 token".into())); return Ok(()); }
                        let a: Account = api::send("POST", "/api/accounts", &json!({ "provider": provider, "label": n, "token": if use_token { Some(t) } else { None } })).await?;
                        saved = true;
                        if use_token {
                            toast(if a.status == "ok" { format!("「{}」已添加，第一次使用时会验证 token", a.label) } else { format!("「{}」已添加，但 CLI 没认出这个 token", a.label) });
                        } else {
                            login(&a);
                        }
                    }
                    Form::Token(a) => {
                        let t = token.get_untracked().trim().to_owned();
                        let ep = a.kind == "endpoint";
                        if t.is_empty() { form_error.set(Some(if ep { "请填写新密钥" } else { "请粘贴完整 token" }.into())); return Ok(()); }
                        let r = api::send::<Value>("PUT", &format!("/api/accounts/{}/token", api::enc(&a.id)), &json!({ "token": t })).await?;
                        saved = true;
                        toast(match (ep, r["status"] == "ok") {
                            (true, true) => "已保存，能连上",
                            (true, false) => "已保存，但暂时没连上接口",
                            (false, true) => "已保存，第一次使用时会验证 token",
                            (false, false) => "已保存，但 CLI 没认出这个 token",
                        });
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
                let _ = form_error.try_set(Some(crate::vocab::lead("保存失败", e.explained()).0));
            }
            if completed {
                on_close();
            }
        });
    };
    let submit2 = submit.clone();
    let adding = matches!(f, Form::Add);
    let (title, ok) = match &f {
        Form::Add => (
            format!("给 {} 添加账号", provider_label(&provider)),
            "创建".to_owned(),
        ),
        Form::Token(a) if a.kind == "endpoint" => {
            (format!("更换密钥 · {}", a.label), "保存".to_owned())
        }
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
    let busy = move || gate.with(|g| g.busy("form"));
    let endpoint_fields = move || {
        let custom = preset.get().is_empty();
        let choices = presets.get_value();
        let options = split_models(&models.get());
        view! {
            <label class="field">
                "服务商" <select disabled=busy on:change=move |e| fill(event_target_value(&e))>
                    {choices
                        .into_iter()
                        .map(|p| {
                            let on = preset.get_untracked() == p.id;
                            view! {
                                <option value=p.id.clone() selected=on>
                                    {p.label.clone()}
                                </option>
                            }
                        })
                        .collect_view()}
                    <option value="" selected=custom>
                        "其他 Anthropic 兼容接口"
                    </option>
                </select>
            </label>
            <label class="field">
                "接口地址"
                <input
                    class="mono"
                    disabled=move || busy() || !preset.get().is_empty()
                    spellcheck="false"
                    autocomplete="off"
                    placeholder="https://…/anthropic"
                    prop:value=move || base_url.get()
                    on:input=move |e| {
                        base_url.set(event_target_value(&e));
                        tested.set(None);
                    }
                />
            </label>
            <div class="field">
                "密钥" <div class="browse-bar">
                    <input
                        aria-label="密钥"
                        class="mono"
                        type="password"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="sk-…"
                        disabled=busy
                        prop:value=move || token.get()
                        on:input=move |e| {
                            token.set(event_target_value(&e));
                            tested.set(None);
                        }
                    />
                    <button
                        type="button"
                        class="btn"
                        disabled=move || busy() || testing.get()
                        on:click=move |_| test()
                    >
                        {move || if testing.get() { "测试中…" } else { "测试" }}
                    </button>
                </div>
            </div>
            {move || {
                let (tone, text) = match tested.get() {
                    _ if testing.get() => ("", "正在连接接口…".to_owned()),
                    Some(Ok(text)) => ("", format!("✓ {text}")),
                    Some(Err(message)) => ("bad", message),
                    None => ("", "填好密钥后可以先测一下能不能连上".to_owned()),
                };
                let hint = text.clone();
                view! {
                    <div class="muted small reach-line" data-tone=tone title=hint>
                        {text}
                    </div>
                }
            }}
            <label class="field">
                "型号"
                <input
                    class="mono"
                    disabled=busy
                    spellcheck="false"
                    autocomplete="off"
                    placeholder="用逗号隔开，例如 deepseek-v4-pro, deepseek-flash"
                    prop:value=move || models.get()
                    on:input=move |e| models.set(event_target_value(&e))
                />
            </label>
            <label class="field">
                "默认型号"
                <select disabled=busy on:change=move |e| default_model.set(event_target_value(&e))>
                    {options
                        .into_iter()
                        .map(|m| {
                            let on = default_model.get_untracked() == m;
                            view! {
                                <option value=m.clone() selected=on>
                                    {m.clone()}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </label>
            <div class="muted small">{ENDPOINT_HELP}</div>
        }
    };
    let body = match f {
        Form::Add => view! {
            <label class="field">
                "账号名称"
                <input
                    data-modal-initial-focus=""
                    disabled=busy
                    maxlength="40"
                    placeholder=move || {
                        if how.get() == "endpoint" {
                            presets
                                .with_value(|ps| {
                                    ps.iter()
                                        .find(|p| p.id == preset.get())
                                        .map(|p| format!("默认叫「{}」", p.label))
                                })
                                .unwrap_or_else(|| "例如：公司网关".into())
                        } else {
                            "例如：工作账号".into()
                        }
                    }
                    prop:value=move || name.get()
                    on:input=move |e| name.set(event_target_value(&e))
                />
            </label>
            {claude
                .then(|| {
                    view! {
                        <label class="field">
                            "接入方式"
                            <select
                                disabled=busy
                                on:change=move |e| pick_how(event_target_value(&e))
                            >
                                <option value="token" selected=true>
                                    "粘贴长期 token（claude setup-token）"
                                </option>
                                <option value="login">"在浏览器里登录"</option>
                                <option value="endpoint">
                                    "自定义接口（DeepSeek 等兼容接口）"
                                </option>
                            </select>
                        </label>
                    }
                })}
            {move || match how.get().as_str() {
                "endpoint" if claude => endpoint_fields().into_any(),
                "token" if claude => {
                    view! {
                        <label class="field">
                            "长期 token"
                            <input
                                disabled=busy
                                class="mono"
                                type="password"
                                autocomplete="off"
                                spellcheck="false"
                                placeholder="sk-ant-oat01-…"
                                prop:value=move || token.get()
                                on:input=move |e| token.set(event_target_value(&e))
                            />
                        </label>
                        <div class="muted small" inner_html=TOKEN_HELP></div>
                    }
                        .into_any()
                }
                _ => {
                    view! {
                        <div class="muted small">
                            "创建后会打开一个终端运行官方登录命令，按提示在浏览器里登录这个账号即可。"
                        </div>
                    }
                        .into_any()
                }
            }}
        }.into_any(),
        Form::Token(a) => {
            let ep = a.kind == "endpoint";
            let help = if ep { KEY_HELP.to_owned() } else { format!("{TOKEN_HELP}{token_note}") };
            view! {
                <label class="field">
                    {if ep { "新密钥" } else { "长期 token" }}
                    <input
                        data-modal-initial-focus=""
                        disabled=busy
                        class="mono"
                        type="password"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder=if ep { "sk-…" } else { "sk-ant-oat01-…" }
                        prop:value=move || token.get()
                        on:input=move |e| token.set(event_target_value(&e))
                        on:keydown=move |e| {
                            if e.key() == "Enter" {
                                submit2()
                            }
                        }
                    />
                </label>
                <div class="muted small" inner_html=help></div>
            }.into_any()
        }
        Form::Plan(_) => view! {
            <label class="field">
                "订阅类型"
                <select
                    data-modal-initial-focus=""
                    disabled=busy
                    on:change=move |e| plan.set(event_target_value(&e))
                >
                    <option value="" selected=move || plan.get().is_empty()>
                        "不标"
                    </option>
                    {["pro", "max5x", "max20x", "team", "enterprise"]
                        .into_iter()
                        .map(|x| {
                            view! {
                                <option value=x selected=move || plan.get() == x>
                                    {plan_label(x)}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </label>
            <div class="muted small">
                "只用于展示；能用哪些模型以实际运行结果为准。"
            </div>
        }.into_any(),
    };
    view! {
        <Modal label=title.clone() on_close=Callback::new(move |_| close())>
            <h3>{title}</h3>
            {body}
            {move || form_error.get().map(|message| view! { <InlineError message /> })}
            <div class="dlg-foot">
                <button class="btn" disabled=busy on:click=move |_| close()>
                    "取消"
                </button>
                <button class="btn primary" disabled=busy on:click=move |_| submit()>
                    {move || {
                        if busy() {
                            "保存中…".to_owned()
                        } else if adding
                            && !(claude && matches!(how.get().as_str(), "token" | "endpoint"))
                        {
                            "创建并登录".to_owned()
                        } else {
                            ok.clone()
                        }
                    }}
                </button>
            </div>
        </Modal>
    }
}
