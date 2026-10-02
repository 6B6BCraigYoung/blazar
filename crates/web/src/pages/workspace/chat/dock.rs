//! 停在输入框上方的审批 / 提问卡片（跟 Claude Code 插件一样）。
//! 审批：1 是 / 2 是，以后不再问 / 3 否，加一个一直在的说明框；数字键直接选，↑↓ 换焦点，回车选焦点那项，Esc 等于「否」。
//! 提问：一题一页，顶上是各题的标签；单选选完自动翻到下一题；每题最后一个是「其他」；Esc 不回答。

use std::collections::BTreeSet;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use crate::chat_model::{self, Extra, Pending};
use crate::components::toast::toast;

use super::log::Md;
use super::state::Chat;

const CHEVRON: &str = r#"<svg viewBox="0 0 16 16"><path d="M6 4l4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#;

fn focused_is_text() -> bool {
    document().active_element().is_some_and(|a| {
        a.closest("input, textarea, [contenteditable]")
            .ok()
            .flatten()
            .is_some()
    })
}

/// 卡片出现半秒后把焦点给第一个选项（不抢正在打字的输入框）。
fn focus_first_later(root: NodeRef<html::Div>, sel: &'static str) {
    gloo_timers::callback::Timeout::new(500, move || {
        if focused_is_text() {
            return;
        }
        if let Some(r) = root.try_get_untracked().flatten()
            && let Ok(Some(b)) = r.query_selector(sel)
            && let Ok(b) = b.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = b.focus();
        }
    })
    .forget();
}

fn opts_of(root: &web_sys::HtmlElement, sel: &str) -> Vec<web_sys::HtmlElement> {
    let Ok(list) = root.query_selector_all(sel) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i)?.dyn_into::<web_sys::HtmlElement>().ok())
        .collect()
}

fn ime(e: &ev::KeyboardEvent) -> bool {
    e.is_composing() || e.key_code() == 229
}

#[component]
pub fn Dock(chat: Chat) -> impl IntoView {
    let top = Memo::new(move |_| {
        chat.transcript.with(|t| {
            t.pending
                .iter()
                .find(|p| !chat.decided.with(|d| d.contains(&p.id)))
                .cloned()
        })
    });
    move || {
        top.get().map(|p| {
            if p.ask {
                view! { <AskCard chat p/> }.into_any()
            } else {
                view! { <ApprovalCard chat p/> }.into_any()
            }
        })
    }
}

#[component]
fn ApprovalCard(chat: Chat, p: Pending) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let folded = RwSignal::new(false);
    let reject = RwSignal::new(String::new());
    let r = p.request.clone();
    let input = r.get("input").cloned().unwrap_or(Value::Null);
    let tn = chat_model::tool_name(r["tool_name"].as_str().unwrap_or(""));
    let plan = tn == "ExitPlanMode";
    let root_path = chat.root.get_value();
    let remote = chat.remote().then(|| chat.node.get_value());
    let title = if plan {
        "按这个计划开始？".to_owned()
    } else {
        chat_model::approval_title(&r, &root_path, remote.as_deref())
    };
    let ws = chat.ws_id();

    let id = p.id.clone();
    let id_no = p.id.clone();
    let no = move || {
        chat.decide(
            id_no.clone(),
            false,
            reject.get_untracked().trim().to_owned(),
            None,
        )
    };
    let no2 = no.clone();
    let yes = {
        let id = id.clone();
        move |mode: Option<&'static str>| {
            if let Some(m) = mode {
                chat.set_mode(m);
            }
            chat.decide(id.clone(), true, String::new(), None);
        }
    };
    let (labels, ph): (Vec<&'static str>, &'static str) = if plan {
        (
            vec!["是，并自动批准改动", "是，改动前逐个确认", "否，继续规划"],
            "告诉 agent 计划要怎么改（回车发送）",
        )
    } else {
        (
            vec!["是", "是，并且这个工作区以后不再询问", "否"],
            "告诉 agent 要怎么做（回车发送）",
        )
    };
    let pick = {
        let yes = yes.clone();
        let req = r.clone();
        let id = id.clone();
        move |i: usize| match (plan, i) {
            (true, 0) => yes(Some("acceptEdits")),
            (true, 1) => yes(Some("default")),
            (false, 0) => yes(None),
            (false, 1) => chat.allow_always(id.clone(), req.clone()),
            _ => no(),
        }
    };
    let pick2 = pick.clone();

    let keys = move |e: ev::KeyboardEvent| {
        let Some(card) = root.try_get_untracked().flatten() else {
            return;
        };
        let card: web_sys::HtmlElement = card.unchecked_into();
        let opts = opts_of(&card, ".aopts > .aopt");
        let inp: Option<web_sys::HtmlElement> = card
            .query_selector(".areject")
            .ok()
            .flatten()
            .and_then(|x| x.dyn_into().ok());
        let mut items = opts.clone();
        if let Some(i) = &inp {
            items.push(i.clone());
        }
        let active = document().active_element();
        let pos = |a: &Option<web_sys::Element>| {
            a.as_ref()
                .and_then(|a| items.iter().position(|x| x.is_same_node(Some(a))))
        };
        let in_text = inp
            .as_ref()
            .is_some_and(|i| active.as_ref().is_some_and(|a| i.is_same_node(Some(a))));
        let step = |d: i32| {
            let n = items.len() as i32;
            let i = pos(&active).map_or(0, |i| (i as i32 + d).rem_euclid(n));
            let _ = items[i as usize].focus();
        };
        if in_text {
            match e.key().as_str() {
                "Enter" if !e.shift_key() && !ime(&e) => {
                    e.prevent_default();
                    no2();
                }
                "Escape" => {
                    e.prevent_default();
                    e.stop_propagation();
                    no2();
                }
                "ArrowUp" | "ArrowDown" if !reject.get_untracked().contains('\n') => {
                    e.prevent_default();
                    step(if e.key() == "ArrowDown" { 1 } else { -1 });
                }
                _ => {}
            }
            return;
        }
        let k = e.key();
        if let Ok(n) = k.parse::<usize>()
            && (1..=opts.len()).contains(&n)
        {
            e.prevent_default();
            pick2(n - 1);
        } else if k == "Escape" {
            e.prevent_default();
            e.stop_propagation();
            no2();
        } else if k == "ArrowDown" || k == "ArrowUp" {
            e.prevent_default();
            step(if k == "ArrowDown" { 1 } else { -1 });
        } else if k == "Enter" && pos(&active).is_none_or(|i| i >= opts.len()) {
            e.prevent_default();
            pick2(0);
        }
    };
    focus_first_later(root, ".aopt");

    let body = if plan {
        let plan_text = input["plan"].as_str().unwrap_or("").to_owned();
        view! { <div class="aplan"><Md text=plan_text ws/></div> }.into_any()
    } else {
        let what = chat_model::approval_what(&r, &root_path);
        let diff = match tn.as_str() {
            "Edit" | "MultiEdit" => chat_model::edit_diff(&input, &root_path),
            "Write" => chat_model::write_preview(&input),
            _ => Extra::None,
        };
        let desc = r["description"]
            .as_str()
            .filter(|d| !d.is_empty() && *d != what && chat_model::rel_path(d, &root_path) != what)
            .map(str::to_owned);
        let blocked = r["blocked_path"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(str::to_owned);
        view! {
            <pre class="acmd">{what}</pre>
            {match diff {
                Extra::Diff { rows, more } => Some(view! {
                    <div class="cc-diff">
                        {rows.into_iter().map(|(c, l)| view! { <div class=match c { '+' => "dadd", '-' => "ddel", _ => "dgap" }><span class="dm">{c.to_string()}</span>{l}</div> }).collect_view()}
                        {(more > 0).then(|| view! { <div class="dgap">{format!("… 还有 {more} 行")}</div> })}
                    </div>
                }),
                _ => None,
            }}
            {desc.map(|d| view! { <div class="adesc">{d}</div> })}
            {blocked.map(|b| view! { <div class="adesc">{format!("涉及路径：{b}")}</div> })}
        }.into_any()
    };

    view! {
        <div class="cc-appr" node_ref=root tabindex="0" data-folded=move || folded.get().to_string() on:keydown=keys>
            <div class="ahd">
                <span class="ah">{title}</span>
                <button class="afold" title="收起 / 展开" inner_html=CHEVRON on:click=move |_| folded.update(|f| *f = !*f)></button>
            </div>
            <div class="abody">{body}</div>
            <div class="aopts">
                {labels.into_iter().enumerate().map(|(i, l)| {
                    let pick = pick.clone();
                    view! { <button class="aopt" class:primary={i == 0} on:click=move |_| pick(i)><span class="ak">{i + 1}</span>{l}</button> }
                }).collect_view()}
                <input class="areject" placeholder=ph prop:value=move || reject.get() on:input=move |e| reject.set(event_target_value(&e))/>
            </div>
            <div class="ahint">"Esc 取消 · ↑↓ 选择 · 回车确认"</div>
        </div>
    }
}

#[derive(Clone)]
struct Question {
    header: String,
    question: String,
    multi: bool,
    options: Vec<(String, String)>,
}

#[component]
fn AskCard(chat: Chat, p: Pending) -> impl IntoView {
    let qs: Vec<Question> = p.request["input"]["questions"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, q)| Question {
                    header: q["header"]
                        .as_str()
                        .filter(|h| !h.is_empty())
                        .map_or_else(|| format!("问题 {}", i + 1), str::to_owned),
                    question: q["question"].as_str().unwrap_or("").to_owned(),
                    multi: q["multiSelect"].as_bool().unwrap_or(false),
                    options: q["options"]
                        .as_array()
                        .map(|o| {
                            o.iter()
                                .map(|x| {
                                    (
                                        x["label"].as_str().unwrap_or("").to_owned(),
                                        x["description"].as_str().unwrap_or("").to_owned(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let n = qs.len();
    let qs = StoredValue::new(qs);
    let cur = RwSignal::new(0usize);
    // 每题选了哪些（下标等于选项数时是「其他」）
    let picked = RwSignal::new(vec![BTreeSet::<usize>::new(); n]);
    let other = RwSignal::new(vec![String::new(); n]);
    let folded = RwSignal::new(false);
    let root = NodeRef::<html::Div>::new();
    let id = p.id.clone();

    let go = move |i: usize| {
        if i < n {
            cur.set(i)
        }
    };
    let focus_other = move || {
        request_animation_frame(move || {
            if let Some(r) = root.try_get_untracked().flatten()
                && let Ok(Some(i)) = r.query_selector(".askq:not([hidden]) .askother")
                && let Ok(i) = i.dyn_into::<web_sys::HtmlElement>()
            {
                let _ = i.focus();
            }
        });
    };
    let focus_submit = move || {
        if let Some(r) = root.try_get_untracked().flatten()
            && let Ok(Some(b)) = r.query_selector("[data-ask-ok]")
            && let Ok(b) = b.dyn_into::<web_sys::HtmlElement>()
        {
            let _ = b.focus();
        }
    };
    let pick = move |qi: usize, oi: usize| {
        let (multi, n_opts) = qs.with_value(|q| (q[qi].multi, q[qi].options.len()));
        picked.update(|p| {
            let s = &mut p[qi];
            if multi {
                if !s.remove(&oi) {
                    s.insert(oi);
                }
            } else {
                s.clear();
                s.insert(oi);
            }
        });
        if oi == n_opts && picked.with_untracked(|p| p[qi].contains(&oi)) {
            focus_other();
            return;
        }
        if !multi {
            if qi + 1 < n {
                gloo_timers::callback::Timeout::new(300, move || {
                    let _ = cur.try_set(qi + 1);
                })
                .forget();
            } else {
                focus_submit();
            }
        }
    };
    let id_submit = id.clone();
    let submit = move || {
        let mut answers = serde_json::Map::new();
        let mut summary = Vec::new();
        for qi in 0..n {
            let q = qs.with_value(|q| q[qi].clone());
            let mut labels: Vec<String> = picked.with_untracked(|p| {
                p[qi]
                    .iter()
                    .filter_map(|&o| q.options.get(o).map(|x| x.0.clone()))
                    .collect()
            });
            let other_on = picked.with_untracked(|p| p[qi].contains(&q.options.len()));
            let o = other.with_untracked(|o| o[qi].trim().to_owned());
            if other_on && !o.is_empty() {
                labels.push(o);
            }
            if labels.is_empty() {
                go(qi);
                toast(format!(
                    "还有问题没回答：{}",
                    if q.header.is_empty() {
                        &q.question
                    } else {
                        &q.header
                    }
                ));
                return;
            }
            let joined = labels.join(", ");
            summary.push(joined.clone());
            answers.insert(q.question.clone(), json!(joined));
        }
        chat.decide(
            id_submit.clone(),
            true,
            String::new(),
            Some(Value::Object(answers)),
        );
    };
    let submit2 = submit.clone();
    let submit3 = submit.clone();
    let id_skip = id.clone();
    let skip = move || chat.decide(id_skip.clone(), false, String::new(), None);
    let skip2 = skip.clone();

    let keys = move |e: ev::KeyboardEvent| {
        let qi = cur.get_untracked();
        let target_other = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|t| t.class_list().contains("askother"));
        if target_other {
            if e.key() == "Enter" && !e.shift_key() && !ime(&e) {
                e.prevent_default();
                if qi + 1 < n {
                    go(qi + 1);
                } else {
                    submit2();
                }
            } else if e.key() == "Escape" {
                e.prevent_default();
                e.stop_propagation();
                if let Some(r) = root.try_get_untracked().flatten() {
                    let _ = r.focus();
                }
            }
            return;
        }
        let Some(card) = root.try_get_untracked().flatten() else {
            return;
        };
        let card: web_sys::HtmlElement = card.unchecked_into();
        let opts = opts_of(&card, ".askq:not([hidden]) .askopt");
        let k = e.key();
        let active = document().active_element();
        let pos = active
            .as_ref()
            .and_then(|a| opts.iter().position(|x| x.is_same_node(Some(a))));
        if let Ok(d) = k.parse::<usize>()
            && (1..=opts.len()).contains(&d)
        {
            e.prevent_default();
            pick(qi, d - 1);
        } else if k == "ArrowRight" || k == "ArrowLeft" {
            e.prevent_default();
            if k == "ArrowRight" {
                go(qi + 1);
            } else if qi > 0 {
                go(qi - 1);
            }
        } else if (k == "ArrowDown" || k == "ArrowUp") && !opts.is_empty() {
            e.prevent_default();
            let len = opts.len() as i32;
            let i = pos.map_or(0, |i| {
                (i as i32 + if k == "ArrowDown" { 1 } else { -1 }).rem_euclid(len)
            });
            let _ = opts[i as usize].focus();
        } else if k == "Enter" {
            e.prevent_default();
            match pos {
                Some(i) => pick(qi, i),
                None => submit3(),
            }
        } else if k == "Escape" {
            e.prevent_default();
            e.stop_propagation();
            skip2();
        }
    };
    focus_first_later(root, ".askopt");

    view! {
        <div class="cc-appr cc-ask" node_ref=root tabindex="0" data-folded=move || folded.get().to_string() on:keydown=keys>
            <div class="anav">
                {(0..n).map(|qi| {
                    let h = qs.with_value(|q| q[qi].header.clone());
                    view! {
                        <button class="atab" data-on=move || (cur.get() == qi).to_string()
                            data-done=move || picked.with(|p| !p[qi].is_empty()).to_string()
                            on:click=move |_| { folded.set(false); go(qi); }>{h}</button>
                    }
                }).collect_view()}
                <span class="grow"></span>
                <button class="afold" title="收起 / 展开" inner_html=CHEVRON on:click=move |_| folded.update(|f| *f = !*f)></button>
                <button class="afold" title="不回答（Esc）" on:click=move |_| skip()>"×"</button>
            </div>
            {(0..n).map(|qi| {
                let q = qs.with_value(|q| q[qi].clone());
                let n_opts = q.options.len();
                view! {
                    <div class="askq" hidden=move || cur.get() != qi>
                        <div class="askh">{q.question.clone()}{q.multi.then(|| view! { <span class="muted">"（可多选）"</span> })}</div>
                        <div class="askopts">
                            {q.options.iter().cloned().enumerate().map(|(oi, (label, desc))| view! {
                                <button class="askopt" aria-pressed=move || picked.with(|p| p[qi].contains(&oi)).to_string() on:click=move |_| pick(qi, oi)>
                                    <span class="ak">{oi + 1}</span><span class="ck"></span>
                                    <span class="at"><b>{label}</b>{(!desc.is_empty()).then(|| view! { <span>{desc}</span> })}</span>
                                </button>
                            }).collect_view()}
                            <button class="askopt" aria-pressed=move || picked.with(|p| p[qi].contains(&n_opts)).to_string() on:click=move |_| pick(qi, n_opts)>
                                <span class="ak">{n_opts + 1}</span><span class="ck"></span><span class="at"><b>"其他"</b></span>
                            </button>
                            <input class="askother" placeholder="输入你的回答…" hidden=move || !picked.with(|p| p[qi].contains(&n_opts))
                                prop:value=move || other.with(|o| o[qi].clone()) on:input=move |e| { let v = event_target_value(&e); other.update(|o| o[qi] = v); }/>
                        </div>
                    </div>
                }
            }).collect_view()}
            <div class="aopts"><button class="aopt primary" data-ask-ok on:click=move |_| submit()><span class="ak">"⏎"</span>"提交回答"</button></div>
            <div class="ahint">"←→ 切换问题 · 数字键选择 · 回车提交 · Esc 不回答"</div>
        </div>
    }
}
