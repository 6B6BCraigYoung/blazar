use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::files::Files;
use crate::api;
use crate::components::toast::toast_error;
use crate::media_kind::{self, Media};

const ZOOM_MIN: f64 = 0.02;
const ZOOM_MAX: f64 = 40.0;
const FIT_MARGIN: f64 = 24.0;

fn raw_url(ws: &str, path: &str) -> String {
    format!(
        "/api/workspaces/{}/raw?path={}",
        api::enc(ws),
        api::enc(path)
    )
}

fn name_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

fn absolute(root: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{}/{path}", root.trim_end_matches('/'))
    }
}

fn open_in_app(path: String) {
    spawn_local(async move {
        if let Err(e) =
            crate::desktop::invoke("open_path", &serde_json::json!({ "path": path })).await
        {
            toast_error("没能用默认程序打开", &e);
        }
    });
}

/// Pictures, video, sound and PDFs in the editor slot, over the code editor.
#[component]
pub fn MediaView(files: Files, local: Signal<bool>, root: Signal<String>) -> impl IntoView {
    let current = Memo::new(move |_| files.current.get().filter(|p| Files::is_media(p)));
    view! {
        <div class="media-view" hidden=move || current.with(Option::is_none)>
            {move || current.get().map(|path| view! { <Viewer files local root path /> })}
        </div>
    }
}

#[component]
fn Viewer(files: Files, local: Signal<bool>, root: Signal<String>, path: String) -> impl IntoView {
    let ws = files.ws();
    let src = raw_url(&ws, &path);
    let kind = media_kind::kind_of(&path).unwrap_or(Media::Image);
    let mime = media_kind::mime_of(&path).unwrap_or_default();
    let size = RwSignal::new(None::<u64>);
    let dims = RwSignal::new(None::<(u32, u32)>);
    let failed = RwSignal::new(false);
    let p = path.clone();
    spawn_local(async move {
        if let Ok(r) = api::read_file(&ws, &p).await {
            let _ = size.try_set(Some(r.size));
        }
    });
    let label = media_kind::label_of(&path);
    let info = move || {
        let mut parts = vec![label.clone()];
        if let Some((w, h)) = dims.get() {
            parts.push(format!("{w} × {h}"));
        }
        if let Some(s) = size.get() {
            parts.push(media_kind::size_label(s));
        }
        parts.join(" · ")
    };
    let actions = {
        let src = src.clone();
        let path = path.clone();
        move || {
            let (src, name) = (src.clone(), name_of(&path));
            let abs = absolute(&root.get(), &path);
            view! {
                <button
                    type="button"
                    class="linkbtn"
                    on:click=move |_| crate::components::downloads::start(src.clone(), name.clone())
                >
                    "下载"
                </button>
                <Show when=move || {
                    local.get()
                }>
                    {
                        let abs = abs.clone();
                        view! {
                            <button
                                type="button"
                                class="linkbtn"
                                on:click=move |_| open_in_app(abs.clone())
                            >
                                "用默认程序打开"
                            </button>
                        }
                    }
                </Show>
            }
        }
    };
    let body = match kind {
        Media::Image => view! { <ImageView src=src.clone() dims /> }.into_any(),
        Media::Video => view! { <Player src=src.clone() mime video=true dims failed /> }.into_any(),
        Media::Audio => {
            view! { <Player src=src.clone() mime video=false dims failed /> }.into_any()
        }
        Media::Pdf => {
            view! { <iframe class="media-pdf" src=src.clone() title=name_of(&path)></iframe> }
                .into_any()
        }
    };
    let fallback_actions = actions.clone();
    let fallback_label = media_kind::label_of(&path);
    let name = name_of(&path);
    view! {
        <Show
            when=move || !failed.get()
            fallback=move || {
                view! {
                    <div class="media-none" role="status">
                        <span class="media-none-ic" aria-hidden="true">
                            "▶"
                        </span>
                        <b>{name.clone()}</b>
                        <span class="muted">
                            {
                                let l = fallback_label.clone();
                                move || match size.get() {
                                    Some(s) => format!("{l} · {}", media_kind::size_label(s)),
                                    None => l.clone(),
                                }
                            }
                        </span>
                        <p>
                            "这种格式在 Blazar 里放不了，可以下载下来，或者用别的程序打开。"
                        </p>
                        <div class="media-none-acts">{fallback_actions.clone()}</div>
                    </div>
                }
            }
        >
            <div class="media-bar">
                <span class="media-info">{info.clone()}</span>
                <span class="grow"></span>
                {actions.clone()}
            </div>
        </Show>
        <div
            class="media-stage"
            hidden=move || failed.get()
            data-kind=format!("{kind:?}").to_lowercase()
        >
            {body}
        </div>
    }
}

#[component]
fn Player(
    src: String,
    mime: &'static str,
    video: bool,
    dims: RwSignal<Option<(u32, u32)>>,
    failed: RwSignal<bool>,
) -> impl IntoView {
    let el = NodeRef::<html::Video>::new();
    let probe = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.create_element(if video { "video" } else { "audio" }).ok())
        .and_then(|e| e.dyn_into::<web_sys::HtmlMediaElement>().ok());
    if probe.is_some_and(|m| m.can_play_type(mime).is_empty()) {
        failed.set(true);
    }
    let fail = move |_| failed.set(true);
    if video {
        view! {
            <video
                node_ref=el
                class="media-video"
                src=src
                controls
                preload="metadata"
                on:error=fail
                on:loadedmetadata=move |_| {
                    if let Some(v) = el.get_untracked() {
                        if v.video_width() == 0 {
                            failed.set(true);
                        } else {
                            dims.set(Some((v.video_width(), v.video_height())));
                        }
                    }
                }
            ></video>
        }
        .into_any()
    } else {
        view! { <audio class="media-audio" src=src controls preload="metadata" on:error=fail></audio> }
            .into_any()
    }
}

#[component]
fn ImageView(src: String, dims: RwSignal<Option<(u32, u32)>>) -> impl IntoView {
    let stage = NodeRef::<html::Div>::new();
    let img = NodeRef::<html::Img>::new();
    let natural = RwSignal::new(None::<(f64, f64)>);
    let fit = RwSignal::new(true);
    let scale = RwSignal::new(1.0f64);
    let pos = RwSignal::new((0.0f64, 0.0f64));
    let drag = StoredValue::new(None::<(i32, i32, (f64, f64))>);
    let failed = RwSignal::new(false);
    let size_of = move || {
        stage
            .get_untracked()
            .map(|s| (f64::from(s.client_width()), f64::from(s.client_height())))
    };
    let place = move |s: f64| {
        let (Some((sw, sh)), Some((w, h))) = (size_of(), natural.get_untracked()) else {
            return;
        };
        scale.set(s);
        pos.set(((sw - w * s) / 2.0, (sh - h * s) / 2.0));
    };
    let fit_now = move || {
        let (Some((sw, sh)), Some((w, h))) = (size_of(), natural.get_untracked()) else {
            return;
        };
        fit.set(true);
        let room = |v: f64| (v - FIT_MARGIN * 2.0).max(1.0);
        place((room(sw) / w).min(room(sh) / h).min(1.0));
    };
    let actual = move || {
        fit.set(false);
        place(1.0);
    };
    let observer = StoredValue::new_local(None::<(web_sys::ResizeObserver, Closure<dyn FnMut()>)>);
    Effect::new(move |done: Option<bool>| {
        if done == Some(true) {
            return true;
        }
        let Some(s) = stage.get() else { return false };
        let on_resize = Closure::<dyn FnMut()>::new(move || {
            if fit.try_get_untracked() == Some(true) {
                fit_now();
            }
        });
        if let Ok(o) = web_sys::ResizeObserver::new(on_resize.as_ref().unchecked_ref()) {
            o.observe(&s);
            observer.set_value(Some((o, on_resize)));
        }
        true
    });
    on_cleanup(move || {
        observer.try_update_value(|o| {
            if let Some((o, _)) = o.take() {
                o.disconnect();
            }
        });
    });
    let loaded = move |_| {
        let Some(i) = img.get_untracked() else { return };
        let (w, h) = (i.natural_width(), i.natural_height());
        if w == 0 || h == 0 {
            failed.set(true);
            return;
        }
        dims.set(Some((w, h)));
        natural.set(Some((f64::from(w), f64::from(h))));
        fit_now();
    };
    let wheel = move |e: web_sys::WheelEvent| {
        e.prevent_default();
        let Some(s) = stage.get_untracked() else {
            return;
        };
        let r = s.get_bounding_client_rect();
        let (cx, cy) = (
            f64::from(e.client_x()) - r.left(),
            f64::from(e.client_y()) - r.top(),
        );
        let old = scale.get_untracked();
        let new = (old * (-e.delta_y() * 0.002).exp()).clamp(ZOOM_MIN, ZOOM_MAX);
        let k = new / old;
        let (x, y) = pos.get_untracked();
        fit.set(false);
        scale.set(new);
        pos.set((cx - (cx - x) * k, cy - (cy - y) * k));
    };
    let down = move |e: web_sys::PointerEvent| {
        if e.button() != 0 {
            return;
        }
        if let Some(s) = stage.get_untracked() {
            let _ = s.set_pointer_capture(e.pointer_id());
        }
        drag.set_value(Some((e.client_x(), e.client_y(), pos.get_untracked())));
    };
    let moved = move |e: web_sys::PointerEvent| {
        let Some((x0, y0, (px, py))) = drag.get_value() else {
            return;
        };
        fit.set(false);
        pos.set((
            px + f64::from(e.client_x() - x0),
            py + f64::from(e.client_y() - y0),
        ));
    };
    let up = move |_| drag.set_value(None);
    let style = move || {
        let (x, y) = pos.get();
        let s = scale.get();
        match natural.get() {
            Some((w, h)) => format!(
                "width: {w}px; height: {h}px; transform: translate({x}px, {y}px) scale({s})"
            ),
            None => "visibility: hidden".to_owned(),
        }
    };
    view! {
        <div class="media-tools" role="group" aria-label="缩放">
            <span class="seg">
                <button
                    type="button"
                    aria-pressed=move || fit.get().to_string()
                    data-on=move || fit.get().to_string()
                    on:click=move |_| fit_now()
                >
                    "适应窗口"
                </button>
                <button
                    type="button"
                    aria-pressed=move || (!fit.get() && scale.get() == 1.0).to_string()
                    data-on=move || (!fit.get() && scale.get() == 1.0).to_string()
                    on:click=move |_| actual()
                >
                    "1:1"
                </button>
            </span>
            <span class="media-zoom">{move || format!("{:.0}%", scale.get() * 100.0)}</span>
        </div>
        <div
            class="media-canvas"
            node_ref=stage
            on:wheel=wheel
            on:pointerdown=down
            on:pointermove=moved
            on:pointerup=up
            on:pointercancel=up
            on:dblclick=move |_| if fit.get_untracked() { actual() } else { fit_now() }
        >
            <Show
                when=move || !failed.get()
                fallback=|| view! { <p class="media-broken">"这张图打不开。"</p> }
            >
                <img
                    node_ref=img
                    class="media-img"
                    src=src.clone()
                    alt=""
                    draggable="false"
                    data-pixel=move || (scale.get() >= 3.0).to_string()
                    style=style
                    on:load=loaded
                    on:error=move |_| failed.set(true)
                />
            </Show>
        </div>
    }
}
