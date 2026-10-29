//! ⌘/: every key the app answers to on one read-only sheet, the global shortcuts from the
//! keymap and the keys that only work on a page (lists, approval cards, the message box).

use blazar_core_types::keymap::{self, Group};
use leptos::ev;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use serde_json::Value;

use super::modal::Modal;
use crate::shortcuts::label;

fn page_keys(g: Group) -> Vec<(String, &'static str)> {
    let plain = |k: &str| k.to_owned();
    match g {
        Group::Global => vec![(plain("Esc"), "关闭弹窗、菜单和面板")],
        Group::Chat => {
            let by_mod = crate::storage::load::<Value>("blazar.ui")
                .is_some_and(|p| p["sendKey"].as_str() == Some("mod"));
            let mut keys = if by_mod {
                vec![(label("Mod+Enter"), "发送"), (label("Enter"), "换行")]
            } else {
                vec![(label("Enter"), "发送"), (label("Shift+Enter"), "换行")]
            };
            keys.extend([
                (plain("↑ ↓"), "翻出之前发过的消息"),
                (label("Shift+Tab"), "切换模式"),
                (plain("Esc"), "停下正在跑的智能体"),
            ]);
            keys
        }
        Group::Console => vec![
            (plain("↑ ↓ / j k"), "选择上一张 / 下一张"),
            (label("Enter"), "打开"),
            (plain("1"), "允许"),
            (plain("2"), "总是允许"),
            (plain("3"), "拒绝"),
            (plain("e"), "标为已处理 / 归档"),
            (plain("u"), "标为已读 / 未读"),
        ],
        Group::Changes => Vec::new(),
    }
}

/// The global keys people reach for most, in that order; the sheet shows these first and
/// folds the rest under 「更多」 so the group stays short.
const GLOBAL_FIRST: [&str; 7] = [
    "palette",
    "newws",
    "console",
    "workspaces",
    "tasks",
    "settings",
    "side",
];
const GLOBAL_SHOWN: usize = 8;

fn rows(g: Group) -> Vec<(String, &'static str)> {
    let mut out: Vec<(&str, String, &'static str)> = crate::shortcuts::effective()
        .into_iter()
        .filter_map(|(id, combo)| {
            let b = keymap::binding(id)?;
            (b.group == g && !combo.is_empty()).then(|| (id, label(&combo), b.label))
        })
        .collect();
    if g == Group::Global {
        let rank = |id: &str| {
            GLOBAL_FIRST
                .iter()
                .position(|f| *f == id)
                .unwrap_or(usize::MAX)
        };
        out.sort_by_key(|(id, ..)| rank(id));
    }
    let mut out: Vec<(String, &'static str)> = out.into_iter().map(|(_, k, w)| (k, w)).collect();
    let page = page_keys(g);
    if g == Group::Global {
        // Esc closes things everywhere, so it belongs with the frequent few.
        let at = out.len().min(GLOBAL_SHOWN - 1);
        for (i, k) in page.into_iter().enumerate() {
            out.insert(at + i, k);
        }
    } else {
        out.extend(page);
    }
    out
}

fn row_view((k, what): (String, &'static str)) -> impl IntoView {
    view! {
        <div class="ks-row">
            <span>{what}</span>
            <kbd>{k}</kbd>
        </div>
    }
}

#[component]
pub fn KeysSheet() -> impl IntoView {
    let open = RwSignal::new(false);
    let navigate = StoredValue::new_local(use_navigate());
    let keys = window_event_listener(ev::keydown, move |e| {
        if crate::shortcuts::action(&e) == Some("help") {
            e.prevent_default();
            open.update(|o| *o = !*o);
        }
    });
    let menu = crate::shortcuts::on_menu(move |id| {
        if id == "act:help" {
            open.set(true);
        }
    });
    on_cleanup(move || {
        keys.remove();
        menu.remove();
    });
    let edit = move |_| {
        open.set(false);
        navigate.with_value(|n| n("/settings?s=shortcuts", Default::default()));
    };
    view! {
        <Show when=move || open.get()>
            <Modal
                label="键盘快捷键"
                class="dlg keys-sheet"
                on_close=Callback::new(move |_| open.set(false))
            >
                <h3>"键盘快捷键"</h3>
                <div class="ks-groups">
                    {Group::ALL
                        .into_iter()
                        .map(|g| {
                            view! {
                                <section class="ks-group" aria-label=g.title()>
                                    <h4>{g.title()}</h4>
                                    {move || {
                                        let mut all = rows(g);
                                        let more = if g == Group::Global && all.len() > GLOBAL_SHOWN
                                        {
                                            all.split_off(GLOBAL_SHOWN)
                                        } else {
                                            Vec::new()
                                        };
                                        let n = more.len();
                                        view! {
                                            {all.into_iter().map(row_view).collect_view()}
                                            {(n > 0)
                                                .then(|| {
                                                    view! {
                                                        <details class="ks-more">
                                                            <summary>{format!("更多 {n} 项")}</summary>
                                                            {more.into_iter().map(row_view).collect_view()}
                                                        </details>
                                                    }
                                                })}
                                        }
                                    }}
                                </section>
                            }
                        })
                        .collect_view()}
                </div>
                <div class="dlg-foot">
                    <button type="button" class="btn" on:click=edit>
                        "更改快捷键…"
                    </button>
                    <span class="grow"></span>
                    <button
                        type="button"
                        class="btn primary"
                        data-modal-initial-focus=""
                        on:click=move |_| open.set(false)
                    >
                        "完成"
                    </button>
                </div>
            </Modal>
        </Show>
    }
}
