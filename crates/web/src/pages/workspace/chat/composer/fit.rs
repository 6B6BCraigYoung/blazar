use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

const LEVELS: u8 = 5;

fn crowded(bar: &web_sys::Element) -> bool {
    if bar.scroll_width() > bar.client_width() {
        return true;
    }
    let Ok(parts) = bar.query_selector_all(".cb-btn, .cb-tx") else {
        return false;
    };
    (0..parts.length())
        .filter_map(|i| parts.item(i)?.dyn_into::<web_sys::Element>().ok())
        .any(|e| e.client_width() > 0 && e.scroll_width() > e.client_width() + 1)
}

fn fit(bar: &web_sys::Element) {
    for level in 0..=LEVELS {
        let _ = bar.set_attribute("data-fit", &level.to_string());
        if !crowded(bar) {
            return;
        }
    }
}

pub fn keep_fitting(bar: NodeRef<html::Div>) {
    let run = move || {
        if let Some(el) = bar.get_untracked() {
            fit(&el);
        }
    };
    let on_change = Closure::<dyn Fn()>::new(move || request_animation_frame(run));
    let resize = web_sys::ResizeObserver::new(on_change.as_ref().unchecked_ref()).ok();
    let mutate = web_sys::MutationObserver::new(on_change.as_ref().unchecked_ref()).ok();
    let held = StoredValue::new_local((resize, mutate, on_change));
    bar.on_load(move |el| {
        held.with_value(|(resize, mutate, _)| {
            if let Some(ro) = resize {
                ro.observe(&el);
            }
            if let Some(mo) = mutate {
                let init = web_sys::MutationObserverInit::new();
                init.set_child_list(true);
                init.set_subtree(true);
                init.set_character_data(true);
                let _ = mo.observe_with_options(&el, &init);
            }
        });
        request_animation_frame(run);
    });
    on_cleanup(move || {
        held.with_value(|(resize, mutate, _)| {
            if let Some(ro) = resize {
                ro.disconnect();
            }
            if let Some(mo) = mutate {
                mo.disconnect();
            }
        });
    });
}
