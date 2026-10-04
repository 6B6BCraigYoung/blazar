use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::components::modal::Modal;
use crate::components::status::{InlineError, LoadingState};
use crate::components::toast::toast;
use crate::git_preferences::GitPreferences;

use super::{Form, Git};

#[component]
pub(super) fn GitForm(f: Form, git: Git, form: RwSignal<Option<Form>>) -> impl IntoView {
    if f == Form::Rename
        || (f == Form::Pr
            && !git
                .status
                .with_untracked(|s| s.as_ref().is_some_and(|s| s.gh)))
    {
        return view! { <GitFormReady f git form prefs=GitPreferences::from_values(None, None)/> }
            .into_any();
    }
    let revision = RwSignal::new(0u32);
    let prefs = LocalResource::new(move || {
        revision.track();
        api::get::<serde_json::Value>("/api/settings")
    });
    (move || match prefs.get() {
        Some(Ok(value)) => {
            let prefs = GitPreferences::from_values(
                value["git"]["pr_draft"].as_bool(),
                value["git"]["ai_draft"].as_bool(),
            );
            view! { <GitFormReady f=f.clone() git form prefs/> }.into_any()
        }
        state => view! {
            <Modal label="读取 Git 设置" class="dlg wide" on_close=Callback::new(move |_| form.set(None))>
                    <h3>"读取 Git 设置"</h3>
                    {match state {
                        Some(Err(error)) => view! {
                            <InlineError message=error.to_string() retry=Callback::new(move |_| revision.update(|n| *n = n.wrapping_add(1)))/>
                        }.into_any(),
                        _ => view! { <LoadingState text="读取 Git 设置…" class="muted"/> }.into_any(),
                    }}
                    <div class="dlg-foot"><button class="btn" on:click=move |_| form.set(None)>"取消"</button></div>
            </Modal>
        }.into_any(),
    }).into_any()
}

#[component]
fn GitFormReady(
    f: Form,
    git: Git,
    form: RwSignal<Option<Form>>,
    prefs: GitPreferences,
) -> impl IntoView {
    let g = git.status.get_untracked().unwrap_or_default();
    let title = RwSignal::new(match f {
        Form::Pr if g.commits.len() == 1 => g.commits[0].subject.clone(),
        Form::Rename => g.branch.clone(),
        _ => String::new(),
    });
    let body = RwSignal::new(String::new());
    let draft = RwSignal::new(prefs.draft_pr);
    let drafting = RwSignal::new(false);
    let gate = RwSignal::new(crate::action_gate::ActionGate::default());
    let close = move || {
        if !gate.with_untracked(|g| g.busy("form")) {
            form.set(None);
        }
    };
    let finish = move |success| {
        let mut completed = false;
        let _ = gate.try_update(|g| completed = g.complete("form", success));
        if completed {
            form.set(None);
        }
    };

    let ai = {
        let f = f.clone();
        move |_| {
            let kind = if f == Form::Pr { "pr" } else { "commit" };
            drafting.set(true);
            let ws = git.ws.get_value();
            spawn_local(async move {
                match api::send::<serde_json::Value>(
                    "POST",
                    &format!("/api/workspaces/{ws}/git/describe"),
                    &serde_json::json!({ "kind": kind }),
                )
                .await
                {
                    Ok(v) if kind == "pr" => {
                        title.set(v["title"].as_str().unwrap_or_default().to_owned());
                        body.set(v["body"].as_str().unwrap_or_default().to_owned());
                    }
                    Ok(v) => body.set(v["text"].as_str().unwrap_or_default().to_owned()),
                    Err(e) => toast(e.to_string()),
                }
                let _ = drafting.try_set(false);
            });
        }
    };

    let submit = {
        let f = f.clone();
        let g = g.clone();
        move || {
            if gate.with_untracked(|g| g.busy("form")) || git.busy.get_untracked().is_some() {
                return;
            }
            let f = f.clone();
            let g = g.clone();
            match f {
                Form::Commit => {
                    let message = body.get_untracked().trim().to_owned();
                    if message.is_empty() {
                        toast("写一句提交信息");
                        return;
                    }
                    gate.write().begin("form");
                    spawn_local(async move {
                        let result = git
                            .op("commit", serde_json::json!({ "message": message }))
                            .await;
                        finish(result.as_ref().is_some_and(|r| r.ok));
                        match result {
                            Some(r) if r.ok => toast(if r.changed {
                                format!("已提交 {}", r.commit.unwrap_or_default())
                            } else {
                                "没有改动可提交".to_owned()
                            }),
                            Some(_) => toast("提交失败，看下面的输出"),
                            None => {}
                        }
                    });
                }
                Form::Rename => {
                    let name = title.get_untracked().trim().to_owned();
                    if name.is_empty() || name == g.branch {
                        return;
                    }
                    gate.write().begin("form");
                    spawn_local(async move {
                        let r = git
                            .op("rename-branch", serde_json::json!({ "name": name }))
                            .await;
                        finish(r.as_ref().is_some_and(|r| r.ok));
                        toast(if r.is_some_and(|r| r.ok) {
                            "已改名（远端的旧分支不会自动删除）"
                        } else {
                            "改名失败"
                        });
                    });
                }
                Form::Pr => {
                    let t = title.get_untracked().trim().to_owned();
                    if !crate::git_policy::valid_pr_title(g.gh, &t) {
                        toast("写一个标题");
                        return;
                    }
                    let b = body.get_untracked();
                    let dr = draft.get_untracked();
                    gate.write().begin("form");
                    spawn_local(async move {
                        if !g.gh {
                            let r = git.op("push", serde_json::json!({})).await;
                            let url = git.status.with_untracked(|s| {
                                s.as_ref()
                                    .and_then(|s| s.web.as_ref())
                                    .and_then(|w| w.new_pr.clone())
                            });
                            finish(r.as_ref().is_some_and(|r| r.ok) && url.is_some());
                            match (r.is_some_and(|r| r.ok), url) {
                                (true, Some(u)) => {
                                    let _ = window().open_with_url_and_target(&u, "_blank");
                                }
                                (true, None) => toast("认不出托管平台，没法给网页链接"),
                                _ => toast("推送失败，看下面的输出"),
                            }
                            return;
                        }
                        let result = git
                            .op(
                                "pr-create",
                                serde_json::json!({ "title": t, "body": b, "draft": dr }),
                            )
                            .await;
                        finish(result.as_ref().is_some_and(|r| r.ok && r.url.is_some()));
                        match result {
                            Some(r) if r.ok && r.url.is_some() => {
                                toast(if r.existing {
                                    "这条分支已经有 PR 了"
                                } else {
                                    "PR 已创建"
                                });
                                if let Some(u) = r.url {
                                    let _ = window().open_with_url_and_target(&u, "_blank");
                                }
                            }
                            Some(_) => toast("PR 没开成，看下面的输出"),
                            None => {}
                        }
                    });
                }
            }
        }
    };
    let submit2 = submit.clone();

    let base = g.target.trim_start_matches("origin/").to_owned();
    let (head_title, ok_label) = match f {
        Form::Commit => ("提交改动", "提交"),
        Form::Pr => (
            if g.gh { "开 Pull Request" } else { "开 PR" },
            if g.gh {
                "创建 PR"
            } else {
                "推送并打开网页"
            },
        ),
        Form::Rename => ("新的分支名", "改名"),
    };
    let fields = match f {
        Form::Commit => view! {
            <div class="dlg-body small">{format!("{} 个文件的改动会全部提交到 {}（含未跟踪的新文件）", g.uncommitted, if g.branch.is_empty() { &g.head } else { &g.branch })}</div>
            <label class="field">"提交信息"
                <textarea disabled=move || gate.with(|g| g.busy("form")) rows="5" placeholder="改了什么、为什么" prop:value=move || body.get() on:input=move |e| body.set(event_target_value(&e))
                    on:keydown=move |e| if e.key() == "Enter" && (e.meta_key() || e.ctrl_key()) { e.prevent_default(); submit2(); }></textarea>
            </label>
        }.into_any(),
        Form::Pr if !g.gh => view! {
            <div class="dlg-body">"工作区所在的机器上没有装 GitHub CLI（gh），没法直接创建 PR。可以先把分支推上去，再到网页上开。"</div>
        }.into_any(),
        Form::Pr => view! {
            <div class="dlg-body small">
                {format!("{} → {} · {} 个提交", g.branch, base, g.ahead)}
                {(g.uncommitted > 0).then(|| view! { <span class="warn-tx">{format!(" · 还有 {} 个没提交的改动不会进 PR", g.uncommitted)}</span> })}
            </div>
            <label class="field">"标题"<input disabled=move || gate.with(|g| g.busy("form")) prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e)) placeholder="这个 PR 做了什么"/></label>
            <label class="field">"描述（Markdown）"<textarea disabled=move || gate.with(|g| g.busy("form")) rows="9" prop:value=move || body.get() on:input=move |e| body.set(event_target_value(&e)) placeholder="改了什么、为什么、怎么验证"></textarea></label>
            <label class="chk"><input disabled=move || gate.with(|g| g.busy("form")) type="checkbox" prop:checked=move || draft.get() on:change=move |_| draft.update(|d| *d = !*d)/>"作为草稿创建"</label>
            <div class="muted small">"会先把分支推到 origin，再用那台机器上 gh 的登录态创建 PR。"</div>
        }.into_any(),
        Form::Rename => view! {
            <label class="field">"分支名称"<input disabled=move || gate.with(|g| g.busy("form")) prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e))/></label>
        }.into_any(),
    };
    let show_ai = prefs.show_ai
        && match f {
            Form::Commit => true,
            Form::Pr => g.gh,
            Form::Rename => false,
        };
    view! {
        <Modal label=head_title class="dlg wide" on_close=Callback::new(move |_| close())>
                <h3>{head_title}</h3>
                {fields}
                <div class="dlg-foot">
                    {show_ai.then(|| view! {
                        <button class="btn" disabled=move || drafting.get() || gate.with(|g| g.busy("form")) title="让本机的 Claude（Haiku）看一眼改动，替你起草" on:click=ai.clone()>
                            {move || if drafting.get() { "起草中…" } else { "AI 起草" }}
                        </button>
                    })}
                    <span class="grow"></span>
                    <button class="btn" disabled=move || gate.with(|g| g.busy("form")) on:click=move |_| close()>"取消"</button>
                    <button class="btn primary" disabled=move || gate.with(|g| g.busy("form")) on:click=move |_| submit()>{move || if gate.with(|g| g.busy("form")) { "处理中…" } else { ok_label }}</button>
                </div>
        </Modal>
    }
}
