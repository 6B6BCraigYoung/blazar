#[cfg(any(target_arch = "wasm32", test))]
mod action_gate;
#[cfg(target_arch = "wasm32")]
mod alerts;
#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod app_state;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod bot_words;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]
mod chat_model;
#[cfg(target_arch = "wasm32")]
mod client_log;
#[cfg(target_arch = "wasm32")]
mod components;
#[cfg(target_arch = "wasm32")]
mod desktop;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod diff;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod download_model;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod drafts;
#[cfg(target_arch = "wasm32")]
mod failure;
#[cfg(target_arch = "wasm32")]
mod files_js;
#[cfg(target_arch = "wasm32")]
mod fmt;
#[cfg(any(target_arch = "wasm32", test))]
mod git_policy;
#[cfg(any(target_arch = "wasm32", test))]
mod git_preferences;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod home_rows;
#[cfg(target_arch = "wasm32")]
mod images_js;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod link_state;
#[cfg(any(target_arch = "wasm32", test))]
mod loopback;
#[cfg(any(target_arch = "wasm32", test))]
mod markdown_mode;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod md;
#[cfg(target_arch = "wasm32")]
mod monaco;
#[cfg(target_arch = "wasm32")]
mod pages;
#[cfg(any(target_arch = "wasm32", test))]
mod palette_match;
#[cfg(target_arch = "wasm32")]
mod realtime;
#[cfg(target_arch = "wasm32")]
mod refocus;
#[cfg(target_arch = "wasm32")]
mod rt_logo;
#[cfg(any(target_arch = "wasm32", test))]
mod runtime_config;
#[cfg(target_arch = "wasm32")]
mod shortcuts;
#[cfg(target_arch = "wasm32")]
mod storage;
#[cfg(target_arch = "wasm32")]
mod stream;
#[cfg(target_arch = "wasm32")]
mod term;
#[cfg(target_arch = "wasm32")]
mod ui_driver;
#[cfg(any(target_arch = "wasm32", test))]
mod undo_ledger;
#[cfg(target_arch = "wasm32")]
mod version_check;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod vocab;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod vocab_en;

#[cfg(target_arch = "wasm32")]
fn main() {
    client_log::install();
    std::panic::set_hook(Box::new(|info| {
        client_log::panicked(&info.to_string());
        console_error_panic_hook::hook(info);
    }));
    pages::apply_ui_preferences();
    leptos::mount::mount_to_body(app::App);
    if let Some(performance) = web_sys::window().and_then(|w| w.performance()) {
        let _ = performance.mark("blazar:mounted");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}
