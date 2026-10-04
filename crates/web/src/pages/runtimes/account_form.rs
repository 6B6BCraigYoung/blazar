use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api::{self, Account};
use crate::components::dialog::{self, Choice};
use crate::components::modal::Modal;
use crate::components::status::InlineError;
use crate::components::toast::toast;

use super::accounts::{plan_label, provider_label};
use super::{Form, login, patch};

const TOKEN_HELP: &str = "先登录对应账号，再运行 <span class=\"mono\">claude setup-token</span>，粘贴完整的 <span class=\"mono\">sk-ant-oat01-…</span>。每个账号单独生成。";

#[component]
pub(super) fn AccountForm(
    f: Form,
    provider: String,
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
                    Form::Add => {
                        let n = name.get_untracked().trim().to_owned();
                        if n.is_empty() { form_error.set(Some("请输入账号名称".into())); return Ok(()); }
                        let use_token = claude && how.get_untracked() == "token";
                        let t = token.get_untracked().trim().to_owned();
                        if use_token && t.is_empty() { form_error.set(Some("请粘贴 claude setup-token 生成的完整 token".into())); return Ok(()); }
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
                        if t.is_empty() { form_error.set(Some("请粘贴完整 token".into())); return Ok(()); }
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
                let _ = form_error.try_set(Some(format!("保存失败：{e}")));
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
            <label class="field">"账号名称"<input data-modal-initial-focus="" disabled=move || gate.with(|g| g.busy("form")) maxlength="40" placeholder="例如：工作账号" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
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
            <label class="field">"长期 token"<input data-modal-initial-focus="" disabled=move || gate.with(|g| g.busy("form")) class="mono" type="password" autocomplete="off" spellcheck="false" placeholder="sk-ant-oat01-…"
                prop:value=move || token.get() on:input=move |e| token.set(event_target_value(&e))
                on:keydown=move |e| if e.key() == "Enter" { submit2() }/></label>
            <div class="muted small" inner_html=format!("{TOKEN_HELP}{token_note}")></div>
        }.into_any(),
        Form::Plan(_) => view! {
            <label class="field">"订阅类型"
                <select data-modal-initial-focus="" disabled=move || gate.with(|g| g.busy("form")) on:change=move |e| plan.set(event_target_value(&e))>
                    <option value="" selected=move || plan.get().is_empty()>"不标"</option>
                    {["pro", "max5x", "max20x", "team", "enterprise"].into_iter().map(|x| view! { <option value=x selected=move || plan.get() == x>{plan_label(x)}</option> }).collect_view()}
                </select>
            </label>
            <div class="muted small">"只用于展示；能用哪些模型以实际运行结果为准。"</div>
        }.into_any(),
    };
    view! {
        <Modal label=title.clone() on_close=Callback::new(move |_| close())>
                <h3>{title}</h3>
                {body}
                {move || form_error.get().map(|message| view! { <InlineError message/> })}
                <div class="dlg-foot">
                    <button class="btn" disabled=move || gate.with(|g| g.busy("form")) on:click=move |_| close()>"取消"</button>
                    <button class="btn primary" disabled=move || gate.with(|g| g.busy("form")) on:click=move |_| submit()>
                        {move || if gate.with(|g| g.busy("form")) { "保存中…".to_owned() } else if ok == "创建" && !(claude && how.get() == "token") { "创建并登录".to_owned() } else { ok.clone() }}
                    </button>
                </div>
        </Modal>
    }
}
