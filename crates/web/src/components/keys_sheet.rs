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

fn rows(g: Group) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = crate::shortcuts::effective()
        .into_iter()
        .filter_map(|(id, combo)| {
            let b = keymap::binding(id)?;
            (b.group == g && !combo.is_empty()).then(|| (label(&combo), b.label))
        })
        .collect();
    out.extend(page_keys(g));
    out
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
                                    {rows(g)
                                        .into_iter()
                                        .map(|(k, what)| {
                                            view! {
                                                <div class="ks-row">
                                                    <span>{what}</span>
                                                    <kbd>{k}</kbd>
                                                </div>
                                            }
                                        })
                                        .collect_view()}
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
