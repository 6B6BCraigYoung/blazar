use leptos::prelude::*;

use crate::components::modal::Modal;
use crate::components::status::{EmptyState, InlineError, LoadingState};
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api;
use crate::components::toast::toast;
use crate::fmt;
use crate::realtime::use_bus;
use crate::storage;

mod script_draft;
use script_draft::{ScriptDraft, Scripts};

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
struct Dev {
    running: bool,
    preview_url: Option<String>,
    reason: String,
    tunnel: Option<String>,
    tunnel_error: Option<String>,
    node: String,
    port: Option<u32>,
    log: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct PvPrefs {
    device: String,
    log: bool,
}

impl Default for PvPrefs {
    fn default() -> Self {
        Self {
            device: "desktop".into(),
            log: false,
        }
    }
}

const DEVICES: [(&str, &str, &str, &str); 3] = [
    ("desktop", "桌面", "100%", "100%"),
    ("mobile", "手机", "390px", "844px"),
    ("fluid", "自适应", "70%", "100%"),
];

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct ScriptRun {
    kind: String,
    status: String,
    #[serde(default)]
    trigger: String,
    #[serde(default)]
    exit_code: Option<i64>,
    #[serde(default)]
    started_at: String,
    #[serde(default)]
    output: String,
}

#[component]
pub fn PreviewBar(
    ws: String,
    active: Signal<bool>,
    panel_max: RwSignal<bool>,
    isolated: Signal<bool>,
) -> impl IntoView {
    let dev = RwSignal::new(None::<Dev>);
    let load_error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(None::<&'static str>);
    let url_override = RwSignal::new(String::new());
    let prefs = RwSignal::new(storage::load::<PvPrefs>("blazar.preview").unwrap_or_default());
    let reload = RwSignal::new(0u32);
    let scripts_open = RwSignal::new(false);
    let ws = StoredValue::new(ws);
    let bus = use_bus();

    let load = move || {
        let id = ws.get_value();
        spawn_local(async move {
            let d = match api::get::<Dev>(&format!("/api/workspaces/{id}/dev")).await {
                Ok(dev) => {
                    let _ = load_error.try_set(None);
                    dev
                }
                Err(error) => {
                    let _ = load_error.try_set(Some(error.to_string()));
                    Dev {
                        reason: error.to_string(),
                        ..Dev::default()
                    }
                }
            };
            let _ = dev.try_set(Some(d));
        });
    };
    let sub = bus.subscribe(move |ev| {
        if let blazar_core_types::api::ServerEvent::ScriptsChanged { workspace_id } = ev
            && workspace_id.to_string() == ws.get_value()
            && active.get_untracked()
            && busy.get_untracked().is_none()
        {
            load();
        }
    });
    on_cleanup(move || bus.unsubscribe(sub));
    Effect::new(move |_| {
        if active.get() {
            load();
        }
    });

    let set_prefs = move |f: &dyn Fn(&mut PvPrefs)| {
        prefs.update(|p| f(p));
        storage::save("blazar.preview", &prefs.get_untracked());
    };
    let start_stop = move |a: &'static str| {
        busy.set(Some(a));
        let id = ws.get_value();
        spawn_local(async move {
            match api::send::<Dev>("POST", &format!("/api/workspaces/{id}/dev/{a}"), &json!({}))
                .await
            {
                Ok(d) => {
                    let _ = load_error.try_set(None);
                    let need_poll = a == "start" && d.preview_url.is_none();
                    dev.set(Some(d));
                    if a == "stop" {
                        url_override.set(String::new());
                    }
                    if need_poll {
                        for _ in 0..8 {
                            gloo_timers::future::TimeoutFuture::new(1200).await;
                            let Ok(d) = api::get::<Dev>(&format!("/api/workspaces/{id}/dev")).await
                            else {
                                break;
                            };
                            let done = d.preview_url.is_some() || !d.running;
                            let _ = dev.try_set(Some(d));
                            if done {
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    toast(e.to_string());
                    if e.to_string().contains("还没配置") {
                        scripts_open.set(true);
                    }
                }
            }
            let _ = busy.try_set(None);
        });
    };
    let url = move || {
        let o = url_override.get();
        if o.is_empty() {
            dev.with(|d| d.as_ref().and_then(|d| d.preview_url.clone()))
                .unwrap_or_default()
        } else {
            o
        }
    };
    let running = move || dev.with(|d| d.as_ref().is_some_and(|d| d.running));

    view! {
        <div class="diffbar pvbar">
            <button class="btn small" class:primary=move || !running() disabled=move || busy.get().is_some()
                on:click=move |_| start_stop(if running() { "stop" } else { "start" })>
                {move || match busy.get() { Some("start") => "启动中…", Some("stop") => "停止中…", _ if running() => "停止", _ => "启动预览" }}
            </button>
            <input class="pv-url" aria-label="预览地址" title="输入地址后按 Enter 打开" spellcheck="false" prop:value=url
                placeholder=move || if running() { "输入预览地址…" } else { "启动后自动填入地址" }
                on:keydown=move |e| if e.key() == "Enter" && !e.is_composing() { url_override.set(event_target_value(&e).trim().to_owned()); reload.update(|n| *n += 1); }/>
            <button class="laybtn" aria-label="刷新预览" title="刷新页面" inner_html=super::ICON_REFRESH on:click=move |_| { reload.update(|n| *n += 1); load(); }></button>
            <span class="seg">
                {DEVICES.iter().map(|&(k, label, _, _)| view! {
                    <button aria-pressed=move || (prefs.get().device == k).to_string() data-on=move || (prefs.get().device == k).to_string() on:click=move |_| set_prefs(&|p| p.device = k.into())>{label}</button>
                }).collect_view()}
            </span>
            <span class="grow"></span>
            {move || dev.get().and_then(|d| d.tunnel.clone().map(|t| view! {
                <span class="gchip info" title=format!("远端工作区：hub 起了一条 ssh 隧道，把 {} 的 {} 端口接到本机 {t}", d.node, d.port.unwrap_or(0))>"远端已连接"</span>
            }))}
            {move || dev.get().and_then(|d| d.tunnel_error.map(|e| view! { <span class="gchip bad" title=e>"连接失败"</span> }))}
            <button class="btn small" aria-pressed=move || prefs.get().log.to_string() on:click=move |_| { set_prefs(&|p| p.log = !p.log); load(); }>"日志"</button>
            <button class="btn small" disabled=move || url().is_empty() on:click=move |_| { let _ = window().open_with_url_and_target(&url(), "_blank"); }>"在浏览器打开"</button>
            <button class="btn small" aria-haspopup="dialog" on:click=move |_| scripts_open.set(true)>"脚本设置"</button>
            <button class="laybtn" title=move || if panel_max.get() { "还原面板" } else { "最大化面板" } aria-pressed=move || panel_max.get().to_string()
                on:click=move |_| panel_max.update(|m| *m = !*m)>"⤢"</button>
        </div>
        <div class="pvbody">
            {move || {
                let u = url();
                let p = prefs.get();
                if dev.get().is_none() && u.is_empty() { return view! { <LoadingState text="读取预览状态…"/> }.into_any(); }
                let d = dev.get().unwrap_or_default();
                let log = (p.log && !d.log.is_empty()).then(|| {
                    let tail: String = d.log.chars().rev().take(6000).collect::<Vec<_>>().into_iter().rev().collect();
                    view! { <pre class="pv-log">{tail}</pre> }
                });
                if u.is_empty() {
                    if let Some(error) = load_error.get() { return view! { <InlineError message=error class="empty" retry=Callback::new(move |_| load())/>{log} }.into_any(); }
                    let msg = if d.running {
                        "预览已启动。查看日志，或在上方输入地址。".to_owned()
                    } else if !d.reason.is_empty() {
                        d.reason.clone()
                    } else {
                        "在「脚本设置」中填写启动命令，然后启动预览。".to_owned()
                    };
                    return view! { <EmptyState title=msg/>{log} }.into_any();
                }
                let (_, _, w, h) = DEVICES.iter().find(|x| x.0 == p.device).copied().unwrap_or(DEVICES[0]);
                reload.track();
                view! {
                    <div class="pv-stage" data-device=p.device.clone()>
                        <iframe src=u title="预览" style=format!("width:{w};height:{h}")
                            sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-modals allow-downloads"></iframe>
                    </div>
                    {log}
                }.into_any()
            }}
        </div>
        <Show when=move || scripts_open.get()>
            <ScriptsDialog ws=ws.get_value() isolated on_close=move || scripts_open.set(false)/>
        </Show>
    }
}

#[component]
fn ScriptsDialog(
    ws: String,
    isolated: Signal<bool>,
    on_close: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let cfg = RwSignal::new(ScriptDraft::default());
    let busy = RwSignal::new(false);
    let runs = RwSignal::new(Vec::<ScriptRun>::new());
    let ws = StoredValue::new(ws);
    let load_runs = move || {
        let id = ws.get_value();
        spawn_local(async move {
            if let Ok(r) =
                api::get::<Vec<ScriptRun>>(&format!("/api/workspaces/{id}/script-runs")).await
            {
                let _ = runs.try_set(r);
            }
        });
    };
    let load_config = move || {
        let id = ws.get_value();
        cfg.set(ScriptDraft::default());
        spawn_local(async move {
            let result = api::get::<Scripts>(&format!("/api/workspaces/{id}/scripts"))
                .await
                .map_err(|error| error.to_string());
            let _ = cfg.try_set(ScriptDraft::loaded(result));
        });
    };
    load_config();
    load_runs();
    let save = move || {
        let id = ws.get_value();
        let c = cfg
            .with_untracked(ScriptDraft::for_save)
            .map_err(api::ApiError);
        async move {
            let c = c?;
            api::send::<Value>("PUT", &format!("/api/workspaces/{id}/scripts"), &json!({ "setup": c.setup, "cleanup": c.cleanup, "dev": c.dev, "copy_files": c.copy_files })).await
        }
    };
    let blocked = move || busy.get() || !cfg.with(ScriptDraft::ready);
    let field = move |label: &'static str,
                      hint: &'static str,
                      ph: &'static str,
                      rows: &'static str,
                      get: fn(&Scripts) -> String,
                      set: fn(&mut Scripts, String)| {
        view! {
            <label class="field" title=hint>{label}
                <textarea class="mono" rows=rows placeholder=ph disabled=blocked prop:value=move || cfg.with(|draft| draft.value().map(get).unwrap_or_default()) on:input=move |e| { let v = event_target_value(&e); cfg.update(|draft| draft.edit(|c| set(c, v))); }></textarea>
            </label>
        }
    };
    let run = move |kind: &'static str| {
        if busy.get_untracked() || !cfg.with_untracked(ScriptDraft::ready) {
            return;
        }
        busy.set(true);
        let id = ws.get_value();
        let saved = save();
        spawn_local(async move {
            if let Err(e) = saved.await {
                toast(e.to_string());
                let _ = busy.try_set(false);
                return;
            }
            match api::send::<Value>(
                "POST",
                &format!("/api/workspaces/{id}/scripts/{kind}/run"),
                &json!({}),
            )
            .await
            {
                Ok(_) => {
                    toast("已开始，可在运行记录中查看结果");
                    if let Ok(result) =
                        api::get::<Vec<ScriptRun>>(&format!("/api/workspaces/{id}/script-runs"))
                            .await
                    {
                        let _ = runs.try_set(result);
                    }
                }
                Err(e) => toast(e.to_string()),
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <Modal label="脚本设置" class="dlg wide" on_close=Callback::new(move |_| on_close())>
                <h3>"脚本设置"</h3>
                <div class="dlg-body small">"脚本在工作区所在机器执行。同一仓库的独立工作区共用配置。"</div>
                {move || cfg.with(|draft| {
                    if let Some(error) = &draft.error {
                        view! {
                            <InlineError message=format!("读取脚本设置失败：{error}")/>
                            <button class="btn" disabled=move || busy.get() on:click=move |_| load_config()>"重试"</button>
                        }.into_any()
                    } else if !draft.ready() {
                        view! { <LoadingState text="读取脚本设置…" class="muted"/> }.into_any()
                    } else {
                        ().into_any()
                    }
                })}
                {field("准备脚本（Setup）", "独立工作区创建后、智能体开始前执行一次，例如安装依赖、生成代码。", "npm ci", "3", |c| c.setup.clone(), |c, v| c.setup = v)}
                {field("收尾脚本（Cleanup）", "每轮正常结束且有改动时执行，例如格式化、lint --fix。", "npm run format", "3", |c| c.cleanup.clone(), |c, v| c.cleanup = v)}
                {field("预览启动命令", "在预览面板启动服务时执行。", "npm run dev", "2", |c| c.dev.clone(), |c, v| c.dev = v)}
                {field("复制文件", "创建独立工作区时从源仓库复制被 Git 忽略的文件；每行一个路径，支持 glob。", ".env\nconfig/*.local.json", "3", |c| c.copy_files.clone(), |c, v| c.copy_files = v)}
                <div class="row-actions">
                    <span class="muted small">"保存并测试："</span>
                    <button class="btn small" disabled=blocked on:click=move |_| run("setup")>"Setup"</button>
                    <button class="btn small" disabled=blocked on:click=move |_| run("cleanup")>"Cleanup"</button>
                    <button class="btn small" disabled=move || blocked() || !isolated.get() title=move || if isolated.get() { "" } else { "只有隔离工作区才有源仓库可拷" } on:click=move |_| run("copy")>"拷贝文件"</button>
                </div>
                <h5 class="sub-h">"最近的运行"</h5>
                <div class="sc-runs">
                    {move || {
                        let r = runs.get();
                        if r.is_empty() {
                            return view! { <EmptyState title="暂无运行记录" class="muted small"/> }.into_any();
                        }
                        r.into_iter().map(|r| {
                            let (t, tone) = match r.status.as_str() { "ok" => ("成功", "ok"), "failed" => ("失败", "bad"), "timeout" => ("超时", "bad"), "running" => ("运行中", "info"), s => (s, "") };
                            let kind = match r.kind.as_str() { "setup" => "Setup", "cleanup" => "Cleanup", "copy" => "拷贝文件", k => k }.to_owned();
                            let trig = match r.trigger.as_str() { "manual" => "手动", "create" => "建工作区时", "turn_end" => "一轮结束后", t => t }.to_owned();
                            let code = r.exit_code.filter(|c| *c != 0).map(|c| format!(" · 退出码 {c}")).unwrap_or_default();
                            view! {
                                <details class="sc-run">
                                    <summary><span class=format!("gchip {tone}")>{t.to_owned()}</span><b>{kind}</b><span class="muted">{format!("{trig}{code}")}</span><span class="grow"></span><span class="muted small">{fmt::ago(&r.started_at)}</span></summary>
                                    <pre>{if r.output.is_empty() { "（没有输出）".to_owned() } else { r.output }}</pre>
                                </details>
                            }
                        }).collect_view().into_any()
                    }}
                </div>
                <div class="dlg-foot">
                    <button class="btn" on:click=move |_| on_close()>"关闭"</button>
                    <button class="btn primary" disabled=blocked on:click=move |_| {
                        if busy.get_untracked() || !cfg.with_untracked(ScriptDraft::ready) { return; }
                        busy.set(true);
                        let saved = save();
                        spawn_local(async move {
                            match saved.await {
                                Ok(_) => { toast("已保存"); if !busy.is_disposed() { on_close(); } }
                                Err(e) => toast(e.to_string()),
                            }
                            let _ = busy.try_set(false);
                        });
                    }>"保存"</button>
                </div>
        </Modal>
    }
}
