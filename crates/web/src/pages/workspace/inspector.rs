use std::collections::HashSet;

use leptos::html;
use leptos::prelude::*;

use super::diff_panel::{DiffBar, DiffState, DiffView};
use super::editor::{EdBar, MdView, Tabs};
use super::files::Files;
use super::git_panel::{Git, GitView};
use super::layout::{Changes, LayoutState, Tab};
use super::preview_panel::PreviewBar;
use crate::api;
use crate::components::empty::{Action, Blank};
use crate::components::icon;

const ICON_MAX: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 2.5h4v4M6.5 13.5h-4v-4M13.5 2.5 9 7M2.5 13.5 7 9"/></svg>"#;
const ICON_RESTORE: &str = r#"<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M13 7H9V3M3 9h4v4M9 7l4.5-4.5M7 9l-4.5 4.5"/></svg>"#;

#[component]
pub fn Inspector(
    ws: String,
    files: Files,
    git: Git,
    diff: DiffState,
    state: LayoutState,
    host: NodeRef<html::Div>,
    changes: Memo<std::collections::HashMap<String, api::ChangeKind>>,
    md_preview: RwSignal<bool>,
    draft: RwSignal<Option<String>>,
    isolated: Signal<bool>,
) -> impl IntoView {
    let lay = move || state.lay.get();
    let on = move |t: Tab| move || (lay().tab == t).to_string();
    let collapsed = RwSignal::new(HashSet::<String>::new());
    let diff_n = move || diff.files.with(Vec::len);
    let git_mark = move || {
        git.status.with(|g| match g {
            Some(g) if g.op.is_some() => Some(("bad", "!".to_owned())),
            Some(g) if g.uncommitted > 0 => Some(("", g.uncommitted.to_string())),
            _ => None,
        })
    };
    let preview_on = Signal::derive(move || {
        let l = lay();
        l.inspect && l.tab == Tab::Preview
    });
    let tab = move |t: Tab, label: &'static str| {
        view! {
            <button
                type="button"
                class="rtab"
                role="tab"
                aria-selected=on(t)
                data-active=on(t)
                on:click=move |_| state.open(t)
            >
                {label}
                {move || {
                    (t == Tab::Changes && diff_n() > 0)
                        .then(|| view! { <span class="tabn">{diff_n()}</span> })
                }}
            </button>
        }
    };
    let sub = move |c: Changes| move || (lay().changes == c).to_string();
    let open_files = Action::new("显示文件", move || {
        if !state.lay.get_untracked().files {
            state.toggle_files();
        }
    })
    .keys(&crate::shortcuts::current("explorer"));
    view! {
        <div class="rhead insp-head">
            <div class="rtabs" role="tablist" aria-label="检查器">
                {tab(Tab::Editor, "编辑器")}
                {tab(Tab::Changes, "更改")}
                {tab(Tab::Preview, "预览")}
            </div>
            <Show when=move || lay().tab == Tab::Changes>
                <span class="seg insp-sub" role="group" aria-label="更改的视图">
                    <button
                        type="button"
                        data-on=sub(Changes::Diff)
                        aria-pressed=sub(Changes::Diff)
                        on:click=move |_| state.show_changes(Changes::Diff)
                    >
                        "差异"
                    </button>
                    <button
                        type="button"
                        data-on=sub(Changes::Git)
                        aria-pressed=sub(Changes::Git)
                        on:click=move |_| state.show_changes(Changes::Git)
                    >
                        "Git"
                        {move || {
                            git_mark()
                                .map(|(c, t)| view! { <span class=format!("tabn {c}")>{t}</span> })
                        }}
                    </button>
                </span>
            </Show>
            <span class="grow"></span>
            <button
                type="button"
                class="laybtn insp-max"
                aria-label=move || if lay().max { "还原检查器" } else { "放大检查器" }
                title=move || if lay().max { "还原检查器" } else { "放大检查器" }
                aria-pressed=move || lay().max.to_string()
                inner_html=move || if lay().max { ICON_RESTORE } else { ICON_MAX }
                on:click=move |_| state.set_max(!lay().max)
            ></button>
            <button
                type="button"
                class="laybtn"
                aria-label="关闭检查器"
                title="关闭 Esc"
                on:click=move |_| state.close()
            >
                "×"
            </button>
        </div>
        <div class="insp-body">
            <div class="ipane col" data-active=on(Tab::Editor)>
                <Tabs files changes />
                <EdBar files preview=md_preview />
                <div class="editor-stack">
                    <div class="editor-host" node_ref=host aria-label="代码编辑器"></div>
                    <MdView files preview=md_preview />
                    <Show when=move || files.open_tabs.with(Vec::is_empty)>
                        <div class="welcome">
                            <Blank
                                icon=icon::FILE
                                title="没有打开的文件"
                                detail="从文件树打开，或点对话里的文件链接。"
                                action=open_files.clone()
                                inset=true
                            />
                        </div>
                    </Show>
                </div>
            </div>
            <div class="ipane col" data-active=on(Tab::Changes)>
                <div class="ipane col" data-active=sub(Changes::Diff)>
                    <DiffBar d=diff git collapsed draft />
                    <DiffView ws=ws.clone() d=diff files collapsed />
                </div>
                <div class="ipane" data-active=sub(Changes::Git)>
                    <GitView git files draft />
                </div>
            </div>
            <div class="ipane col" data-active=on(Tab::Preview)>
                <PreviewBar ws=ws.clone() active=preview_on isolated />
            </div>
        </div>
    }
}
