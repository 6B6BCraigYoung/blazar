use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::components::toast::toast;
use crate::download_model::{Speed, percent, progress_line};
use crate::files_js;

/// 下完的条目留这么久再自动收起；失败的一直留着，等用户看完关掉。
const DONE_LINGER_MS: u32 = 30_000;

#[derive(Clone, PartialEq)]
enum Phase {
    Running,
    Done,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Source {
    Desktop(u64),
    Browser,
}

#[derive(Clone, PartialEq)]
struct Item {
    key: u32,
    source: Source,
    name: String,
    dest: String,
    received: u64,
    total: Option<u64>,
    speed: Option<f64>,
    meter: Speed,
    phase: Phase,
}

#[derive(Clone, Copy)]
struct Downloads {
    list: RwSignal<Vec<Item>>,
    next: StoredValue<u32>,
}

thread_local! {
    static DOWNLOADS: Cell<Option<Downloads>> = const { Cell::new(None) };
    static ABORTS: RefCell<HashMap<u32, web_sys::AbortController>> = RefCell::new(HashMap::new());
}

pub fn provide() {
    DOWNLOADS.set(Some(Downloads {
        list: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
    }));
}

/// 树里的「下载」：先让用户选存到哪，取消就什么都不做；选好后在右下角显示进度。
/// 桌面端走原生面板 + 桌面进程写盘；浏览器支持 showSaveFilePicker 就流式写入，否则退回 <a download>。
pub fn start(url: String, name: String) {
    if crate::desktop::available() {
        spawn_local(desktop(url, name));
    } else if files_js::can_pick_save_file() {
        spawn_local(browser(url, name));
    } else {
        files_js::download_url(&url, &name);
    }
}

fn js_message(v: &JsValue) -> String {
    v.as_string()
        .or_else(|| {
            js_sys::Reflect::get(v, &"message".into())
                .ok()
                .and_then(|m| m.as_string())
        })
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "未知错误".into())
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

fn next_key(d: Downloads) -> u32 {
    let key = d.next.get_value();
    d.next.set_value(key.wrapping_add(1));
    key
}

fn insert(d: Downloads, item: Item) {
    d.list.update(|l| l.push(item));
}

fn edit(d: Downloads, key: u32, f: impl FnOnce(&mut Item)) {
    d.list.try_update(|l| {
        if let Some(item) = l.iter_mut().find(|x| x.key == key) {
            f(item);
        }
    });
}

fn remove(d: Downloads, key: u32) {
    d.list.try_update(|l| l.retain(|x| x.key != key));
}

fn advance(item: &mut Item, received: u64, total: Option<u64>) {
    item.received = received;
    item.total = total.or(item.total);
    item.speed = item.meter.push(now_ms(), received);
}

fn finish(d: Downloads, key: u32, dest: Option<String>, received: Option<u64>) {
    edit(d, key, |item| {
        if let Some(dest) = dest {
            item.dest = dest;
        }
        if let Some(received) = received {
            item.received = received;
        }
        item.phase = Phase::Done;
    });
    gloo_timers::callback::Timeout::new(DONE_LINGER_MS, move || {
        if let Some(d) = DOWNLOADS.get() {
            d.list
                .try_update(|l| l.retain(|x| x.key != key || x.phase != Phase::Done));
        }
    })
    .forget();
}

fn fail(d: Downloads, key: u32, why: String) {
    edit(d, key, |item| item.phase = Phase::Failed(why));
}

fn on_desktop_event(d: Downloads, key: u32, name: &str, ev: &Value) {
    let text = |k: &str| ev[k].as_str().unwrap_or_default().to_owned();
    let id = ev["id"].as_u64().unwrap_or_default();
    let fresh = |phase: Phase| Item {
        key,
        source: Source::Desktop(id),
        name: Some(text("name"))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| name.to_owned()),
        dest: text("path"),
        received: 0,
        total: ev["total"].as_u64(),
        speed: None,
        meter: Speed::default(),
        phase,
    };
    match ev["kind"].as_str() {
        Some("started") => insert(d, fresh(Phase::Running)),
        Some("progress") => edit(d, key, |item| {
            advance(
                item,
                ev["received"].as_u64().unwrap_or_default(),
                ev["total"].as_u64(),
            );
        }),
        Some("done") => finish(d, key, Some(text("path")), ev["received"].as_u64()),
        Some("failed") => {
            // 没收到 started 就失败（比如 hub 直接拒绝：太大、不是文件）时，也要留一条说明原因。
            if d.list.with_untracked(|l| l.iter().any(|x| x.key == key)) {
                fail(d, key, text("error"));
            } else {
                insert(d, fresh(Phase::Failed(text("error"))));
            }
        }
        Some("cancelled") => remove(d, key),
        _ => {}
    }
}

async fn desktop(url: String, name: String) {
    let Some(d) = DOWNLOADS.get() else { return };
    let key = next_key(d);
    let suggested = name.clone();
    let on_event = Closure::<dyn Fn(JsValue)>::new(move |v: JsValue| {
        let text = js_sys::JSON::stringify(&v)
            .ok()
            .and_then(|s| s.as_string())
            .unwrap_or_default();
        if let Ok(ev) = serde_json::from_str::<Value>(&text) {
            on_desktop_event(d, key, &suggested, &ev);
        }
    });
    let promise = files_js::desktop_save_download(&url, &name, on_event.as_ref().unchecked_ref());
    // 下载在后台继续，进度回调要一直活到桌面端不再推送为止。
    on_event.forget();
    let result = match promise {
        Ok(p) => wasm_bindgen_futures::JsFuture::from(p).await,
        Err(e) => Err(e),
    };
    if let Err(e) = result {
        toast(format!("没能下载：{}", js_message(&e)));
    }
}

async fn browser(url: String, name: String) {
    let Some(d) = DOWNLOADS.get() else { return };
    let handle = match wasm_bindgen_futures::JsFuture::from(files_js::pick_save_file(&name)).await {
        Ok(h) if h.is_null() || h.is_undefined() => return,
        Ok(h) => h,
        Err(e) => {
            toast(format!("没能下载：{}", js_message(&e)));
            return;
        }
    };
    let Ok(abort) = web_sys::AbortController::new() else {
        files_js::download_url(&url, &name);
        return;
    };
    let key = next_key(d);
    insert(
        d,
        Item {
            key,
            source: Source::Browser,
            name: name.clone(),
            dest: files_js::saved_name(&handle),
            received: 0,
            total: None,
            speed: None,
            meter: Speed::default(),
            phase: Phase::Running,
        },
    );
    ABORTS.with_borrow_mut(|a| a.insert(key, abort.clone()));
    let on_progress = Closure::<dyn Fn(f64, JsValue)>::new(move |received: f64, total: JsValue| {
        let total = total.as_f64().map(|t| t as u64);
        edit(d, key, |item| advance(item, received as u64, total));
    });
    let result = wasm_bindgen_futures::JsFuture::from(files_js::save_to_file(
        &handle,
        &url,
        &abort.signal(),
        on_progress.as_ref().unchecked_ref(),
    ))
    .await;
    ABORTS.with_borrow_mut(|a| a.remove(&key));
    drop(on_progress);
    match result {
        Ok(received) => finish(d, key, None, received.as_f64().map(|r| r as u64)),
        Err(_) if abort.signal().aborted() => remove(d, key),
        Err(e) => fail(d, key, js_message(&e)),
    }
}

fn cancel(d: Downloads, key: u32, source: Source) {
    match source {
        Source::Desktop(id) => spawn_local(async move {
            if let Err(e) =
                crate::desktop::invoke("cancel_download", &serde_json::json!({ "id": id })).await
            {
                toast(format!("没能取消：{e}"));
            }
        }),
        Source::Browser => {
            if let Some(abort) = ABORTS.with_borrow(|a| a.get(&key).cloned()) {
                abort.abort();
            } else {
                remove(d, key);
            }
        }
    }
}

fn reveal(id: u64) {
    spawn_local(async move {
        if let Err(e) =
            crate::desktop::invoke("reveal_download", &serde_json::json!({ "id": id })).await
        {
            toast(format!("没能在访达中显示：{e}"));
        }
    });
}

fn row(d: Downloads, key: u32) -> impl IntoView {
    let item = Memo::new(move |_| d.list.with(|l| l.iter().find(|x| x.key == key).cloned()));
    let phase = Memo::new(move |_| item.with(|i| i.as_ref().map(|i| i.phase.clone())));
    let (name, dest, source) = item
        .get_untracked()
        .map(|i| (i.name, i.dest, i.source))
        .unwrap_or((String::new(), String::new(), Source::Browser));
    let shown_dest = move || {
        item.with(|i| i.as_ref().map(|i| i.dest.clone()))
            .unwrap_or_else(|| dest.clone())
    };
    let pct = move || item.with(|i| i.as_ref().and_then(|i| percent(i.received, i.total)));
    let line = move || {
        item.with(|i| {
            i.as_ref()
                .map(|i| progress_line(i.received, i.total, i.speed))
                .unwrap_or_default()
        })
    };
    let close_label = format!("关闭 {name}");
    let cancel_label = format!("取消下载 {name}");
    view! {
        <div
            class="dl-item"
            class:failed=move || matches!(phase.get(), Some(Phase::Failed(_)))
            role="group"
            aria-label=name.clone()
        >
            <div class="dl-head">
                <span class="dl-name" title=name.clone()>
                    {name.clone()}
                </span>
                {move || {
                    if phase.get() == Some(Phase::Running) {
                        view! {
                            <button
                                type="button"
                                class="btn small ghost dl-cancel"
                                aria-label=cancel_label.clone()
                                on:click=move |_| cancel(d, key, source)
                            >
                                "取消"
                            </button>
                        }
                            .into_any()
                    } else {
                        view! {
                            <button
                                type="button"
                                class="dl-x"
                                aria-label=close_label.clone()
                                title="关闭"
                                on:click=move |_| remove(d, key)
                            >
                                "×"
                            </button>
                        }
                            .into_any()
                    }
                }}
            </div>
            {move || match phase.get() {
                Some(Phase::Running) => {
                    view! {
                        <div
                            class="dl-bar"
                            class:indeterminate=move || pct().is_none()
                            role="progressbar"
                            aria-valuemin="0"
                            aria-valuemax="100"
                            aria-valuenow=move || pct().map(|p| p.to_string())
                            aria-valuetext=line
                        >
                            <span style:width=move || {
                                pct().map(|p| format!("{p}%")).unwrap_or_default()
                            }></span>
                        </div>
                        <div class="dl-meta">{line}</div>
                    }
                        .into_any()
                }
                Some(Phase::Done) => {
                    view! {
                        <div class="dl-meta" title=shown_dest.clone()>
                            "已保存到 "
                            {shown_dest.clone()}
                        </div>
                        {match source {
                            Source::Desktop(id) => {
                                Some(
                                    view! {
                                        <div class="dl-acts">
                                            <button
                                                type="button"
                                                class="btn small"
                                                on:click=move |_| reveal(id)
                                            >
                                                "在访达中显示"
                                            </button>
                                        </div>
                                    },
                                )
                            }
                            Source::Browser => None,
                        }}
                    }
                        .into_any()
                }
                Some(Phase::Failed(why)) => {
                    view! { <div class="dl-meta err">"下载失败：" {why}</div> }.into_any()
                }
                None => ().into_any(),
            }}
        </div>
    }
}

#[component]
pub fn DownloadHost() -> impl IntoView {
    let d = DOWNLOADS.get().expect("downloads::provide 还没调用");
    view! {
        <div class="downloads" aria-live="polite" aria-label="下载">
            <For
                each=move || d.list.with(|l| l.iter().map(|x| x.key).collect::<Vec<_>>())
                key=|k| *k
                children=move |k| row(d, k)
            />
        </div>
    }
}
