use super::*;
use crate::components::page_head::{MenuItem, MoreMenu};
use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow};

#[component]
pub(super) fn MeshLocal(
    m: Mesh,
    invite_in: NodeRef<leptos::html::Input>,
    toml_in: NodeRef<leptos::html::Input>,
) -> impl IntoView {
    let local = LocalResource::new(move || {
        m.local.track();
        api::get::<Value>("/api/mesh/local")
    });
    let ext_rpc = RwSignal::new(String::new());
    let rpc_busy = RwSignal::new(false);
    let busy = Signal::derive(move || m.operation.get());
    let use_invite = move || {
        if let Some(i) = invite_in.get_untracked() {
            i.click();
        }
    };
    let import_toml = move || {
        if let Some(i) = toml_in.get_untracked() {
            i.click();
        }
    };
    let refresh = move || spawn_local(refresh_mesh(m));
    let new_config = move || m.config.set(Some((BLANK_TOML.to_owned(), None)));
    let edit_config = move || {
        spawn_local(async move {
            match api::get::<Value>("/api/mesh/config").await {
                Ok(c) => m.config.set(Some((s(&c, "toml"), None))),
                Err(e) => toast(format!("读取配置失败：{e}")),
            }
        });
    };
    let leave = move || {
        spawn_local(async move {
            if m.operation.get_untracked() {
                return;
            }
            if dialog::ask(
                "退出组网",
                "退出组网后，本机上的组网凭据会被删除，其它机器将无法再连到本机。确定？",
                vec![Choice::plain("取消"), Choice::danger("退出")],
            )
            .await
                != Some(1)
            {
                return;
            }
            if m.operation.get_untracked() {
                return;
            }
            m.operation.set(true);
            toast("等待系统授权…");
            match api::send::<Value>("POST", "/api/mesh/leave", &json!({})).await {
                Ok(_) => {
                    toast("已退出组网");
                    m.local.update(|n| *n += 1);
                    m.invites.update(|n| *n += 1);
                    m.app.load_state();
                }
                Err(e) => toast(format!("退出失败：{e}")),
            }
            m.operation.set(false);
        });
    };
    let connect_rpc = move || {
        if rpc_busy.get_untracked() {
            return;
        }
        rpc_busy.set(true);
        let r = ext_rpc.get_untracked();
        spawn_local(async move {
            match api::send::<Value>("PUT", "/api/mesh/external-rpc", &json!({ "rpc": r.trim() }))
                .await
            {
                Ok(v) => {
                    toast(if v["status"]["external"]["rpc"].is_string() {
                        "已连上"
                    } else {
                        "还连不上 —— 确认 GUI 里设置的地址一致"
                    });
                    m.local.update(|n| *n += 1);
                }
                Err(e) => toast(e.to_string()),
            }
            rpc_busy.try_set(false);
        });
    };
    view! {
        <PrefGroup title="本机组网">
            {move || match local.get() {
                None => {
                    view! { <LoadingState text="正在读取组网状态…" class="pref-empty" /> }
                        .into_any()
                }
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            class="pref-empty"
                            message=format!("无法读取组网状态：{e}")
                            retry=Callback::new(move |()| m.local.update(|n| *n += 1))
                        />
                    }
                        .into_any()
                }
                Some(Ok(v)) => {
                    let st = v["status"].clone();
                    if st["joined"].as_bool() == Some(true) {
                        let n = st["node"].clone();
                        let running = st["running"].as_bool() == Some(true);
                        let other = st["other_engine"].as_str().map(str::to_owned);
                        let net = dash(s(&n, "network_name"));
                        let addr = dash(s(&n, "virtual_ipv4"));
                        let addr_row = addr.clone();
                        let host = dash(s(&n, "hostname"));
                        let nat = dash(s(&n, "nat_type"));
                        let peers = st["peer_count"].as_u64().unwrap_or(0).to_string();
                        let engine = format!(
                            "EasyTier {}",
                            Some(s(&n, "version"))
                                .filter(|x| !x.is_empty())
                                .unwrap_or_else(|| s(&st, "engine_version")),
                        );
                        let detail = if running {
                            format!("网络 {net}")
                        } else {
                            "组网服务已停止。请重新打开邀请文件加入；排查日志位于系统组网目录的 logs/。"
                                .to_owned()
                        };
                        let menu = vec![
                            MenuItem::new("刷新机器", refresh),
                            MenuItem::new("编辑 EasyTier 配置", edit_config),
                            MenuItem::new("使用邀请文件", use_invite),
                            MenuItem::new("导入 EasyTier 配置", import_toml),
                            MenuItem::danger("退出组网", leave),
                        ];
                        return view! {
                            <PrefRow label="状态" detail=detail>
                                {if running {
                                    view! { <Status tone=Tone::Other label="已连接" /> }.into_any()
                                } else {
                                    view! { <Status tone=Tone::Failed label="未连接" /> }.into_any()
                                }}
                                <span class="pref-text mono">{addr_row}</span>
                                <MoreMenu label="更多…".to_owned() text="更多…" items=menu disabled=busy />
                            </PrefRow>
                            {other
                                .map(|o| {
                                    view! {
                                        <PrefRow
                                            label="另一个 EasyTier"
                                            detail=format!(
                                                "本机还在运行另一个 EasyTier（{o}）。两个实例连同一个网络会互相抢路由，建议退出它。",
                                            )
                                        />
                                    }
                                })}
                            <PrefAdvanced title="组网详情">
                                {text_row("可见机器", peers.clone(), false)}
                                {text_row("网络", net.clone(), true)}
                                {text_row("主机名", host.clone(), true)}
                                {text_row("虚拟地址", addr.clone(), true)}
                                {text_row("NAT", nat.clone(), false)}
                                {text_row("引擎", engine.clone(), true)}
                            </PrefAdvanced>
                        }
                            .into_any();
                    }
                    if st["external"].is_object() {
                        let x = st["external"].clone();
                        let xn = x["node"].clone();
                        let label = s(&x, "label");
                        let rpc = s(&x, "rpc");
                        let addr = Some(s(&x, "virtual_ipv4"))
                            .filter(|a| !a.is_empty())
                            .unwrap_or_else(|| "未能识别".into());
                        let net = dash(s(&xn, "network_name"));
                        let host = dash(s(&xn, "hostname"));
                        let nat = dash(s(&xn, "nat_type"));
                        let peers = if xn.is_object() {
                            x["peer_count"].as_u64().unwrap_or(0).to_string()
                        } else {
                            "—".into()
                        };
                        let menu = vec![
                            MenuItem::new("刷新机器", refresh),
                            MenuItem::new("使用邀请文件", use_invite),
                            MenuItem::new("导入 EasyTier 配置", import_toml),
                        ];
                        return view! {
                            <PrefRow
                                label="状态"
                                detail=format!("连接由 {label} 提供，Blazar 复用这个连接，无需重新加入。")
                            >
                                <Status tone=Tone::Other label="已连接" />
                                <span class="pref-text mono">{addr}</span>
                                <MoreMenu label="更多…".to_owned() text="更多…" items=menu disabled=busy />
                            </PrefRow>
                            <PrefAdvanced title="组网详情">
                                {text_row("网络", net.clone(), true)}
                                {text_row("主机名", host.clone(), true)}
                                {text_row("可见节点", peers.clone(), false)}
                                {text_row("NAT", nat.clone(), false)}
                                {text_row("RPC", dash(rpc.clone()), true)}
                                {rpc
                                    .is_empty()
                                    .then(|| {
                                        view! {
                                            <PrefRow
                                                label="RPC 门户"
                                                detail=format!(
                                                    "在 {label} 设置里把「RPC 门户」设为 127.0.0.1:15888，可看到本机 NAT 与可见节点数。",
                                                )
                                                for_id="mesh-rpc"
                                            >
                                                <input
                                                    id="mesh-rpc"
                                                    class="settings-input mono"
                                                    placeholder="127.0.0.1:15888"
                                                    disabled=move || rpc_busy.get()
                                                    prop:value=move || ext_rpc.get()
                                                    on:input=move |e| ext_rpc.set(event_target_value(&e))
                                                />
                                                <button
                                                    type="button"
                                                    class="btn"
                                                    disabled=move || rpc_busy.get()
                                                    on:click=move |_| connect_rpc()
                                                >
                                                    "连接"
                                                </button>
                                            </PrefRow>
                                        }
                                    })}
                            </PrefAdvanced>
                        }
                            .into_any();
                    }
                    let bundled = st["engine_bundled"].as_bool() == Some(true);
                    let version = s(&st, "engine_version");
                    let menu = vec![
                        MenuItem::new("使用邀请文件", use_invite),
                        MenuItem::new("导入 EasyTier 配置", import_toml),
                        MenuItem::new("新建 EasyTier 配置", new_config),
                        MenuItem::new("刷新机器", refresh),
                    ];
                    view! {
                        <PrefRow
                            label="状态"
                            detail="用邀请文件或 EasyTier 配置加入，也可把 .blazar 或 .toml 文件拖到此处。"
                        >
                            <Status tone=Tone::Other label="未加入" />
                            <MoreMenu label="更多…".to_owned() text="更多…" items=menu disabled=busy />
                        </PrefRow>
                        {(!bundled)
                            .then(|| {
                                view! {
                                    <PrefRow
                                        label="组网引擎"
                                        detail="这个安装包没有带组网引擎，请安装完整版桌面端。"
                                    >
                                        <span class="pref-text bad">"缺失"</span>
                                    </PrefRow>
                                }
                            })}
                        <PrefAdvanced title="组网详情">
                            {text_row(
                                "引擎",
                                format!(
                                    "EasyTier {version}{}",
                                    if bundled { "（随安装包提供）" } else { "（未提供）" },
                                ),
                                true,
                            )}
                            <PrefRow label="EasyTier 配置" detail="从空白模板开始写一份配置，预览后应用。">
                                <button type="button" class="btn" on:click=move |_| new_config()>
                                    "新建配置"
                                </button>
                            </PrefRow>
                        </PrefAdvanced>
                    }
                        .into_any()
                }
            }}
        </PrefGroup>
    }
}

#[component]
pub(super) fn ConfigEditor(m: Mesh, text: String, err: Option<String>) -> impl IntoView {
    let t = RwSignal::new(text);
    let err = RwSignal::new(err);
    let busy = RwSignal::new(false);
    let initial_values = StoredValue::new(t.get_untracked());
    let dirty = RwSignal::new(false);
    let confirming_close = RwSignal::new(false);
    Effect::new(move |_| dirty.set(t.get() != initial_values.get_value()));
    crate::pages::agents::guard_unsaved(dirty);
    let close = Callback::new(move |()| {
        if busy.get_untracked() || confirming_close.get_untracked() {
            return;
        }
        if t.get_untracked() == initial_values.get_value() {
            m.config.try_set(None);
            return;
        }
        confirming_close.set(true);
        spawn_local(async move {
            if dialog::ask(
                "放弃修改",
                "EasyTier 配置尚未应用。关闭后，本次修改将丢失。",
                vec![Choice::plain("继续编辑"), Choice::danger("放弃修改")],
            )
            .await
                == Some(1)
                && confirming_close.try_get_untracked().is_some()
            {
                m.config.try_set(None);
            }
            confirming_close.try_set(false);
        });
    });
    view! {
        <Modal label="EasyTier 配置" class="dlg wide nodes-dialog" on_close=close>
            <h3>"EasyTier 配置"</h3>
            <div class="muted small">
                "应用后将替换本机组网配置并重启服务，需要系统授权。配置含组网密钥，将保存在本机私密位置。"
            </div>
            <textarea
                class="mono cfgedit"
                aria-label="EasyTier TOML 配置"
                disabled=move || busy.get()
                spellcheck="false"
                prop:value=move || t.get()
                on:input=move |e| t.set(event_target_value(&e))
            ></textarea>
            {move || err.get().map(|e| view! { <InlineError message=e /> })}
            <div class="dlg-foot">
                <button class="btn" disabled=move || busy.get() on:click=move |_| close.run(())>
                    "取消"
                </button>
                <button
                    class="btn primary"
                    disabled=move || busy.get()
                    on:click=move |_| {
                        if busy.get_untracked() {
                            return;
                        }
                        busy.set(true);
                        let text = t.get_untracked();
                        spawn_local(async move {
                            match api::send::<
                                Value,
                            >("POST", "/api/mesh/config/preview", &json!({ "toml": text }))
                                .await
                            {
                                Ok(p) => {
                                    m.config.set(None);
                                    m.preview.set(Some((p, text)));
                                }
                                Err(e) => {
                                    err.try_set(Some(e.to_string()));
                                }
                            }
                            busy.try_set(false);
                        });
                    }
                >
                    {move || if busy.get() { "校验中…" } else { "预览配置" }}
                </button>
            </div>
        </Modal>
    }
}

#[component]
pub(super) fn ConfigPreview(m: Mesh, p: Value, text: String) -> impl IntoView {
    let sm = p["summary"].clone();
    let list = |k: &str| {
        sm[k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let peers = list("peers");
    let listeners = list("listeners");
    let features = list("features");
    let warnings = list("warnings");
    let busy = m.operation;
    let can = p["can_join"].as_bool() == Some(true);
    let net = s(&sm, "network_name");
    let t2 = text.clone();
    view! {
        <Modal
            label="应用 EasyTier 配置"
            class="dlg nodes-dialog"
            on_close=Callback::new(move |()| {
                if !busy.get_untracked() {
                    m.preview.set(None);
                }
            })
        >
            <h3>"应用 EasyTier 配置"</h3>
            <div class="kv">
                <span class="k">"网络"</span>
                <span class="v mono">{net.clone()}</span>
            </div>
            <div class="kv">
                <span class="k">"身份"</span>
                <span class="v">
                    {if s(&sm, "auth") == "network_secret" {
                        "网络密钥（管理员 / 枢纽）"
                    } else {
                        "个人凭据"
                    }}
                </span>
            </div>
            <details class="nodes-details">
                <summary>"配置详情"</summary>
                <div class="kv">
                    <span class="k">"主机名"</span>
                    <span class="v mono">
                        {Some(s(&sm, "hostname"))
                            .filter(|x| !x.is_empty())
                            .unwrap_or_else(|| "（系统主机名）".into())}
                    </span>
                </div>
                <div class="kv">
                    <span class="k">"地址"</span>
                    <span class="v mono">
                        {Some(s(&sm, "ipv4"))
                            .filter(|x| !x.is_empty())
                            .unwrap_or_else(|| {
                                if sm["dhcp"].as_bool() == Some(true) {
                                    "自动分配".into()
                                } else {
                                    "无".into()
                                }
                            })}
                    </span>
                </div>
                <div class="kv">
                    <span class="k">"对端"</span>
                    <span class="v mono">{peers.join("\n")}</span>
                </div>
                {(!listeners.is_empty())
                    .then(|| {
                        view! {
                            <div class="kv">
                                <span class="k">"监听"</span>
                                <span class="v mono">{listeners.join("\n")}</span>
                            </div>
                        }
                    })}
                {(!features.is_empty())
                    .then(|| {
                        view! {
                            <div class="kv">
                                <span class="k">"功能"</span>
                                <span class="v">
                                    {features
                                        .into_iter()
                                        .map(|f| {
                                            view! {
                                                <span class="gchip info">{f}</span>
                                                " "
                                            }
                                        })
                                        .collect_view()}
                                </span>
                            </div>
                        }
                    })}
            </details>
            {warnings
                .into_iter()
                .map(|w| view! { <div class="notice-box warn">{w}</div> })
                .collect_view()}
            <p class="muted small">
                "应用后将替换本机组网配置并重启服务，需要系统授权。"
            </p>
            <div class="dlg-foot">
                <button
                    class="btn ghost"
                    disabled=move || busy.get()
                    on:click=move |_| {
                        m.preview.set(None);
                        m.config.set(Some((t2.clone(), None)));
                    }
                >
                    "返回修改"
                </button>
                <span class="grow"></span>
                <button
                    class="btn"
                    disabled=move || busy.get()
                    on:click=move |_| m.preview.set(None)
                >
                    "取消"
                </button>
                <button
                    class="btn primary"
                    disabled=move || busy.get() || !can
                    title=if can { "" } else { "安装包没有带组网引擎" }
                    on:click=move |_| {
                        if busy.get_untracked() {
                            return;
                        }
                        busy.set(true);
                        let (text, net) = (text.clone(), net.clone());
                        spawn_local(async move {
                            match api::send::<
                                Value,
                            >("PUT", "/api/mesh/config", &json!({ "toml": text }))
                                .await
                            {
                                Ok(st) => {
                                    busy.set(false);
                                    m.preview.set(None);
                                    toast(
                                        format!(
                                            "已应用，{net} · 本机地址 {}",
                                            Some(s(&st["node"], "virtual_ipv4"))
                                                .filter(|x| !x.is_empty())
                                                .unwrap_or_else(|| "分配中".into()),
                                        ),
                                    );
                                    m.local.update(|n| *n += 1);
                                    m.app.load_state();
                                }
                                Err(e) => {
                                    let _ = busy.try_set(false);
                                    toast(format!("应用失败：{e}"));
                                }
                            }
                        });
                    }
                >
                    {move || if busy.get() { "等待系统授权…" } else { "应用" }}
                </button>
            </div>
        </Modal>
    }
}

#[component]
pub(super) fn JoinDialog(m: Mesh, p: Value, text: Option<String>) -> impl IntoView {
    let sm = p["summary"].clone();
    let busy = m.operation;
    let can = p["can_join"].as_bool() == Some(true);
    let net = s(&sm, "network_name");
    let exp = sm["expires_at"].as_i64().unwrap_or(0);
    let peers: Vec<String> = sm["peers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let warns: Vec<String> = p["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let pending = text.is_none();
    let close = Callback::new(move |()| {
        if busy.get_untracked() {
            return;
        }
        m.join.set(None);
        if pending {
            spawn_local(async {
                if let Err(e) = api::send::<Value>("DELETE", "/api/mesh/join", &json!({})).await {
                    toast(format!("未能清除待用邀请：{e}"));
                }
            });
        }
    });
    view! {
        <Modal
            label="加入团队组网"
            class="dlg nodes-dialog"
            close_on_backdrop=false
            on_close=close
        >
            <h3>"加入团队组网"</h3>
            <div class="muted small">
                {format!(
                    "{} 邀请这台电脑加入组网。加入后，团队里的机器可以通过虚拟地址访问它，它也能访问团队的机器。",
                    Some(s(&sm, "issued_by"))
                        .filter(|x| !x.is_empty())
                        .unwrap_or_else(|| "管理员".into()),
                )}
            </div>
            <div class="kv">
                <span class="k">"网络"</span>
                <span class="v mono">{net.clone()}</span>
            </div>
            <div class="nodes-details">
                <div class="kv">
                    <span class="k">"本机主机名"</span>
                    <span class="v mono">{s(&sm, "hostname")}</span>
                </div>
                <div class="kv">
                    <span class="k">"本机地址"</span>
                    <span class="v mono">
                        {Some(s(&sm, "ipv4"))
                            .filter(|x| !x.is_empty())
                            .unwrap_or_else(|| "自动分配".into())}
                    </span>
                </div>
                <div class="kv">
                    <span class="k">"入网地址"</span>
                    <span class="v mono">{peers.join("\n")}</span>
                </div>
            </div>
            <div class="kv">
                <span class="k">"有效期至"</span>
                <span class="v">{format!("{}（{} 天）", date_of(exp), days_left(exp))}</span>
            </div>
            {(!s(&sm, "note").is_empty())
                .then(|| {
                    view! {
                        <div class="kv">
                            <span class="k">"备注"</span>
                            <span class="v">{s(&sm, "note")}</span>
                        </div>
                    }
                })}
            {warns
                .into_iter()
                .map(|w| view! { <div class="notice-box warn">{w}</div> })
                .collect_view()}
            <div class="muted small">
                "需要输入这台电脑的登录密码：组网要创建虚拟网卡，并把引擎装成开机自启的系统服务（关掉 Blazar 也保持在线）。"
            </div>
            <div class="dlg-foot">
                <button class="btn" disabled=move || busy.get() on:click=move |_| close.run(())>
                    "暂不加入"
                </button>
                <button
                    class="btn primary"
                    disabled=move || busy.get() || !can
                    on:click=move |_| {
                        if busy.get_untracked() {
                            return;
                        }
                        busy.set(true);
                        let (text, net) = (text.clone(), net.clone());
                        let ip = s(&sm, "ipv4");
                        spawn_local(async move {
                            let body = match &text {
                                Some(t) => json!({ "invite": t }),
                                None => json!({}),
                            };
                            match api::send::<Value>("POST", "/api/mesh/join", &body).await {
                                Ok(st) => {
                                    busy.set(false);
                                    m.join.set(None);
                                    toast(
                                        format!(
                                            "已加入 {net}，本机地址 {}",
                                            Some(s(&st["node"], "virtual_ipv4"))
                                                .filter(|x| !x.is_empty())
                                                .unwrap_or(ip),
                                        ),
                                    );
                                    m.local.update(|n| *n += 1);
                                    m.app.load_state();
                                }
                                Err(e) => {
                                    let _ = busy.try_set(false);
                                    toast(format!("加入失败：{e}"));
                                }
                            }
                        });
                    }
                >
                    {move || if busy.get() { "等待系统授权…" } else { "加入" }}
                </button>
            </div>
        </Modal>
    }
}
