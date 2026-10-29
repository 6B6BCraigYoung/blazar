use super::*;
use crate::components::prefs::PrefRow;

/// 「高级」里的机器发现：团队里的机器从哪台读出来，也可以手动重读。
#[component]
pub(super) fn Discovery(m: Mesh) -> impl IntoView {
    let bus = crate::realtime::use_bus();
    let topo = LocalResource::new(move || {
        m.local.track();
        bus.nodes.track();
        api::get::<Value>("/api/mesh/topology")
    });
    let source = move || match topo.get() {
        None => "正在读取…".to_owned(),
        Some(Err(e)) => crate::vocab::lead("暂时读不到团队里的机器", e.explained()).0,
        Some(Ok(t)) => Some(s(&t, "source"))
            .filter(|x| !x.is_empty())
            .unwrap_or_else(|| "暂时读不到团队里的机器".into()),
    };
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
            toast("填一台机器的 SSH 名称");
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
                Err(e) => toast_error("设置失败", &e),
            }
            busy.try_set(false);
        });
    };
    view! {
        <p class="muted small">
            "团队里的机器会自动出现；也可以从本机 SSH 配置里添加机器，那样不用加入团队。"
        </p>
        <PrefRow label="发现来源">
            <span class="pref-text">{source}</span>
        </PrefRow>
        <PrefRow label="重新发现" detail="重新读取团队里的机器。">
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
            label="从哪台机器读取"
            detail="运行 EasyTier 的那台机器（填 SSH 名称），Blazar 经 SSH 读取它看到的机器。"
            for_id="mesh-via"
        >
            <input
                id="mesh-via"
                class="settings-input mono"
                autocomplete="off"
                spellcheck="false"
                placeholder="如 office-server"
                disabled=move || busy.get()
                prop:value=move || via.get()
                on:input=move |e| via.set(event_target_value(&e))
            />
        </PrefRow>
        <PrefRow
            label="容器名"
            detail="EasyTier 跑在容器里时填写。"
            for_id="mesh-container"
        >
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
            <button type="button" class="btn" disabled=move || busy.get() on:click=move |_| read()>
                {move || if busy.get() { "读取中…" } else { "读取机器" }}
            </button>
        </PrefRow>
    }
}
