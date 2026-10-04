#[cfg(target_arch = "wasm32")]
mod alerts;
#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod app_state;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod chat_model;
#[cfg(target_arch = "wasm32")]
mod components;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod diff;
#[cfg(target_arch = "wasm32")]
mod files_js;
#[cfg(target_arch = "wasm32")]
mod fmt;
#[cfg(any(target_arch = "wasm32", test))]
mod git_policy;
#[cfg(any(target_arch = "wasm32", test))]
mod git_preferences;
#[cfg(any(target_arch = "wasm32", test))]
mod markdown_mode;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod md;
#[cfg(target_arch = "wasm32")]
mod monaco;
#[cfg(target_arch = "wasm32")]
mod pages;
#[cfg(target_arch = "wasm32")]
mod realtime;
#[cfg(target_arch = "wasm32")]
mod rt_logo;
#[cfg(target_arch = "wasm32")]
mod shortcuts;
#[cfg(target_arch = "wasm32")]
mod storage;
#[cfg(target_arch = "wasm32")]
mod term;

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    pages::apply_ui_preferences();
    leptos::mount::mount_to_body(app::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
