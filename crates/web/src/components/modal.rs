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
}

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
            <div class=class node_ref=root role="dialog" aria-modal="true" aria-label=label tabindex="-1" data-modal-root="">
                {children()}
            </div>
        </div>
    }
}
