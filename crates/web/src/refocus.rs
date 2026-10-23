use leptos::ev;
use leptos::prelude::*;

pub fn on_refocus(run: impl Fn() + Clone + 'static) {
    let last = StoredValue::new_local(js_sys::Date::now());
    let fire = move || {
        let now = js_sys::Date::now();
        let Some(previous) = last.try_get_value() else {
            return;
        };
        if now - previous < 1500.0 {
            return;
        }
        let _ = last.try_set_value(now);
        run();
    };
    let on_visible = fire.clone();
    let focus = window_event_listener(ev::focus, move |_| fire());
    let visibility = window_event_listener(ev::visibilitychange, move |_| {
        if !document().hidden() {
            on_visible();
        }
    });
    on_cleanup(move || {
        focus.remove();
        visibility.remove();
    });
}
