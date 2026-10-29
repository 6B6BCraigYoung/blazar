use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/interaction.js")]
extern "C" {
    type ModalHandle;

    #[wasm_bindgen(js_name = captureModalFocus)]
    fn capture_modal_focus() -> JsValue;

    #[wasm_bindgen(js_name = attachModal)]
    fn attach_modal(
        root: &web_sys::HtmlElement,
        opener: &JsValue,
        close: &js_sys::Function,
        close_on_backdrop: bool,
    ) -> ModalHandle;

    #[wasm_bindgen(method)]
    fn dispose(this: &ModalHandle);

    #[wasm_bindgen(js_name = prefersReducedMotion)]
    fn prefers_reduced_motion() -> bool;

    #[wasm_bindgen(js_name = exitMs)]
    fn exit_ms_js() -> f64;
}

/// 退出动画的时长（`--motion-exit`，毫秒）；用户开了“减少动态效果”时是 0。
pub fn exit_ms() -> u32 {
    let ms = exit_ms_js();
    if ms.is_finite() && ms > 0.0 {
        ms.round() as u32
    } else {
        0
    }
}

/// 程序触发的滚动用哪种方式：平时平滑，“减少动态效果”时直接跳过去。
pub fn scroll_behavior() -> web_sys::ScrollBehavior {
    if prefers_reduced_motion() {
        web_sys::ScrollBehavior::Auto
    } else {
        web_sys::ScrollBehavior::Smooth
    }
}

/// 选中正开着的弹窗根节点。关掉的弹窗会带着 `data-closing` 留一个只读残影播完退出动画
/// （见 `interaction.js` 的 `lingerClosed`），判断“有没有弹窗开着”要用这个选择器把它排除。
pub const OPEN: &str = "[data-modal-mask]:not([data-closing]) > [data-modal-root]";

struct MountedModal {
    handle: ModalHandle,
    _close: Closure<dyn Fn()>,
}

impl Drop for MountedModal {
    fn drop(&mut self) {
        self.handle.dispose();
    }
}

#[component]
pub fn Modal(
    #[prop(into)] label: String,
    on_close: Callback<()>,
    #[prop(default = "dlg")] class: &'static str,
    #[prop(default = "dlg-mask")] mask_class: &'static str,
    #[prop(default = true)] close_on_backdrop: bool,
    children: Children,
) -> impl IntoView {
    let root = NodeRef::<html::Div>::new();
    let opener = capture_modal_focus();
    let mounted = StoredValue::new_local(None::<MountedModal>);
    root.on_load(move |element| {
        let close = Closure::<dyn Fn()>::new(move || on_close.run(()));
        let handle = attach_modal(
            element.unchecked_ref(),
            &opener,
            close.as_ref().unchecked_ref(),
            close_on_backdrop,
        );
        mounted.set_value(Some(MountedModal {
            handle,
            _close: close,
        }));
    });
    on_cleanup(move || {
        mounted.set_value(None);
    });
    view! {
        <div class=mask_class data-modal-mask="">
            <div
                class=class
                node_ref=root
                role="dialog"
                aria-modal="true"
                aria-label=label
                tabindex="-1"
                data-modal-root=""
            >
                {children()}
            </div>
        </div>
    }
}
