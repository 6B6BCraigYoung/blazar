use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/images.js")]
extern "C" {
    #[wasm_bindgen(js_name = installImages)]
    fn install_images(toast: &js_sys::Function, download: &js_sys::Function);
    #[wasm_bindgen(js_name = settleImages)]
    pub fn settle_images(root: &web_sys::Element);
}

/// 对话图片的点击、复制、失败占位都在 js/images.js 里委托处理，启动时装一次。
pub fn install() {
    let toast = Closure::<dyn Fn(String)>::new(|text: String| {
        if text.ends_with("failed") {
            crate::components::toast::toast(text);
        } else {
            crate::components::toast::flash(text);
        }
    });
    let download = Closure::<dyn Fn(String, String)>::new(|url: String, name: String| {
        crate::components::downloads::start(url, name);
    });
    install_images(
        toast.as_ref().unchecked_ref(),
        download.as_ref().unchecked_ref(),
    );
    toast.forget();
    download.forget();
}
