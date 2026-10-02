//! Blazar 的网页界面（Leptos，纯客户端渲染）。由 Trunk 编成 wasm，hub 挂在 `/v2/` 下提供。

#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod components;
#[cfg(target_arch = "wasm32")]
mod fmt;
#[cfg(target_arch = "wasm32")]
mod pages;
#[cfg(target_arch = "wasm32")]
mod realtime;

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}

// 原生目标（工作区整体构建）下什么都不做：界面只在浏览器里跑。
#[cfg(not(target_arch = "wasm32"))]
fn main() {}
