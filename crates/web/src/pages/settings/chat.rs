use leptos::prelude::*;
use serde_json::json;

use crate::components::prefs::{PrefGroup, PrefRow};

use super::{set_ui, text, ui_prefs};

#[component]
pub(super) fn ChatSettings() -> impl IntoView {
    let ui = ui_prefs();
    view! {
        <PrefGroup title="对话">
            <PrefRow label="发送键" for_id="settings-send">
                <select
                    id="settings-send"
                    class="settings-input"
                    prop:value=text(&ui, "sendKey")
                    on:change=|e| set_ui("sendKey", json!(event_target_value(&e)))
                >
                    <option value="enter" selected=text(&ui, "sendKey") == "enter">
                        "Enter 发送，Shift+Enter 换行"
                    </option>
                    <option value="mod" selected=text(&ui, "sendKey") == "mod">
                        "⌘ / Ctrl + Enter 发送，Enter 换行"
                    </option>
                </select>
            </PrefRow>
        </PrefGroup>
    }
}
