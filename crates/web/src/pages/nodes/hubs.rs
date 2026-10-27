use super::*;
use crate::api::Accounts;
use crate::components::prefs::PrefGroup;
use crate::fmt;
use crate::pages::runtimes::{WITH_ACCOUNTS, account_status, active_account};

const NOTE: &str =
    "每个 Blazar 各管各的账号；在这里看它们用的是哪个，也可以直接改，请求经本机 hub 转发过去。";
const NOTE_READ_ONLY: &str =
    "每个 Blazar 各管各的账号；这是开发实例，只能看它们用的是哪个，要改请到正式的 Blazar 里改。";

#[derive(Clone, PartialEq)]
struct Hub {
    session: String,
    url: String,
    label: String,
    accounts: Accounts,
}

#[derive(Clone, PartialEq)]
struct Listing {
    hubs: Vec<Hub>,
    read_only: bool,
}

fn parse(v: &Value) -> Listing {
    let hubs = v["hubs"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|h| {
                    let accounts: Accounts = serde_json::from_value(
                        json!({ "accounts": h["accounts"], "modes": h["modes"] }),
                    )
                    .ok()?;
                    Some(Hub {
                        session: s(h, "session"),
                        url: s(h, "url"),
                        label: s(h, "label"),
                        accounts,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Listing {
        hubs,
        read_only: v["read_only"].as_bool().unwrap_or(false),
    }
}

fn chosen(h: &Hub, provider: &str) -> String {
    if h.accounts.modes.get(provider).is_some_and(|m| m == "auto") {
        return "自动轮换".to_owned();
    }
    match active_account(&h.accounts, provider) {
        Some(a) if a.builtin => "默认登录".to_owned(),
        Some(a) => a.label,
        None => "已不存在".to_owned(),
    }
}

fn mode_line(h: &Hub, provider: &str) -> String {
    let mode = h.accounts.modes.get(provider).cloned().unwrap_or_default();
    if mode == "auto" {
        return "自动轮换：每次开始前挑额度最充裕的账号".to_owned();
    }
    let Some(a) = active_account(&h.accounts, provider) else {
        return "绑定的账号已经不存在，去那边重新选一个".to_owned();
    };
    let (status, _) = account_status(&a);
    let who = if a.builtin {
        "默认登录".to_owned()
    } else {
        a.label.clone()
    };
    let checked = a
        .checked_at
        .as_deref()
        .map(|t| format!("检查于 {}", fmt::ago(t)))
        .unwrap_or_else(|| "没检查过".to_owned());
    format!("{who} · {status} · {checked}")
}

#[component]
pub fn NodeHubs(name: String, online: Signal<bool>) -> impl IntoView {
    let nm = StoredValue::new(name);
    let hubs = RwSignal::new(None::<Result<Listing, String>>);
    let loading = RwSignal::new(false);
    let load = move || {
        if loading.try_get_untracked().unwrap_or(true) {
            return;
        }
        loading.set(true);
        let n = nm.get_value();
        spawn_local(async move {
            let r = api::get::<Value>(&format!("/api/nodes/{}/hubs", api::enc(&n)))
                .await
                .map(|v| parse(&v))
                .map_err(|e| e.to_string());
            let _ = hubs.try_set(Some(r));
            loading.try_set(false);
        });
    };
    let loaded = RwSignal::new(false);
    Effect::new(move |_| {
        if online.get() && !loaded.get_untracked() {
            loaded.set(true);
            load();
        }
    });
    let switching = RwSignal::new(false);
    let switch = move |session: String, provider: String, mode: String| {
        if switching.get_untracked() {
            return;
        }
        switching.set(true);
        let n = nm.get_value();
        spawn_local(async move {
            match api::send::<Value>(
                "PUT",
                &format!("/api/nodes/{}/hubs/mode", api::enc(&n)),
                &json!({ "session": session, "provider": provider, "mode": mode }),
            )
            .await
            {
                Ok(_) => toast("那边的账号设置已改好"),
                Err(e) => toast(format!("改不了：{e}")),
            }
            switching.try_set(false);
            load();
        });
    };
    let rescan = move || {
        view! {
            <PrefRow
                label="重新查找"
                detail="那边加了账号或改了设置后，重新读一次。"
            >
                <button
                    type="button"
                    class="btn"
                    disabled=move || loading.get() || !online.get()
                    on:click=move |_| load()
                >
                    "重新查找"
                </button>
            </PrefRow>
        }
    };
    let read_only = Memo::new(move |_| hubs.with(|h| matches!(h, Some(Ok(l)) if l.read_only)));
    let body = move || match hubs.get() {
        None if loading.get() => view! { <p class="pref-status">"正在查找…"</p> }.into_any(),
        None => view! {
            <PrefRow
                label="尚未查找"
                detail="读取这台机器上其他 Blazar 的账号设置。"
            >
                <button
                    type="button"
                    class="btn"
                    disabled=move || !online.get()
                    on:click=move |_| load()
                >
                    "查找"
                </button>
            </PrefRow>
        }
        .into_any(),
        Some(Err(e)) => view! {
            <InlineError
                message=format!("查找失败：{e}")
                retry=Callback::new(move |()| load())
            />
        }
        .into_any(),
        Some(Ok(l)) if l.hubs.is_empty() => view! {
            <PrefRow
                label="没有发现"
                detail="这台机器上没有别的正在运行的 Blazar。"
            />
            {rescan()}
        }
        .into_any(),
        Some(Ok(l)) => {
            let read_only = l.read_only;
            view! {
                {l
                    .hubs
                    .into_iter()
                    .enumerate()
                    .map(|(i, h)| {
                        let rows = WITH_ACCOUNTS
                            .iter()
                            .map(|(provider, plabel)| {
                                let detail = mode_line(&h, provider);
                                if read_only {
                                    let who = chosen(&h, provider);
                                    return view! {
                                        <PrefRow label=(*plabel).to_owned() detail=detail>
                                            <span class="pref-text">{who}</span>
                                        </PrefRow>
                                    }
                                        .into_any();
                                }
                                let current = h
                                    .accounts
                                    .modes
                                    .get(*provider)
                                    .cloned()
                                    .unwrap_or_default();
                                let choices: Vec<(String, String)> = [
                                    ("auto".to_owned(), "自动轮换".to_owned()),
                                    (String::new(), "默认登录".to_owned()),
                                ]
                                    .into_iter()
                                    .chain(
                                        h
                                            .accounts
                                            .accounts
                                            .iter()
                                            .filter(|a| {
                                                a.provider == *provider && !a.builtin && !a.disabled
                                            })
                                            .map(|a| (a.id.clone(), a.label.clone())),
                                    )
                                    .collect();
                                let (session, prov) = (h.session.clone(), (*provider).to_owned());
                                let select_id = format!("peer-hub-{i}-{provider}");
                                view! {
                                    <PrefRow
                                        label=(*plabel).to_owned()
                                        detail=detail
                                        for_id=select_id.clone()
                                    >
                                        <select
                                            id=select_id
                                            class="settings-input"
                                            disabled=move || switching.get()
                                            prop:value=current.clone()
                                            on:change=move |e| {
                                                switch(
                                                    session.clone(),
                                                    prov.clone(),
                                                    event_target_value(&e),
                                                )
                                            }
                                        >
                                            {choices
                                                .into_iter()
                                                .map(|(value, label)| {
                                                    let on = value == current;
                                                    view! {
                                                        <option value=value selected=on>
                                                            {label}
                                                        </option>
                                                    }
                                                })
                                                .collect_view()}
                                        </select>
                                    </PrefRow>
                                }
                                    .into_any()
                            })
                            .collect_view();
                        view! {
                            <PrefRow label=h.label.clone() detail=h.url.clone() />
                            {rows}
                        }
                    })
                    .collect_view()}
                {rescan()}
            }
            .into_any()
        }
    };
    view! {
        {move || {
            view! {
                <PrefGroup
                    title="这台机器上的其他 Blazar"
                    note=if read_only.get() { NOTE_READ_ONLY } else { NOTE }
                    trailing=move || view! { <Checking busy=loading /> }
                >
                    {body}
                </PrefGroup>
            }
        }}
    }
}
