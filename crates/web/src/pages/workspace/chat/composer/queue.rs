use leptos::prelude::*;
use serde_json::{Value, json};

use crate::api;
use crate::components::modal::Modal;
use crate::components::status::InlineError;

use super::super::state::{Chat, Queued, edit_request};

#[component]
pub(super) fn QueueBand(chat: Chat) -> impl IntoView {
    move || {
        let view_id = chat.view.get();
        let q = chat.queue.get();
        let mine = q.iter().find(|x| x.thread_id == view_id).cloned();
        let others = q.len() - usize::from(mine.is_some());
        if mine.is_none() && others == 0 {
            return None;
        }
        let running = chat.running.get();
        Some(match mine {
            Some(m) => {
                let (m1, m2, m3) = (m.clone(), m.clone(), m.clone());
                view! {
                    <div class="cb-band queue">
                        <div class="cq-h">
                            <span class="cq-dot" data-held=m.held.is_some().to_string()></span>
                            <b>{if m.held.is_some() { "Paused" } else { "Queued" }}</b>
                            <span class="muted">{m.held.clone().unwrap_or_else(|| "Sends after this turn".into())}</span>
                            <span class="grow"></span>
                            {if running {
                                view! { <button class="linkbtn" title="Add to the current turn now" on:click=move |_| chat.queue_act("steer", m1.clone())>"Send now"</button> }.into_any()
                            } else {
                                view! { <button class="linkbtn" on:click=move |_| chat.queue_act("send", m1.clone())>"Send"</button> }.into_any()
                            }}
                            <button class="linkbtn" on:click=move |_| chat.queue_act("edit", m2.clone())>"Edit"</button>
                            <button class="linkbtn" on:click=move |_| chat.queue_act("drop", m3.clone())>"Remove"</button>
                        </div>
                        <div class="cq-t">{m.text.clone()}{(m.images > 0).then(|| view! { <span class="muted">{format!(" · {} attachments", m.images)}</span> })}</div>
                        {(others > 0).then(|| view! { <div class="cq-o muted">{format!("{others} more queued in other conversations")}</div> })}
                    </div>
                }.into_any()
            }
            None => view! { <div class="cb-band queue"><div class="cq-o muted">{format!("{others} queued in other conversations")}</div></div> }.into_any(),
        })
    }
}

#[component]
pub(super) fn QueueEditor(chat: Chat, queued: Queued) -> impl IntoView {
    let text = RwSignal::new(queued.text.clone());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let queued = StoredValue::new(queued);
    let ws = chat.ws_id();
    let close = move || {
        if !busy.get_untracked() {
            chat.editing_queue.set(None);
        }
    };
    let save = move |_| {
        if busy.get_untracked() {
            return;
        }
        let queued = queued.get_value();
        let Some(original) = queued.request else {
            return;
        };
        let request = edit_request(&original, text.get_untracked());
        if request["text"].as_str().is_none_or(|t| t.trim().is_empty())
            && request["images"].as_array().is_none_or(Vec::is_empty)
        {
            error.set("Enter a message".into());
            return;
        }
        let ws = ws.clone();
        busy.set(true);
        error.set(String::new());
        chat.spawn(async move {
            let result = api::send::<Vec<Queued>>(
                "PUT",
                &format!("/api/workspaces/{ws}/queue"),
                &json!({ "thread_id": queued.thread_id, "request": request, "expected": original }),
            )
            .await;
            let _ = busy.try_set(false);
            match result {
                Ok(queue) => {
                    let _ = chat.queue.try_set(queue);
                    let _ = chat.editing_queue.try_set(None);
                }
                Err(e) => {
                    let _ = error.try_set(e.to_string());
                }
            }
        });
    };
    view! {
        <Modal label="Edit queued message" on_close=Callback::new(move |_| close())>
                <h3>"Edit queued message"</h3>
                <textarea data-modal-initial-focus="" aria-label="Message" prop:value=move || text.get() disabled=move || busy.get() on:input=move |e| text.set(event_target_value(&e))></textarea>
                <p class="muted">"Attachments, file context and send settings are kept."</p>
                <Show when=move || !error.get().is_empty()>{move || view! { <InlineError message=error.get()/> }}</Show>
                <div class="dlg-foot"><button class="btn" disabled=move || busy.get() on:click=move |_| close()>"Cancel"</button><button class="btn primary" disabled=move || busy.get() on:click=save>{move || if busy.get() { "Saving…" } else { "Save" }}</button></div>
        </Modal>
    }
}

#[component]
pub(super) fn FailedEditor(
    chat: Chat,
    id: u32,
    request: Value,
    on_close: Callback<()>,
) -> impl IntoView {
    let text = RwSignal::new(request["text"].as_str().unwrap_or_default().to_owned());
    let images = RwSignal::new(request["images"].as_array().cloned().unwrap_or_default());
    let original = StoredValue::new(request);
    let error = RwSignal::new(String::new());
    let save = move |_| {
        let text = text.get_untracked();
        let images = images.get_untracked();
        if text.trim().is_empty() && images.is_empty() {
            error.set("Enter a message or keep an attachment".into());
            return;
        }
        let mut request = edit_request(&original.get_value(), text);
        request["images"] = json!(images);
        if chat.deliveries.write().edit_failed(id, request) {
            on_close.run(());
        } else {
            error.set("The message was already retried or discarded".into());
        }
    };
    view! {
        <Modal label="Edit unsent message" on_close>
                <h3>"Edit unsent message"</h3>
                <textarea data-modal-initial-focus="" aria-label="Message" prop:value=move || text.get() on:input=move |e| text.set(event_target_value(&e))></textarea>
                {move || images.get().iter().enumerate().map(|(index, _)| view! {
                    <div class="row"><span>{format!("Attachment {}", index + 1)}</span><button class="linkbtn" on:click=move |_| images.update(|images| { if index < images.len() { images.remove(index); } })>"Remove"</button></div>
                }).collect_view()}
                <p class="muted">"File context and send settings are kept. Retry after saving."</p>
                <Show when=move || !error.get().is_empty()>{move || view! { <InlineError message=error.get()/> }}</Show>
                <div class="dlg-foot"><button class="btn" on:click=move |_| on_close.run(())>"Cancel"</button><button class="btn primary" on:click=save>"Save"</button></div>
        </Modal>
    }
}
