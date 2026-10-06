use leptos::html;
use leptos::prelude::*;
use serde_json::json;

use crate::components::status::EmptyState;
use crate::components::toast::toast;

use super::super::state::{ACC_RUNTIMES, Chat, ModelSel};
use super::presentation::{editor_url, eff_label, icon};
use super::{Pop, SlashItem};

fn row_btn(
    title: String,
    sub: Option<String>,
    right: Option<String>,
    on: bool,
    click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button class="cp-row" data-sel=on.to_string() on:click=move |_| click()>
            <span class="cp-t"><b>{title}</b>{sub.map(|s| view! { <span>{s}</span> })}</span>
            {right.map(|r| view! { <span class="cp-r">{r}</span> })}
            {on.then(|| view! { <span class="cp-ok">"✓"</span> })}
        </button>
    }
}

pub(super) fn slash_pop(
    items: Vec<SlashItem>,
    sel: RwSignal<usize>,
    run: impl Fn(usize) + Copy + 'static,
) -> impl IntoView {
    if items.is_empty() {
        return view! { <EmptyState title="No matching commands" class="cp-none"/> }.into_any();
    }
    items
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            view! {
                <button class="cp-row cp-cmd" data-sel=move || (sel.get() == i).to_string() on:click=move |_| run(i)>
                    <span class="cp-l">{format!("/{}", c.name)}{(!c.hint.is_empty()).then(|| view! { " "<span class="hint">{c.hint.clone()}</span> })}</span>
                    <span class="cp-d">{c.desc.clone()}</span>
                </button>
            }
        })
        .collect_view()
        .into_any()
}

#[component]
pub(super) fn FilesPop(
    chat: Chat,
    tree_files: Signal<Vec<String>>,
    insert_ref: Callback<String>,
    insert_at_caret: Callback<String>,
    pop: RwSignal<Option<Pop>>,
) -> impl IntoView {
    let q = RwSignal::new(String::new());
    let input = NodeRef::<html::Input>::new();
    Effect::new(move |_| {
        if let Some(i) = input.get() {
            let _ = i.focus();
        }
    });
    let snippets = move || {
        let k = q.get().to_lowercase();
        chat.snippets
            .get()
            .into_iter()
            .filter(|s| {
                k.is_empty()
                    || s.name.to_lowercase().contains(&k)
                    || s.body.to_lowercase().contains(&k)
            })
            .take(8)
            .collect::<Vec<_>>()
    };
    let files = move || {
        let k = q.get().to_lowercase();
        tree_files
            .get()
            .into_iter()
            .filter(|f| k.is_empty() || f.to_lowercase().contains(&k))
            .take(40)
            .collect::<Vec<_>>()
    };
    let insert_snip = move |body: String| {
        let pre_nl = chat
            .prompt
            .with_untracked(|v| !v.is_empty() && !v.ends_with(char::is_whitespace));
        insert_at_caret.run(format!("{}{body}\n", if pre_nl { "\n" } else { "" }));
    };
    view! {
        <div class="cp-h">"Add context"</div>
        <input class="cp-q mono" aria-label="Search files and snippets" node_ref=input placeholder="Search files or snippets…" prop:value=move || q.get()
            on:input=move |e| q.set(event_target_value(&e))
            on:keydown=move |e| {
                if e.key() == "Enter" && !e.is_composing() {
                    e.prevent_default();
                    if let Some(s) = snippets().into_iter().next() { insert_snip(s.body); } else if let Some(f) = files().into_iter().next() { insert_ref.run(f); }
                }
                if e.key() == "Escape" { pop.set(None); }
            }/>
        <div class="cp-list">
            {move || snippets().into_iter().map(|s| {
                let body = s.body.clone();
                let preview: String = s.body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(70).collect();
                view! { <button class="cp-row" on:click=move |_| insert_snip(body.clone())><span class="cp-t"><b>{format!("@{}", s.name)}</b><span>{preview}</span></span><span class="cp-r">"snippet"</span></button> }
            }).collect_view()}
            {move || files().into_iter().map(|f| {
                let f2 = f.clone();
                view! { <button class="cp-row cp-file" on:click=move |_| insert_ref.run(f2.clone())>{f}</button> }
            }).collect_view()}
            {move || (snippets().is_empty() && files().is_empty()).then(|| view! { <EmptyState title="No matching files or snippets" class="cp-none"/> })}
        </div>
    }
}

fn open_in_editor(chat: Chat) {
    let root = chat.root.get_value();
    let target = chat.files.current.get_untracked().map_or_else(
        || root.clone(),
        |f| format!("{}/{f}", root.trim_end_matches('/')),
    );
    let (_, url) = editor_url(&chat.node.get_value(), &target, None);
    let _ = window().location().set_href(&url);
}

pub(super) fn plus_pop(
    chat: Chat,
    pop: RwSignal<Option<Pop>>,
    file_in: NodeRef<html::Input>,
) -> impl IntoView {
    let cur = chat.files.current.get_untracked();
    view! {
        <button class="cp-row" on:click=move |_| { pop.set(None); if let Some(i) = file_in.get_untracked() { i.click(); } }>
            <span class="cp-ico" inner_html=icon("up")></span><span class="cp-t"><b>"Attach file…"</b></span><span class="cp-r">"or paste / drop"</span>
        </button>
        <button class="cp-row" on:click=move |_| { chat.load_snippets(); pop.set(Some(Pop::Files)); }>
            <span class="cp-ico" inner_html=icon("file")></span><span class="cp-t"><b>"Mention file from this project…"</b></span><span class="cp-r">"@"</span>
        </button>
        <button class="cp-row" on:click=move |_| { pop.set(None); open_in_editor(chat); }>
            <span class="cp-ico" inner_html=icon("file")></span><span class="cp-t"><b>{format!("Open in {}", editor_url("local", "", None).0)}</b></span>
        </button>
        {cur.map(|f| {
            let on = chat.ctx_off.get_untracked().as_deref() != Some(f.as_str());
            let f2 = f.clone();
            view! {
                <button class="cp-row" on:click=move |_| { chat.ctx_off.set(if on { Some(f2.clone()) } else { None }); pop.set(None); }>
                    <span class="cp-ico" inner_html=icon("file")></span>
                    <span class="cp-t"><b>{if on { "Remove current file" } else { "Add current file" }}</b><span>{f}</span></span>
                </button>
            }
        })}
    }
}

fn effort_row(chat: Chat) -> impl IntoView {
    move || {
        let rt = chat.runtime();
        let s = chat.model_sel(&rt);
        let list = chat.models_for(&rt);
        let m = list
            .iter()
            .find(|x| x.id == s.model.clone().unwrap_or_default())
            .or_else(|| list.first())
            .cloned();
        let effs = m.map(|m| m.efforts).unwrap_or_default();
        (!effs.is_empty()).then(|| {
            let cur_i = s.effort.as_ref().and_then(|e| effs.iter().position(|x| x == e));
            let label = s.effort.as_deref().map_or_else(|| "Default".to_owned(), eff_label);
            view! {
                <div class="cp-eff">
                    <span>"Effort "<span class="muted">{format!("({label})")}</span></span>
                    <span class="eff-track">
                        {effs.iter().enumerate().map(|(i, e)| {
                            let e = e.clone();
                            let rt = rt.clone();
                            let title = eff_label(&e);
                            view! {
                                <button class="eff-dot" title=title data-on=cur_i.is_some_and(|c| c >= i).to_string() data-cur=(cur_i == Some(i)).to_string()
                                    on:click=move |ev| {
                                        ev.stop_propagation();
                                        let mut s = chat.model_sel(&rt);
                                        s.effort = if s.effort.as_deref() == Some(e.as_str()) { None } else { Some(e.clone()) };
                                        chat.set_model_sel(&rt, &s);
                                    }></button>
                            }
                        }).collect_view()}
                    </span>
                </div>
            }
        })
    }
}

pub(super) fn mode_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    let cur = chat.effective_mode().map(|m| m.0);
    view! {
        <div class="cp-sec flex">"Modes"<span class="cp-hk"><kbd>"⇧"</kbd>" + "<kbd>"tab"</kbd>" to switch"</span></div>
        {chat.modes().iter().map(|&(v, t, d, ic)| view! {
            <button class="cp-row" data-sel=(Some(v) == cur).to_string() on:click=move |_| { chat.set_mode(v); pop.set(None); }>
                <span class="cp-ico" inner_html=icon(ic)></span>
                <span class="cp-t"><b>{t}</b><span>{d}</span></span>
                {(Some(v) == cur).then(|| view! { <span class="cp-ok">"✓"</span> })}
            </button>
        }).collect_view()}
        {effort_row(chat)}
    }
}

pub(super) fn model_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    let rt = chat.runtime();
    view! {
        <div class="cp-h">"Switch model"</div>
        {move || {
            let rt = rt.clone();
            let s = chat.model_sel(&rt);
            let cur = s.model.clone().unwrap_or_default();
            chat.models_for(&rt).into_iter().map(|m| {
                let rt = rt.clone();
                let id = m.id.clone();
                let on = m.id == cur;
                row_btn(m.label.clone(), (!m.desc.is_empty()).then(|| m.desc.clone()), None, on, move || {
                    let mut next = chat.model_sel(&rt);
                    next.model = Some(id.clone()).filter(|x| !x.is_empty());
                    let effs = chat.models_for(&rt).into_iter().find(|x| x.id == id).map(|x| x.efforts).unwrap_or_default();
                    if next.effort.as_ref().is_some_and(|e| !effs.contains(e)) {
                        next.effort = None;
                    }
                    chat.set_model_sel(&rt, &ModelSel { model: next.model, effort: next.effort });
                    pop.set(None);
                })
            }).collect_view()
        }}
        {effort_row(chat)}
        {(chat.runtime() == "claude").then(|| view! {
            <button class="cp-row" on:click=move |_| toggle_thinking(chat)>
                <span class="cp-t"><b>"Thinking"</b><span>"Extended thinking for the next messages"</span></span>
                <span class="cp-r">{move || if chat.prefs.get().thinking { "On" } else { "Off" }}</span>
            </button>
        })}
    }
}

fn toggle_thinking(chat: Chat) {
    let on = !chat.prefs.get_untracked().thinking;
    chat.set_prefs(
        |p| p.thinking = on,
        Some((
            json!({ "thinking": on }),
            if on { "thinking on" } else { "thinking off" },
        )),
    );
    if !chat.running.get_untracked() {
        toast(if on {
            "Thinking on"
        } else {
            "Thinking off for next messages"
        });
    }
}

pub(super) fn agent_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    let cur = chat.agent.get_untracked();
    let profiles = chat.profiles.get_untracked();
    let rts = untrack(move || chat.runtimes());
    let remote = chat.remote();
    let node = chat.node.get_value();
    let pick = move |v: String, acc: Option<String>| {
        pop.set(None);
        chat.set_agent(v);
        if let Some(a) = acc {
            chat.set_acc_sel(&a, chat.view.get_untracked().as_deref());
            if chat.running.get_untracked() {
                toast("The next message will use the new account after this turn.");
            }
        }
    };
    let acc_rows = move |rt: String, current: bool| -> AnyView {
        if remote && rt == "codex" {
            let st = match chat.remote_codex.get_untracked() {
                Some(true) => "Logged in",
                Some(false) => "Logged out · log in",
                None => "Log in",
            };
            let n2 = node.clone();
            return view! {
                <button class="cp-row cp-sub" on:click=move |_| {
                    pop.set(None);
                    crate::pages::runtimes::node_login(n2.clone(), Callback::new(move |()| chat.load_catalogs()));
                }><span class="cp-t"><b>{format!("Log in to Codex on {node}")}</b></span><span class="cp-r">{st}</span></button>
            }.into_any();
        }
        let list: Vec<crate::api::Account> = chat.accounts.with_untracked(|a| {
            a.as_ref()
                .map(|a| {
                    a.accounts
                        .iter()
                        .filter(|x| x.provider == rt)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        });
        let on_id = if current {
            untrack(move || chat.current_account(&rt))
        } else {
            None
        };
        list.into_iter().map(|a| {
            let away = remote && a.kind != "token";
            let off = !a.usable() || away;
            let note = if away {
                "Remote needs a setup token".to_owned()
            } else if off {
                if a.disabled { "Disabled".to_owned() } else if a.kind == "token" { "Token expired".to_owned() } else { "Logged out".to_owned() }
            } else {
                a.windows.iter().filter(|w| w.name != "blocked").map(|w| format!("{} {}%", match w.name.as_str() { "five_hour" => "5h", "seven_day" => "7d", n => n }, (w.utilization * 100.0).round())).collect::<Vec<_>>().join(" · ")
            };
            let on = on_id.as_deref() == Some(a.id.as_str());
            let v = format!("r:{}", a.provider);
            let id = a.id.clone();
            view! {
                <button class="cp-row cp-sub" disabled=off on:click=move |_| pick(v.clone(), Some(id.clone()))>
                    <span class="cp-t"><b>{a.label.clone()}</b></span>
                    {(!note.is_empty()).then(|| view! { <span class="cp-r">{note}</span> })}
                    {on.then(|| view! { <span class="cp-ok">"✓"</span> })}
                </button>
            }
        }).collect_view().into_any()
    };
    let none = profiles.is_empty() && rts.is_empty();
    view! {
        {(!profiles.is_empty()).then(|| view! { <div class="cp-sec">"My agents"</div> })}
        {profiles.into_iter().map(|p| {
            let v = format!("p:{}", p.id);
            let on = v == cur;
            let v2 = v.clone();
            view! {
                <button class="cp-row" on:click=move |_| pick(v2.clone(), None)>
                    <span class="rt-dot" data-rt=p.runtime.clone()></span>
                    <span class="cp-t"><b>{format!("{} · {}", p.name, p.runtime_label)}</b></span>
                    {on.then(|| view! { <span class="cp-ok">"✓"</span> })}
                </button>
            }
        }).collect_view()}
        {(!rts.is_empty()).then(|| view! { <div class="cp-sec">"Runtimes"</div> })}
        {rts.into_iter().map(|r| {
            let v = format!("r:{}", r.id);
            let on = v == cur;
            let has_acc = ACC_RUNTIMES.contains(&r.id.as_str());
            let v2 = v.clone();
            view! {
                <button class="cp-row" on:click=move |_| pick(v2.clone(), None)>
                    <span class="rt-dot" data-rt=r.id.clone()></span>
                    <span class="cp-t"><b>{r.label.clone()}</b></span>
                    {(on && !has_acc).then(|| view! { <span class="cp-ok">"✓"</span> })}
                </button>
                {has_acc.then(|| acc_rows(r.id.clone(), on))}
            }
        }).collect_view()}
        {none.then(|| view! {
            <div class="cp-none">{if remote { "Set up Claude Code or Codex on the Runtimes page to use remote workspaces." } else { "Set up Claude Code or Codex on the Runtimes page first." }}</div>
        })}
    }
}

pub(super) fn mcp_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    move || {
        let live = chat.running.get();
        let mut list = chat.transcript.with(|t| t.mcp.clone());
        if list.is_empty() {
            list = chat.catalog.with(|c| {
                c.as_ref()
                    .map(|c| c.mcp_servers.clone())
                    .unwrap_or_default()
            });
        }
        let order = |s: &str| match s {
            "failed" => 0,
            "needs-auth" => 1,
            "pending" => 2,
            "connected" => 3,
            "disabled" => 4,
            _ => 5,
        };
        list.sort_by(|a, b| {
            order(&a.status)
                .cmp(&order(&b.status))
                .then_with(|| a.name.cmp(&b.name))
        });
        let ok = list.iter().filter(|m| m.status == "connected").count();
        let n = list.len();
        view! {
            <div class="cp-h">{format!("MCP servers · {ok}/{n} connected")}</div>
            {if list.is_empty() {
                view! { <EmptyState title="No MCP servers" class="cp-none"/> }.into_any()
            } else {
                list.into_iter().map(|m| {
                    let st = match m.status.as_str() { "connected" => "connected", "pending" => "connecting", "failed" => "failed", "needs-auth" => "needs auth", "disabled" => "disabled", s => s }.to_owned();
                    let name = m.name.clone();
                    let name2 = m.name.clone();
                    let disabled = m.status == "disabled";
                    view! {
                        <div class="mcp-row">
                            <span class="mcp-dot" data-s=m.status.clone()></span>
                            <span class="nm" title=m.name.clone()>{m.name.clone()}</span>
                            <span class="mcp-st">{st}</span>
                            {(live && m.status == "failed").then(|| view! {
                                <button class="mcp-act" on:click=move |_| { chat.live(json!({ "mcp_reconnect": name }), "MCP reconnect"); pop.set(None); }>"Reconnect"</button>
                            })}
                            {(live && (m.status == "connected" || disabled)).then(|| view! {
                                <button class="mcp-act" on:click=move |_| { chat.live(json!({ "mcp_toggle": { "name": name2, "enabled": disabled } }), if disabled { "MCP enable" } else { "MCP disable" }); pop.set(None); }>
                                    {if disabled { "Enable" } else { "Disable" }}
                                </button>
                            })}
                        </div>
                    }
                }).collect_view().into_any()
            }}
            <div class="cp-none">"To authenticate, run claude in a terminal and use /mcp. Reconnect and disable only affect the running session."</div>
        }
    }
}

pub(super) fn style_pop(chat: Chat, pop: RwSignal<Option<Pop>>) -> impl IntoView {
    move || {
        let mut styles = chat.catalog.with(|c| {
            c.as_ref()
                .map(|c| c.output_styles.clone())
                .unwrap_or_default()
        });
        if styles.is_empty() {
            styles = vec!["default".into(), "Explanatory".into(), "Learning".into()];
        }
        let cur = {
            let p = chat.prefs.get();
            if p.style.is_empty() {
                "default".to_owned()
            } else {
                p.style
            }
        };
        view! {
            <div class="cp-sec">"Output styles"</div>
            {styles.into_iter().map(|x| {
                let on = x == cur;
                let label = if x == "default" { "Default".to_owned() } else { x.clone() };
                row_btn(label, None, None, on, move || {
                    let v = x.clone();
                    chat.set_prefs(|p| p.style = if v == "default" { String::new() } else { v.clone() }, Some((json!({ "output_style": v }), "output style")));
                    pop.set(None);
                })
            }).collect_view()}
        }
    }
}
