use super::*;
use crate::components::prefs::{PrefRow, Sheet};

#[component]
pub(super) fn AdvancedSheet(
    m: Mesh,
    source: Signal<Option<String>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let via = RwSignal::new(String::new());
    let ct = RwSignal::new(String::new());
    spawn_local(async move {
        if let Ok(v) = api::get::<Value>("/api/mesh/issuer").await {
            let cfg = &v["config"];
            via.try_set(s(cfg, "via"));
            ct.try_set(s(cfg, "container"));
        }
    });
    let read = move || {
        if busy.get_untracked() {
            return;
        }
        let v = via.get_untracked().trim().to_owned();
        if v.is_empty() {
            toast("填一个 SSH Host");
            return;
        }
        let c = Some(ct.get_untracked().trim().to_owned()).filter(|x| !x.is_empty());
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>(
                "PUT",
                "/api/mesh/issuer",
                &json!({ "via": v, "container": c }),
            )
            .await
            {
                Ok(_) => {
                    toast("已设置，正在发现机器…");
                    gloo_timers::future::TimeoutFuture::new(4000).await;
                    m.local.update(|n| *n += 1);
                    m.issuer.update(|n| *n += 1);
                    m.app.load_state();
                }
                Err(e) => toast(format!("设置失败：{e}")),
            }
            busy.try_set(false);
        });
    };
    view! {
        <Sheet
            title="机器发现"
            on_close
            foot=move || {
                view! {
                    <button
                        type="button"
                        class="btn primary"
                        data-modal-initial-focus=""
                        on:click=move |_| on_close.run(())
                    >
                        "完成"
                    </button>
                }
            }
        >
            <p>"组网机器会自动出现；也可以从本机 SSH 配置中选择机器。SSH 机器无需加入组网。"</p>
            <PrefRow label="发现来源">
                <span class="pref-text">
                    {move || source.get().unwrap_or_else(|| "暂时无法发现组网机器".into())}
                </span>
            </PrefRow>
            <PrefRow label="发现组网机器" detail="重新读取组网里的机器。">
                <button
                    type="button"
                    class="btn"
                    disabled=move || m.refreshing.get()
                    on:click=move |_| spawn_local(refresh_mesh(m))
                >
                    {move || if m.refreshing.get() { "发现中…" } else { "立即发现" }}
                </button>
            </PrefRow>
            <PrefRow
                label="SSH Host"
                detail="运行 EasyTier 的机器，Blazar 通过 SSH 读取它看到的机器。"
                for_id="mesh-via"
            >
                <input
                    id="mesh-via"
                    class="settings-input mono"
                    autocomplete="off"
                    spellcheck="false"
                    placeholder="如 hub-host"
                    disabled=move || busy.get()
                    prop:value=move || via.get()
                    on:input=move |e| via.set(event_target_value(&e))
                />
            </PrefRow>
            <PrefRow label="容器名" detail="EasyTier 跑在容器里时填写。" for_id="mesh-container">
                <input
                    id="mesh-container"
                    class="settings-input mono"
                    autocomplete="off"
                    spellcheck="false"
                    placeholder="可选"
                    disabled=move || busy.get()
                    prop:value=move || ct.get()
                    on:input=move |e| ct.set(event_target_value(&e))
                />
            </PrefRow>
            <PrefRow label="通过 SSH 读取">
                <button
                    type="button"
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| read()
                >
                    {move || if busy.get() { "读取中…" } else { "读取机器" }}
                </button>
            </PrefRow>
        </Sheet>
    }
}
