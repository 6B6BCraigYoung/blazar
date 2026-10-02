//! 对话记录的渲染。条目按 (key, 签名) 复用：流式输出时只有变了的那一条重绘；
//! 展开 / 收起的状态记在外面，重绘后不丢。

use std::collections::HashSet;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::chat_model::{Body, Extra, Fold, Item, Res, Step, Tool, fmt_tokens};
use crate::components::dialog::{self, Choice};
use crate::components::toast::toast;
use crate::md;

use super::state::Chat;

/// 记住哪些 <details> / 「展开」被打开过（按 id）。
#[derive(Clone, Copy)]
pub struct Opened(RwSignal<HashSet<String>>);

impl Opened {
    fn is(self, k: &str) -> bool {
        self.0.with_untracked(|s| s.contains(k))
    }
    fn set(self, k: &str, on: bool) {
        self.0.update(|s| {
            if on {
                s.insert(k.to_owned());
            } else {
                s.remove(k);
            }
        });
    }
}

const CHEVRON: &str = r#"<svg class="cc-fchev" viewBox="0 0 16 16"><path d="M6 4l4 4-4 4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>"#;
const COPY: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/></svg>"#;

/// <details>，开合状态记在 `opened` 里。
fn details(
    opened: Opened,
    key: String,
    class: &'static str,
    summary: AnyView,
    body: impl FnOnce() -> AnyView,
) -> impl IntoView {
    let k = key.clone();
    view! {
        <details class=class open=opened.is(&key) on:toggle=move |e| {
            let open = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlDetailsElement>().ok()).is_some_and(|d| d.open());
            opened.set(&k, open);
        }>
            <summary>{summary}</summary>
            {body()}
        </details>
    }
}

/// 渲染 Markdown 并用 Monaco 给代码块上色。
#[component]
pub fn Md(text: String, ws: String) -> impl IntoView {
    let el = NodeRef::<html::Div>::new();
    let html = md::render(&text, "", &ws);
    Effect::new(move |_| {
        let Some(root) = el.get() else { return };
        let Ok(list) = root.query_selector_all("pre code[class^=\"language-\"]") else {
            return;
        };
        for i in 0..list.length() {
            let Some(code) = list
                .item(i)
                .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
            else {
                continue;
            };
            let lang = code.class_name().trim_start_matches("language-").to_owned();
            let text = code.text_content().unwrap_or_default();
            if text.len() > 20_000 {
                continue;
            }
            leptos::task::spawn_local(async move {
                if let Some(h) = crate::monaco::colorize(&text, &super::super::md_lang(&lang)).await
                    && code.is_connected()
                {
                    code.set_inner_html(&h);
                }
            });
        }
    });
    view! { <div class="cc-md md-body" node_ref=el inner_html=html></div> }
}

fn result_view(r: Res, key: String, opened: Opened) -> impl IntoView {
    let full = opened.is(&key);
    let show = RwSignal::new(full);
    let Res {
        shown,
        full: all,
        more,
        cls,
    } = r;
    view! {
        <div class=format!("cc-out {cls}")>
            <span class="cc-elbow">"⎿"</span>
            <div class="cc-pre">
                {move || if show.get() { all.clone() } else { shown.clone() }}
                {move || (more > 0 && !show.get()).then(|| {
                    let k = key.clone();
                    view! { <button class="cc-more" on:click=move |_| { show.set(true); opened.set(&k, true); }>{format!("… +{more} line{} (click to expand)", if more == 1 { "" } else { "s" })}</button> }
                })}
            </div>
        </div>
    }
}

fn extra_view(e: &Extra, opened: Opened, key: &str, ws: &str) -> AnyView {
    match e {
        Extra::None => ().into_any(),
        Extra::Cmd(c) => view! { <div class="cc-io"><span class="lb">"IN"</span><div class="cc-pre">{c.clone()}</div></div> }.into_any(),
        Extra::Todos(list) => todo_list(list).into_any(),
        Extra::Diff { rows, more } => view! {
            <div class="cc-diff">
                {rows.iter().map(|(c, l)| {
                    let cls = match c { '+' => "dadd", '-' => "ddel", _ => "dgap" };
                    view! { <div class=cls><span class="dm">{if *c == '⋯' { ' ' } else { *c }}</span>{if *c == '⋯' { "⋯".to_owned() } else { l.clone() }}</div> }
                }).collect_view()}
                {(*more > 0).then(|| view! { <div class="dgap">{format!("… {more} more line{}", if *more == 1 { "" } else { "s" })}</div> })}
            </div>
        }.into_any(),
        Extra::Files(f) => view! { <div class="cc-diff">{f.iter().map(|x| view! { <div class="dh">{x.clone()}</div> }).collect_view()}</div> }.into_any(),
        Extra::Plan(p) => {
            let p = p.clone();
            let ws = ws.to_owned();
            details(opened, format!("plan:{key}"), "cc-planbody", view! { "Plan" }.into_any(), move || view! { <Md text=p ws/> }.into_any()).into_any()
        }
    }
}

pub fn todo_list(list: &[crate::chat_model::Todo]) -> impl IntoView + use<> {
    view! {
        <div class="cc-todos">
            {list.iter().map(|t| view! {
                <div class=format!("cc-todo {}", t.status)>
                    <span>{if t.status == "completed" { "☒" } else { "☐" }}</span>
                    <span>{t.content.clone()}</span>
                </div>
            }).collect_view()}
        </div>
    }
}

fn tool_view(t: Tool, opened: Opened, ws: String) -> AnyView {
    let sub = (!t.sub.is_empty() || t.sub_calls > 0).then(|| {
        let summary = if t.sub_calls > 0 { format!("{} tool call{} · latest: {}", t.sub_calls, if t.sub_calls == 1 { "" } else { "s" }, t.sub_last) } else { "Subagent working…".to_owned() };
        let steps = t.sub.clone();
        let ws = ws.clone();
        details(opened, format!("sub:{}", t.id), "cc-sub", view! { {summary} }.into_any(), move || {
            view! { <div class="cc-subl">{steps.into_iter().map(|s| step_view(s, opened, ws.clone())).collect_view()}</div> }.into_any()
        })
    });
    view! {
        <div class="cc-row cc-tool" data-st=t.state.as_str() data-name=t.name.clone()>
            <span class="cc-dot"></span>
            <div class="cc-main">
                <div class="cc-head"><b>{t.label.clone()}</b>{(!t.arg.is_empty()).then(|| view! { <span class="cc-arg">{t.arg.clone()}</span> })}</div>
                {extra_view(&t.extra, opened, &t.id, &ws)}
                {sub}
                <div class="cc-res">{t.result.map(|r| result_view(r, format!("res:{}", t.id), opened))}</div>
            </div>
        </div>
    }.into_any()
}

fn step_view(s: Step, opened: Opened, ws: String) -> AnyView {
    match s {
        Step::Thinking { text, secs } => {
            let head = if secs > 0 { format!("✻ Thought for {secs}s") } else { "✻ Thought".to_owned() };
            if text.trim().is_empty() {
                view! { <div class="cc-thinkmark">{head}</div> }.into_any()
            } else {
                let key = format!("think:{secs}:{}:{}", text.len(), text.chars().take(24).collect::<String>());
                details(opened, key, "cc-think", view! { {head} }.into_any(), move || view! { <Md text ws/> }.into_any()).into_any()
            }
        }
        Step::Tool(t) => tool_view(*t, opened, ws),
        Step::Text(t) => view! { <div class="cc-row cc-msg sub"><span class="cc-dot"></span><div class="cc-main"><Md text=t ws/></div></div> }.into_any(),
        Step::Bg(t) => view! {
            <div class="cc-row cc-bg"><span class="cc-dot"></span><div class="cc-main"><div class="cc-head"><b>"Background task"</b><span class="cc-arg">{t}</span></div></div></div>
        }.into_any(),
        Step::Note(t) => view! { <div class="cc-meta">{t}</div> }.into_any(),
        Step::Orphan(r) => view! { <div class="cc-row cc-orphan">{result_view(r, String::new(), opened)}</div> }.into_any(),
    }
}

fn fold_view(key: String, f: Fold, opened: Opened, ws: String) -> AnyView {
    let Fold {
        steps,
        live,
        summary,
        state,
    } = f;
    view! {
        <div class="cc-row cc-fold" data-st=state.as_str() data-live=live.to_string()>
            <span class="cc-dot"></span>
            <div class="cc-main">
                {details(opened, format!("fold:{key}"), "cc-fd",
                    view! { <span class="cc-fl">{summary}</span><span inner_html=CHEVRON></span> }.into_any(),
                    move || view! { <div class="cc-fb">{steps.into_iter().map(|s| step_view(s, opened, ws.clone())).collect_view()}</div> }.into_any())}
            </div>
        </div>
    }.into_any()
}

fn copy(text: String, done: RwSignal<bool>) {
    let Some(w) = web_sys::window() else { return };
    let p = w.navigator().clipboard().write_text(&text);
    leptos::task::spawn_local(async move {
        if wasm_bindgen_futures::JsFuture::from(p).await.is_ok() {
            let _ = done.try_set(true);
            gloo_timers::future::TimeoutFuture::new(1200).await;
            let _ = done.try_set(false);
        } else {
            toast("Copy failed");
        }
    });
}

#[component]
fn UserMsg(
    chat: Chat,
    sid: String,
    seq: u64,
    text: String,
    first: bool,
    long: bool,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let cp_key = format!("{sid}:{seq}");
    let cp = Memo::new(move |_| chat.checkpoints.with(|c| c.get(&cp_key).cloned()));
    let sid2 = sid.clone();
    let t2 = text.clone();
    // 这条之后还有几轮（重试时提示会一起丢）。
    let later = move || {
        chat.transcript.with_untracked(|t| {
            let mut after = false;
            let mut n = 0;
            for it in &t.items {
                if let Body::User {
                    sid: s,
                    seq: q,
                    first: f,
                    ..
                } = &it.body
                {
                    if after && *f {
                        n += 1;
                    }
                    if *s == sid2 && *q == seq {
                        after = true;
                    }
                }
            }
            n
        })
    };
    let later2 = later.clone();
    let sid_edit = sid.clone();
    let do_retry = move || {
        let t = draft.get_untracked().trim().to_owned();
        if t.is_empty() {
            return;
        }
        editing.set(false);
        chat.retry(sid_edit.clone(), Some(t), later2());
    };
    let do_retry2 = do_retry.clone();
    view! {
        <div class="cc-user" data-first=first.to_string() data-long=long.to_string() data-open=move || open.get().to_string()>
            <Show when=move || !editing.get() fallback=move || {
                let go = do_retry.clone();
                let go2 = do_retry2.clone();
                view! {
                    <div class="cc-edit">
                        <textarea rows="3" prop:value=move || draft.get() on:input=move |e| draft.set(event_target_value(&e))
                            on:keydown=move |e| {
                                if e.key() == "Escape" { e.prevent_default(); e.stop_propagation(); editing.set(false); }
                                if e.key() == "Enter" && (e.meta_key() || e.ctrl_key()) { e.prevent_default(); go(); }
                            }></textarea>
                        <div class="row">
                            <span class="muted small">"Retrying restores files to before this message and drops the conversation after it"</span><span class="grow"></span>
                            <button class="btn small" on:click=move |_| editing.set(false)>"Cancel"</button>
                            <button class="btn small primary" on:click=move |_| go2()>"Retry"</button>
                        </div>
                    </div>
                }
            }>
                <div class="cc-ut">{text.clone()}</div>
            </Show>
            {long.then(|| view! {
                <button class="cc-xp" on:click=move |_| open.update(|o| *o = !*o)>{move || if open.get() { "Show less" } else { "Show more" }}</button>
            })}
            <span class="cc-acts">
                {first.then(|| {
                    let t = t2.clone();
                    let sid_r = sid.clone();
                    let later = later.clone();
                    view! {
                        <button class="cc-rw" title="Edit this message and continue from here" on:click=move |_| { draft.set(t.clone()); editing.set(true); }>"✎ Edit"</button>
                        <button class="cc-rw" title="Ask again with the same message" on:click=move |_| chat.retry(sid_r.clone(), None, later())>"⟳ Retry"</button>
                    }
                })}
                {move || cp.get().map(|c| view! {
                    <button class="cc-rw" title="Restore files to before this message" on:click=move |_| chat.rewind(c.clone(), false)>"↺ Rewind"</button>
                })}
            </span>
        </div>
    }
}

fn error_view(chat: Chat, text: String, by_account: bool) -> AnyView {
    let switch = move |_| {
        chat.spawn(async move {
            let rt = untrack(move || chat.runtime());
            let last = chat.view.get_untracked().and_then(|v| {
                chat.threads.with_untracked(|t| {
                    t.iter().find(|x| x.id == v).and_then(|x| x.account.clone())
                })
            });
            let list: Vec<crate::api::Account> = chat.accounts.with_untracked(|a| {
                a.as_ref()
                    .map(|a| {
                        a.accounts
                            .iter()
                            .filter(|x| {
                                x.provider == rt && Some(&x.id) != last.as_ref() && x.usable()
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            });
            if list.is_empty() {
                toast("No other usable account. Add or log in to one on the Runtimes page first.");
                return;
            }
            let mut choices = vec![Choice::plain("Cancel")];
            choices.extend(list.iter().map(|a| Choice::plain(a.label.clone())));
            if let Some(i) = dialog::ask(
                "Continue with another account",
                "Pick up this conversation where it stopped. The context stays the same; only the account changes.",
                choices,
            )
            .await
                && i > 0
            {
                chat.continue_on_another(list[i - 1].id.clone());
            }
        });
    };
    let can = by_account
        && super::state::ACC_RUNTIMES.contains(&untrack(move || chat.runtime()).as_str());
    view! {
        <div class="cc-row cc-err"><span class="cc-dot"></span><div class="cc-main">
            {text}
            {can.then(|| view! { <div><button class="linkbtn" on:click=switch>"Continue with another account"</button></div> })}
        </div></div>
    }.into_any()
}

fn item_view(it: Item, chat: Chat, opened: Opened, ws: String) -> AnyView {
    let key = it.key.clone();
    match it.body {
        Body::User { sid, seq, text, first, long } => view! { <UserMsg chat sid seq text first long/> }.into_any(),
        Body::Assistant { text } => {
            let done = RwSignal::new(false);
            let t = text.clone();
            view! {
                <div class="cc-row cc-msg"><span class="cc-dot"></span><div class="cc-main">
                    <Md text ws/>
                    <div class="cc-acts2">
                        <button class="cc-copy" title="Copy" data-done=move || done.get().to_string() inner_html=COPY on:click=move |_| copy(t.clone(), done)></button>
                    </div>
                </div></div>
            }.into_any()
        }
        Body::Fold(f) => fold_view(key, f, opened, ws),
        Body::Error { text, by_account } => error_view(chat, text, by_account),
        Body::Warn { denied } => view! {
            <div class="cc-row cc-warn"><span class="cc-dot"></span><div class="cc-main">
                {format!("{} action{} blocked by permissions and not run:", denied.len(), if denied.len() == 1 { " was" } else { "s were" })}
                {denied.into_iter().map(|d| view! { <div class="muted">{format!("· {d}")}</div> }).collect_view()}
                <div class="muted small">"In headless mode the agent denies instead of asking. Switch the mode to Edit automatically or Auto to allow these."</div>
            </div></div>
        }.into_any(),
        Body::Meta { text, bad } => view! { <div class="cc-meta" class:bad=bad>{text}</div> }.into_any(),
        Body::Rewound { turns, items } => details(opened, format!("rw:{key}"), "cc-rewound",
            view! { {format!("Rewound · {turns} message{} (dropped by an edit, no longer in context)", if turns == 1 { "" } else { "s" })} }.into_any(),
            move || view! { <div class="cc-rwl">{items.into_iter().map(|i| item_view(i, chat, opened, ws.clone())).collect_view()}</div> }.into_any()).into_any(),
    }
}

const SPIN: [&str; 12] = ["·", "✢", "*", "✶", "✻", "✽", "✽", "✻", "✶", "*", "✢", "·"];
const WORDS: [&str; 12] = [
    "Thinking",
    "Pondering",
    "Musing",
    "Brewing",
    "Cogitating",
    "Percolating",
    "Mulling",
    "Crafting",
    "Tinkering",
    "Noodling",
    "Ruminating",
    "Synthesizing",
];

/// 跟 Claude Code 一样的局部「转圈」：字符来回变，后面一个动词隔几秒换一次。
#[component]
fn Spinner(chat: Chat, tick: RwSignal<u32>) -> impl IntoView {
    let word = RwSignal::new(0usize);
    let changes = StoredValue::new((0u32, 0u32));
    Effect::new(move |_| {
        let t = tick.get();
        let (at, n) = changes.get_value();
        // 2s / 3s / 5s 后换词，之后每 5s 换一次（tick 是 120ms）
        let gap = [17u32, 25, 42].get(n as usize).copied().unwrap_or(42);
        if t.wrapping_sub(at) > gap {
            changes.set_value((t, n + 1));
            word.update(|w| *w = (*w + 1 + (js_sys::Math::random() * 10.0) as usize) % WORDS.len());
        }
    });
    let mode = move || chat.effective_mode().map(|m| m.0).unwrap_or("");
    // 正在思考的折叠行已经带着 token 数时，转圈这里就不重复了
    let tokens = move || {
        chat.transcript.with(|t| {
            let live = matches!(t.items.last(), Some(Item { body: Body::Fold(f), .. }) if f.live);
            (t.turn_tokens > 0 && !live).then(|| fmt_tokens(t.turn_tokens))
        })
    };
    view! {
        <div class="cc-spin" data-mode=mode>
            <span class="ic">{move || SPIN[tick.get() as usize % SPIN.len()]}</span>
            <span class="tx">{move || format!("{}…", WORDS[word.get()])}</span>
            {move || tokens().map(|n| view! { <span class="tk">{format!("· {n} tokens")}</span> })}
        </div>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(module = "/js/scroll.js")]
extern "C" {
    #[wasm_bindgen(js_name = keepScroll)]
    fn keep_scroll(el: &web_sys::HtmlElement);
}

#[component]
pub fn Log(chat: Chat, tick: RwSignal<u32>) -> impl IntoView {
    let opened = Opened(RwSignal::new(HashSet::new()));
    let el = NodeRef::<html::Div>::new();
    let at_bottom = StoredValue::new(true);
    let ws = chat.ws_id();
    // 换对话：展开状态清掉，滚到底。
    Effect::new(move |_| {
        chat.view.track();
        opened.0.set(HashSet::new());
        at_bottom.set_value(true);
    });
    // 拖窄拖宽面板时别让文字跑掉
    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        el.get().map(|e| keep_scroll(&e)).is_some()
    });
    let items = move || chat.transcript.with(|t| t.items.clone());
    // 内容变了：原来在底部就跟着滚到底。
    Effect::new(move |_| {
        chat.transcript.track();
        chat.local_errors.track();
        chat.pending.track();
        if at_bottom.get_value()
            && let Some(e) = el.get()
        {
            request_animation_frame(move || e.set_scroll_top(e.scroll_height()));
        }
    });
    // 自己发了消息：不管原来滚到哪，都回到底部看它。
    Effect::new(move |prev: Option<usize>| {
        let n = chat.pending.with(Vec::len);
        if prev.is_some_and(|p| n > p) {
            at_bottom.set_value(true);
        }
        n
    });
    let dim = move || {
        chat.transcript.with(|t| {
            t.pending
                .iter()
                .any(|p| !p.ask && !chat.decided.with(|d| d.contains(&p.id)))
        })
    };
    // 刚发出去也算在跑：先转圈，不等服务端。
    let busy = move || {
        (chat.running.get() || chat.pending.with(|p| !p.is_empty()))
            && chat.transcript.with(|t| {
                t.pending
                    .iter()
                    .all(|p| chat.decided.with(|d| d.contains(&p.id)))
            })
    };
    let empty = move || {
        !chat.loading.get()
            && chat.rows.with(Vec::is_empty)
            && chat.local_errors.with(Vec::is_empty)
            && chat.pending.with(Vec::is_empty)
    };
    view! {
        <div class="cc-log" node_ref=el data-dim=move || dim().to_string()
            on:scroll=move |_| if let Some(e) = el.try_get_untracked().flatten() {
                at_bottom.set_value(e.scroll_height() - e.scroll_top() - e.client_height() < 80);
            }>
            <Show when=empty>
                <Empty chat/>
            </Show>
            <For each=items key=|it| (it.key.clone(), it.sig) let:it>
                {item_view(it, chat, opened, ws.clone())}
            </For>
            <For each=move || chat.pending.get() key=|p| p.id let:p>
                <div class="cc-user pending">
                    <div class="cc-ut">{p.text.clone()}{(p.images > 0).then(|| format!("\n[{} image{}]", p.images, if p.images == 1 { "" } else { "s" }))}</div>
                </div>
            </For>
            {move || chat.local_errors.get().into_iter().map(|e| view! {
                <div class="cc-row cc-err"><span class="cc-dot"></span><div class="cc-main">{e}</div></div>
            }).collect_view()}
            <Show when=busy>
                <Spinner chat tick/>
            </Show>
        </div>
    }
}

#[component]
fn Empty(chat: Chat) -> impl IntoView {
    move || {
        let p = chat.profile();
        let starters = p.as_ref().map(|p| p.starters.clone()).unwrap_or_default();
        view! {
            <div class="cc-empty">
                {match &p {
                    Some(p) => view! {
                        <div class="who"><b>{format!("Chat with {}", p.name)}</b><div class="muted small">{p.description.clone().unwrap_or_default()}</div></div>
                    }.into_any(),
                    None => view! { <div>"New conversation."</div> }.into_any(),
                }}
                {(!starters.is_empty()).then(|| view! {
                    <div class="starters">
                        {starters.into_iter().map(|s| {
                            let pr = s.prompt.clone();
                            view! { <button class="starter" on:click=move |_| chat.prompt.set(pr.clone())><b>{s.label}</b><span>{s.prompt}</span></button> }
                        }).collect_view()}
                    </div>
                })}
                <div class="muted small">"Type below, Enter to send."</div>
            </div>
        }
    }
}
