use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::app_state::use_app;

use super::modal::Modal;
use super::status::EmptyState;

#[derive(Clone)]
struct Hit {
    title: String,
    kind: String,
    href: String,
}

#[component]
pub fn Palette() -> impl IntoView {
    let app = use_app();
    let q = RwSignal::new(String::new());
    let sel = RwSignal::new(0usize);
    let navigate = use_navigate();

    let global_navigate = navigate.clone();
    let keys = window_event_listener(ev::keydown, move |e| match crate::shortcuts::action(&e) {
        Some("palette") => {
            e.prevent_default();
            app.palette.update(|p| *p = !*p);
        }
        Some("side") => {
            e.prevent_default();
            app.side_collapsed.update(|p| *p = !*p);
        }
        Some(action @ ("inbox" | "tasks" | "workspaces")) => {
            e.prevent_default();
            if !crate::files_js::confirm_navigation() {
                return;
            }
            global_navigate(
                if action == "workspaces" {
                    "/"
                } else if action == "inbox" {
                    "/inbox"
                } else {
                    "/tasks"
                },
                Default::default(),
            );
        }
        _ => {}
    });
    on_cleanup(move || keys.remove());
    Effect::new(move |_| {
        if app.palette.get() {
            q.set(String::new());
            sel.set(0);
        }
    });

    let hits = move || {
        let mut all: Vec<Hit> = [
            ("全部工作区", "导航", "/"),
            ("运行中", "导航", "/running"),
            ("等我审批", "导航", "/waiting"),
            ("运行时", "导航", "/runtimes"),
            ("收件箱", "导航", "/inbox"),
            ("任务", "导航", "/tasks"),
            ("自动化", "导航", "/autopilots"),
            ("SKILLs", "导航", "/skills"),
            ("接入的软件", "导航", "/apps"),
            ("Agent", "导航", "/agents"),
            ("机器与组网", "导航", "/nodes"),
            ("用量", "导航", "/usage"),
            ("设置", "导航", "/settings"),
            ("新建工作区", "命令", "cmd:new"),
        ]
        .into_iter()
        .map(|(t, k, h)| Hit {
            title: t.into(),
            kind: k.into(),
            href: h.into(),
        })
        .collect();
        if let Some(s) = app.state.get() {
            all.extend(s.workspaces.iter().map(|w| Hit {
                title: w.name.clone(),
                kind: format!("工作区 · {}", w.node),
                href: format!("/w/{}", w.id),
            }));
            all.extend(s.nodes.iter().map(|n| Hit {
                title: n.name.clone(),
                kind: "机器".into(),
                href: format!("/nodes/{}", js_sys::encode_uri_component(&n.name)),
            }));
        }
        let k = q.get().to_lowercase();
        all.into_iter()
            .filter(|h| {
                k.is_empty() || format!("{}{}", h.title, h.kind).to_lowercase().contains(&k)
            })
            .take(40)
            .collect::<Vec<_>>()
    };
    let go = StoredValue::new_local(move |h: &Hit| {
        if h.href != "cmd:new" && !crate::files_js::confirm_navigation() {
            return;
        }
        app.palette.set(false);
        if h.href == "cmd:new" {
            app.new_ws.set(true);
        } else if h.href.starts_with("/") {
            navigate(&h.href, Default::default());
        } else {
            let _ = window().location().set_href(&h.href);
        }
    });
    view! {
        <Show when=move || app.palette.get()>
            <Modal label="搜索" class="palette" mask_class="dlg-mask top" on_close=Callback::new(move |_| app.palette.set(false))>
                    <input data-modal-initial-focus="" aria-label="搜索工作区、机器和页面" placeholder="搜索工作区、机器、页面…" prop:value=move || q.get()
                        on:input=move |e| { q.set(event_target_value(&e)); sel.set(0); }
                        on:keydown=move |e| {
                            let n = hits().len().max(1);
                            match e.key().as_str() {
                                "ArrowDown" => { e.prevent_default(); sel.update(|s| *s = (*s + 1) % n); }
                                "ArrowUp" => { e.prevent_default(); sel.update(|s| *s = (*s + n - 1) % n); }
                                "Enter" if !e.is_composing() => { e.prevent_default(); if let Some(h) = hits().get(sel.get_untracked()) { go.with_value(|g| g(h)); } }
                                _ => {}
                            }
                        }/>
                    <div class="pal-list">
                        {move || {
                            let list = hits();
                            if list.is_empty() {
                                return view! { <EmptyState title="无匹配"/> }.into_any();
                            }
                            list.into_iter().enumerate().map(|(i, h)| {
                                let h2 = h.clone();
                                view! {
                                    <button type="button" class="palrow" data-sel=move || (sel.get() == i).to_string() on:click=move |_| go.with_value(|g| g(&h2))>
                                        <span class="t">{h.title.clone()}</span><span class="k">{h.kind.clone()}</span>
                                    </button>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
            </Modal>
        </Show>
    }
}

#[component]
pub fn AlertsDialog() -> impl IntoView {
    use crate::alerts::{self, SoundPrefs};
    use crate::components::toast::toast;
    let app = use_app();
    let prefs = RwSignal::new(alerts::sound_prefs());
    let sys = RwSignal::new(alerts::notify_on());
    let save = move |p: SoundPrefs| {
        alerts::set_sound_prefs(&p);
        prefs.set(p);
        app.alerts_rev.update(|n| *n += 1);
    };
    let toggle_sys = move |_| {
        leptos::task::spawn_local(async move {
            if alerts::notify_on() {
                alerts::set_notify(false);
                sys.set(false);
            } else if !alerts::notify_supported() {
                toast("这个浏览器不支持系统通知");
            } else if alerts::request_permission().await == "granted" {
                alerts::set_notify(true);
                sys.set(true);
                toast("已开启：跑完、出错、等你裁决时会弹系统通知");
            } else {
                toast("浏览器没有给通知权限，可以在地址栏左侧的站点设置里打开");
            }
            app.alerts_rev.update(|n| *n += 1);
        });
    };
    view! {
        <Show when=move || app.alerts_open.get()>
            <Modal label="提醒方式" on_close=Callback::new(move |_| app.alerts_open.set(false))>
                    <h3>"提醒方式"</h3>
                    <label class="chk block"><input type="checkbox" prop:checked=move || sys.get() on:change=toggle_sys/>
                        <span><b>"系统通知"</b><span class="muted small">" 完成或需要处理时通知你"</span></span></label>
                    <label class="chk block"><input type="checkbox" prop:checked=move || prefs.get().on
                        on:change=move |_| { let mut p = prefs.get_untracked(); p.on = !p.on; let on = p.on; save(p); if on { alerts::play("done", true); } }/>
                        <span><b>"提示音"</b><span class="muted small">" 区分完成和待处理"</span></span></label>
                    <div class="row-actions">
                        <span class="muted small">"音色"</span>
                        <span class="seg">
                            {[("soft", "柔和"), ("bright", "清脆"), ("wood", "木质")].into_iter().map(|(k, l)| view! {
                                <button type="button" data-on=move || (prefs.get().tone == k).to_string() on:click=move |_| { let mut p = prefs.get_untracked(); p.tone = k.into(); save(p); alerts::play("done", true); }>{l}</button>
                            }).collect_view()}
                        </span>
                        <span class="muted small">"音量"</span>
                        <input aria-label="提示音音量" type="range" min="0.05" max="1" step="0.05" prop:value=move || prefs.get().volume.to_string()
                            on:change=move |e| { let mut p = prefs.get_untracked(); p.volume = event_target_value(&e).parse().unwrap_or(0.5); save(p); alerts::play("done", true); }/>
                    </div>
                    <div class="row-actions">
                        <button type="button" class="btn small" on:click=move |_| alerts::play("done", true)>"试听完成提示"</button>
                        <button type="button" class="btn small" on:click=move |_| alerts::play("attention", true)>"试听待处理提示"</button>
                    </div>
                    <div class="dlg-foot"><button type="button" class="btn primary" on:click=move |_| app.alerts_open.set(false)>"完成"</button></div>
            </Modal>
        </Show>
    }
}
