use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::files::Files;
use crate::md;

#[component]
pub fn Tabs(files: Files) -> impl IntoView {
    view! {
        <div class="tabbar" role="group" aria-label="打开的文件">
            <For each=move || files.open_tabs.get() key=|p| p.clone() let:path>
                {
                    let p = path.clone();
                    let p2 = path.clone();
                    let p4 = path.clone();
                    let active = move || files.current.with(|c| c.as_deref() == Some(p.as_str()));
                    let active_button = active.clone();
                    let dirty = move || files.dirty.with(|d| d.contains(&p2));
                    view! {
                        <div
                            class="etab"
                            data-active=move || active().to_string()
                            data-dirty=move || dirty().to_string()
                            title=path.clone()
                            on:click=move |_| {
                                if !files
                                    .current
                                    .with_untracked(|c| c.as_deref() == Some(p4.as_str()))
                                {
                                    files.open(p4.clone(), 0)
                                }
                            }
                        >
                            <button
                                type="button"
                                class="etab-select"
                                aria-pressed=move || active_button().to_string()
                                aria-label=path.clone()
                            >
                                <span class="nm">
                                    {path.rsplit('/').next().unwrap_or(&path).to_owned()}
                                </span>
                            </button>
                            <button
                                type="button"
                                class="x"
                                aria-label=format!("关闭 {path}")
                                title="关闭文件"
                                on:click={
                                    let p = path.clone();
                                    move |e| {
                                        e.stop_propagation();
                                        files.close(p.clone());
                                    }
                                }
                            >
                                "×"
                            </button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}

#[component]
pub fn EdBar(files: Files, preview: RwSignal<bool>) -> impl IntoView {
    view! {
        {move || {
            files
                .current
                .get()
                .map(|p| {
                    let crumbs = p.split('/').map(str::to_owned).collect::<Vec<_>>();
                    let last = crumbs.len().saturating_sub(1);
                    let p_dirty = p.clone();
                    let p_save = p.clone();
                    let is_md = Files::is_md(&p);
                    let ro = files.readonly(&p);
                    view! {
                        <div class="edbar">
                            <span class="crumbs">
                                {crumbs
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, c)| {
                                        view! {
                                            {(i > 0).then_some(view! { <i>"›"</i> })}
                                            <span class:cur=i == last>{c}</span>
                                        }
                                    })
                                    .collect_view()}
                            </span>
                            <span class="grow"></span>
                            {move || {
                                if ro {
                                    view! { <span class="muted">"只读"</span> }.into_any()
                                } else if files.dirty.with(|d| d.contains(&p_dirty)) {
                                    let p = p_save.clone();
                                    view! {
                                        <button
                                            class="linkbtn"
                                            title="保存（⌘S）"
                                            on:click=move |_| files.save(p.clone(), false)
                                        >
                                            "保存改动"
                                        </button>
                                    }
                                        .into_any()
                                } else {
                                    view! { <span class="muted">"已保存"</span> }.into_any()
                                }
                            }}
                            {is_md
                                .then(|| {
                                    view! {
                                        <span class="seg">
                                            <button
                                                type="button"
                                                aria-pressed=move || preview.get().to_string()
                                                data-on=move || preview.get().to_string()
                                                on:click=move |_| {
                                                    preview.set(true);
                                                    crate::markdown_mode::save(
                                                        crate::markdown_mode::MarkdownMode::Preview,
                                                    );
                                                }
                                            >
                                                "预览"
                                            </button>
                                            <button
                                                type="button"
                                                aria-pressed=move || (!preview.get()).to_string()
                                                data-on=move || (!preview.get()).to_string()
                                                on:click=move |_| {
                                                    preview.set(false);
                                                    crate::markdown_mode::save(
                                                        crate::markdown_mode::MarkdownMode::Source,
                                                    );
                                                }
                                            >
                                                "源码"
                                            </button>
                                        </span>
                                    }
                                })}
                        </div>
                    }
                })
        }}
    }
}

#[component]
pub fn MdView(files: Files, preview: RwSignal<bool>) -> impl IntoView {
    let el = NodeRef::<html::Div>::new();
    let show = move || {
        preview.get()
            && files
                .current
                .with(|c| c.as_deref().is_some_and(Files::is_md))
    };
    let html = RwSignal::new(String::new());
    let timer = StoredValue::new_local(None::<gloo_timers::callback::Timeout>);
    Effect::new(move |_| {
        files.rev.track();
        if !show() {
            return;
        }
        let Some(p) = files.current.get_untracked() else {
            return;
        };
        let t = gloo_timers::callback::Timeout::new(150, move || {
            let src = files.value(&p);
            let _ = html.try_set(md::render(&src, &p, &files.ws()));
        });
        timer.set_value(Some(t));
    });
    Effect::new(move |_| {
        html.track();
        let Some(root) = el.get() else { return };
        let Ok(list) = root.query_selector_all("pre code[class^=\"language-\"]:not([data-hl])")
        else {
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
            let _ = code.set_attribute("data-hl", "");
            if text.len() > 20_000 {
                continue;
            }
            leptos::task::spawn_local(async move {
                if let Some(h) = crate::monaco::colorize(&text, &md_lang(&lang)).await
                    && code.is_connected()
                {
                    code.set_inner_html(&h);
                }
            });
        }
    });
    let click = move |e: ev::MouseEvent| {
        let Some(a) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("a[href]").ok().flatten())
        else {
            return;
        };
        let href = a.get_attribute("href").unwrap_or_default();
        if let Some(p) = href.strip_prefix(md::OPEN_PREFIX) {
            e.prevent_default();
            let p = js_sys::decode_uri_component(p)
                .map(String::from)
                .unwrap_or_else(|_| p.to_owned());
            files.open(p, 0);
        } else if href.starts_with("http") || href.starts_with("mailto:") {
            e.prevent_default();
            let _ = window().open_with_url_and_target(&href, "_blank");
        } else if href == "#" {
            e.prevent_default();
        }
    };
    view! {
        <div
            class="mdview"
            role="region"
            aria-label="Markdown 预览"
            tabindex="0"
            node_ref=el
            hidden=move || !show()
            on:click=click
        >
            <article class="md-body" inner_html=move || html.get()></article>
        </div>
    }
}

pub(crate) fn md_lang(l: &str) -> String {
    match l {
        "js" => "javascript",
        "ts" => "typescript",
        "sh" | "bash" | "zsh" => "shell",
        "rs" => "rust",
        "py" => "python",
        "yml" => "yaml",
        "md" => "markdown",
        "toml" => "ini",
        other => other,
    }
    .to_owned()
}
