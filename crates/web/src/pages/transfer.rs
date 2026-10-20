use blazar_core_types::transfer::{
    ImportAction, ImportItem, ImportReport, MAX_IMPORT_BYTES, section_label,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::modal::Modal;
use crate::{api, files_js};

fn export_name() -> String {
    let now = js_sys::Date::new_0();
    format!(
        "blazar-export-{:04}{:02}{:02}.json",
        now.get_full_year(),
        now.get_month() + 1,
        now.get_date()
    )
}

#[derive(Clone)]
struct Pending {
    name: String,
    text: String,
    report: ImportReport,
}

#[component]
pub fn TransferSettings() -> impl IntoView {
    let picker = NodeRef::<leptos::html::Input>::new();
    let busy = RwSignal::new(false);
    let status = RwSignal::new((String::new(), false));
    let pending = RwSignal::new(None::<Pending>);
    let export = move |_| status.set((format!("已开始下载 {}", export_name()), false));
    let on_file = move |e: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&e);
        let Some(file) = input.files().and_then(|l| l.get(0)) else {
            return;
        };
        input.set_value("");
        if file.size() > MAX_IMPORT_BYTES as f64 {
            status.set((
                format!("文件超过 {} MB", MAX_IMPORT_BYTES / 1024 / 1024),
                true,
            ));
            return;
        }
        busy.set(true);
        status.set((format!("正在检查 {}…", file.name()), false));
        spawn_local(async move {
            let name = file.name();
            let result = match files_js::read_text(&file).await {
                Some(text) => {
                    api::post_text::<ImportReport>("/api/import?dry_run=true", text.clone())
                        .await
                        .map(|report| Pending { name, text, report })
                        .map_err(|e| e.message)
                }
                None => Err("读不了这个文件".to_owned()),
            };
            match result {
                Ok(p) => {
                    status.set((String::new(), false));
                    pending.set(Some(p));
                }
                Err(e) => status.set((e, true)),
            }
            busy.set(false);
        });
    };
    view! {
        <section class="card settings-card">
            <h3>"迁移配置"</h3>
            <p class="muted">
                "把智能体档案、skills、MCP、自动化、审批规则等配置导出成一个文件，再导入到另一台机器上的 hub。会话历史和账号不迁移，密钥的值导出时会清空。"
            </p>
            <div class="settings-actions">
                <a class="btn" href="/api/export" download="" on:click=export>
                    "导出"
                </a>
                <button
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| {
                        if let Some(input) = picker.get() {
                            input.click();
                        }
                    }
                >
                    "导入…"
                </button>
                <input
                    type="file"
                    node_ref=picker
                    accept=".json,application/json"
                    hidden
                    on:change=on_file
                />
            </div>
            <p class="settings-status" class:bad=move || status.get().1>
                {move || status.get().0}
            </p>
        </section>
        {move || {
            pending
                .get()
                .map(|p| {
                    view! {
                        <ImportDialog
                            pending=p
                            on_close=Callback::new(move |()| pending.set(None))
                        />
                    }
                })
        }}
    }
}

fn action_title(action: ImportAction) -> &'static str {
    match action {
        ImportAction::Create => "新建",
        ImportAction::Skip => "跳过",
        ImportAction::Attention => "需要处理",
    }
}

fn item_detail(item: &ImportItem) -> String {
    let mut parts = Vec::new();
    if !item.detail.is_empty() {
        parts.push(item.detail.clone());
    }
    if item.action == ImportAction::Attention && !item.imported {
        parts.push("不会导入".to_owned());
    }
    parts.join(" · ")
}

#[component]
fn ImportDialog(pending: Pending, on_close: Callback<()>) -> impl IntoView {
    let Pending { name, text, report } = pending;
    let report = RwSignal::new(report);
    let text = StoredValue::new(text);
    let busy = RwSignal::new(false);
    let status = RwSignal::new((String::new(), false));
    let close = Callback::new(move |()| {
        if !busy.get_untracked() {
            on_close.run(());
        }
    });
    let nothing = move || report.with(|r| r.items.iter().all(|i| !i.imported));
    let apply = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        status.set(("正在导入…".to_owned(), false));
        spawn_local(async move {
            match api::post_text::<ImportReport>("/api/import?dry_run=false", text.get_value())
                .await
            {
                Ok(r) => {
                    report.set(r);
                    status.set(("已导入。".to_owned(), false));
                }
                Err(e) => status.set((e.message, true)),
            }
            busy.set(false);
        });
    };
    view! {
        <Modal label="导入配置" class="dlg wide transfer-dialog" on_close=close>
            <h3>{move || if report.get().applied { "导入完成" } else { "导入预览" }}</h3>
            <p class="muted transfer-file">{name}</p>
            <div class="transfer-groups">
                {[ImportAction::Create, ImportAction::Skip, ImportAction::Attention]
                    .into_iter()
                    .map(|action| {
                        let items = move || {
                            report
                                .with(|r| {
                                    r.items
                                        .iter()
                                        .filter(|i| i.action == action)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                })
                        };
                        view! {
                            <details
                                class="transfer-group"
                                open=action == ImportAction::Attention
                            >
                                <summary>
                                    <span>{action_title(action)}</span>
                                    <span class="transfer-count">{move || items().len()}</span>
                                </summary>
                                <ul class="transfer-items">
                                    {move || {
                                        let list = items();
                                        if list.is_empty() {
                                            return view! { <li class="muted">"没有"</li> }
                                                .into_any();
                                        }
                                        list.into_iter()
                                            .map(|i| {
                                                let detail = item_detail(&i);
                                                view! {
                                                    <li>
                                                        <span class="transfer-section">
                                                            {section_label(&i.section).to_owned()}
                                                        </span>
                                                        <span class="transfer-key">{i.key.clone()}</span>
                                                        {(!detail.is_empty())
                                                            .then(|| {
                                                                view! {
                                                                    <span class="transfer-detail">{detail}</span>
                                                                }
                                                            })}
                                                    </li>
                                                }
                                            })
                                            .collect_view()
                                            .into_any()
                                    }}
                                </ul>
                            </details>
                        }
                    })
                    .collect_view()}
            </div>
            <ul class="transfer-notes">
                {move || {
                    report
                        .get()
                        .notes
                        .into_iter()
                        .map(|n| view! { <li>{n}</li> })
                        .collect_view()
                }}
            </ul>
            <p class="transfer-status" class:bad=move || status.get().1>
                {move || status.get().0}
            </p>
            <div class="dlg-foot">
                {move || {
                    if report.get().applied {
                        view! {
                            <button class="btn primary" on:click=move |_| on_close.run(())>
                                "完成"
                            </button>
                        }
                            .into_any()
                    } else {
                        view! {
                            <button
                                class="btn"
                                disabled=move || busy.get()
                                on:click=move |_| close.run(())
                            >
                                "取消"
                            </button>
                            <button
                                class="btn primary"
                                data-modal-initial-focus=""
                                disabled=move || busy.get() || nothing()
                                on:click=apply
                            >
                                "导入"
                            </button>
                        }
                            .into_any()
                    }
                }}
            </div>
        </Modal>
    }
}
