use super::*;
use crate::components::group::ListGroup;
use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow};

#[component]
pub(super) fn Issuer(m: Mesh) -> impl IntoView {
    let v = LocalResource::new(move || {
        m.issuer.track();
        api::get::<Value>("/api/mesh/issuer")
    });
    let settings = RwSignal::new(false);
    let member = RwSignal::new(String::new());
    let host = RwSignal::new(String::new());
    let days = RwSignal::new("180".to_owned());
    let ip = RwSignal::new("auto".to_owned());
    let note = RwSignal::new(String::new());
    let msg = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let generate = move || {
        if busy.get_untracked() {
            return;
        }
        let who = member.get_untracked().trim().to_owned();
        if who.is_empty() {
            toast("请填写同事称呼");
            return;
        }
        busy.set(true);
        let body = json!({ "member": who, "hostname": Some(host.get_untracked().trim().to_owned()).filter(|x| !x.is_empty()),
            "days": days.get_untracked().parse::<i64>().unwrap_or(180), "ipv4": ip.get_untracked(), "note": note.get_untracked().trim() });
        spawn_local(async move {
            match api::send::<Value>("POST", "/api/mesh/invites", &body).await {
                Ok(r) => {
                    files_js::download_text(
                        &s(&r, "file_name"),
                        &s(&r, "content"),
                        "application/x-blazar-invite",
                    );
                    msg.try_set(format!("已生成 {}（{}）—— 发给 {who}，TA 用 Blazar 打开即可加入。文件即凭据，请私下发送。", s(&r, "file_name"), Some(s(&r["summary"], "ipv4")).filter(|x| !x.is_empty()).unwrap_or_else(|| "动态地址".into())));
                    member.try_set(String::new());
                    host.try_set(String::new());
                    note.try_set(String::new());
                    m.invites.update(|n| *n += 1);
                }
                Err(e) => toast(format!("生成失败：{e}")),
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <PrefGroup title="邀请同事">
            {move || match v.get() {
                None => {
                    view! { <LoadingState text="正在读取邀请设置…" class="pref-empty" /> }
                        .into_any()
                }
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            class="pref-empty"
                            message=format!("无法读取邀请设置：{e}")
                            retry=Callback::new(move |()| m.issuer.update(|n| *n += 1))
                        />
                    }
                        .into_any()
                }
                Some(Ok(v)) if v["configured"].as_bool() != Some(true) => {
                    view! {
                        <PrefRow
                            label="状态"
                            detail="设置签发机器后，即可生成邀请文件。加入他人的组网无需此设置。"
                        >
                            <Status tone=Tone::Other label="待设置" />
                            <button type="button" class="btn" on:click=move |_| settings.set(true)>
                                "签发设置"
                            </button>
                        </PrefRow>
                        <PrefAdvanced title="如何设置">
                            <p>
                                "选择持有网络密钥 network_secret 的机器（通常是枢纽），填写 SSH Host 或 local。"
                            </p>
                            <p>
                                "Blazar 通过 SSH 调用 easytier-cli credential 签发和吊销邀请。加入他人的组网可使用邀请文件或 EasyTier 配置，不需要这项设置。"
                            </p>
                        </PrefAdvanced>
                    }
                        .into_any()
                }
                Some(Ok(v)) => {
                    let cfg = v["config"].clone();
                    let reach = v["reachable"].as_bool() == Some(true);
                    let where_ = format!(
                        "{}{}",
                        s(&cfg, "via"),
                        Some(s(&cfg, "container"))
                            .filter(|x| !x.is_empty())
                            .map(|c| format!(" · 容器 {c}"))
                            .unwrap_or_default(),
                    );
                    let eps: Vec<String> = v["entry_points"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                        .unwrap_or_default();
                    let no_entry = eps.is_empty();
                    let detail = if reach {
                        format!("由 {where_} 签发")
                    } else {
                        format!(
                            "无法连接签发机器 {where_}，请检查连接或修改签发设置：{}",
                            s(&v, "error"),
                        )
                    };
                    view! {
                        <PrefRow label="状态" detail=detail>
                            <Status
                                tone=if reach { Tone::Other } else { Tone::Failed }
                                label=if reach { "可邀请" } else { "未连接" }
                            />
                            <button type="button" class="btn" on:click=move |_| settings.set(true)>
                                "签发设置"
                            </button>
                        </PrefRow>
                        {reach
                            .then(|| {
                                view! {
                                    <PrefRow label="同事称呼" for_id="invite-member">
                                        <input
                                            id="invite-member"
                                            class="settings-input"
                                            placeholder="张三"
                                            disabled=move || busy.get()
                                            prop:value=move || member.get()
                                            on:input=move |e| member.set(event_target_value(&e))
                                        />
                                    </PrefRow>
                                    <PrefRow
                                        label="主机名"
                                        detail="组网内唯一；称呼是英文时可留空。"
                                        for_id="invite-host"
                                    >
                                        <input
                                            id="invite-host"
                                            class="settings-input mono"
                                            placeholder="zhangsan-mbp"
                                            disabled=move || busy.get()
                                            prop:value=move || host.get()
                                            on:input=move |e| host.set(event_target_value(&e))
                                        />
                                    </PrefRow>
                                    <PrefRow label="有效期" for_id="invite-days">
                                        <select
                                            id="invite-days"
                                            class="settings-input"
                                            disabled=move || busy.get()
                                            prop:value=move || days.get()
                                            on:change=move |e| days.set(event_target_value(&e))
                                        >
                                            <option value="30" selected=move || days.get() == "30">
                                                "30 天"
                                            </option>
                                            <option value="90" selected=move || days.get() == "90">
                                                "90 天"
                                            </option>
                                            <option value="180" selected=move || days.get() == "180">
                                                "180 天"
                                            </option>
                                            <option value="365" selected=move || days.get() == "365">
                                                "1 年"
                                            </option>
                                        </select>
                                    </PrefRow>
                                    <PrefRow label="地址" for_id="invite-ip">
                                        <select
                                            id="invite-ip"
                                            class="settings-input"
                                            disabled=move || busy.get()
                                            prop:value=move || ip.get()
                                            on:change=move |e| ip.set(event_target_value(&e))
                                        >
                                            <option value="auto" selected=move || ip.get() == "auto">
                                                "自动分配固定地址"
                                            </option>
                                            <option value="dhcp" selected=move || ip.get() == "dhcp">
                                                "由网络动态分配"
                                            </option>
                                        </select>
                                    </PrefRow>
                                    <PrefRow label="备注" for_id="invite-note">
                                        <input
                                            id="invite-note"
                                            class="settings-input"
                                            placeholder="可选，如：算法组 · 实习"
                                            disabled=move || busy.get()
                                            prop:value=move || note.get()
                                            on:input=move |e| note.set(event_target_value(&e))
                                        />
                                    </PrefRow>
                                    <PrefRow
                                        label="邀请文件"
                                        detail="邀请文件就是个人入网凭据，请私下发送。可单独吊销，到期自动失效，不含整个网络的密钥。"
                                    >
                                        <button
                                            type="button"
                                            class="btn primary"
                                            disabled=move || busy.get() || no_entry
                                            on:click=move |_| generate()
                                        >
                                            {move || if busy.get() { "生成中…" } else { "生成邀请文件" }}
                                        </button>
                                    </PrefRow>
                                    {no_entry
                                        .then(|| {
                                            view! {
                                                <p class="pref-status bad">"请先在签发设置中填写入网地址。"</p>
                                            }
                                        })}
                                    <p class="pref-status" role="status" aria-live="polite">
                                        {move || msg.get()}
                                    </p>
                                    <PrefAdvanced title="签发详情">
                                        {text_row("机器", where_.clone(), true)}
                                        {text_row("网络", dash(s(&v, "network_name")), true)}
                                        {text_row("网段", dash(s(&v, "subnet")), true)}
                                        {text_row(
                                            "入网地址",
                                            if no_entry {
                                                "未设置 —— 在「签发设置」里填写".into()
                                            } else {
                                                eps.join("，")
                                            },
                                            true,
                                        )}
                                    </PrefAdvanced>
                                }
                            })}
                    }
                        .into_any()
                }
            }}
            {move || {
                (settings.get())
                    .then(|| {
                        view! {
                            <IssuerSettings
                                v=v.get().and_then(Result::ok).unwrap_or(Value::Null)
                                on_close=move |saved: bool| {
                                    settings.try_set(false);
                                    if saved {
                                        m.issuer.update(|n| *n += 1);
                                        m.invites.update(|n| *n += 1);
                                        m.local.update(|n| *n += 1);
                                        m.app.load_state();
                                    }
                                }
                            />
                        }
                    })
            }}
        </PrefGroup>
    }
}

#[component]
fn IssuerSettings(
    v: Value,
    on_close: impl Fn(bool) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let busy = RwSignal::new(false);
    let c = v["config"].clone();
    let via = RwSignal::new(s(&c, "via"));
    let ct = RwSignal::new(s(&c, "container"));
    let eps = RwSignal::new(
        c["entry_points"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
    );
    let derived = v["entry_points"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("，")
        })
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| "无".into());
    let configured = v["configured"].as_bool() == Some(true);
    let initial_values =
        StoredValue::new((via.get_untracked(), ct.get_untracked(), eps.get_untracked()));
    let dirty = RwSignal::new(false);
    let confirming_close = RwSignal::new(false);
    Effect::new(move |_| dirty.set((via.get(), ct.get(), eps.get()) != initial_values.get_value()));
    crate::pages::agents::guard_unsaved(dirty);
    let close = Callback::new(move |()| {
        if busy.get_untracked() || confirming_close.get_untracked() {
            return;
        }
        if (via.get_untracked(), ct.get_untracked(), eps.get_untracked())
            == initial_values.get_value()
        {
            on_close(false);
            return;
        }
        confirming_close.set(true);
        spawn_local(async move {
            if dialog::ask(
                "放弃修改",
                "签发设置尚未保存。关闭后，本次修改将丢失。",
                vec![Choice::plain("继续编辑"), Choice::danger("放弃修改")],
            )
            .await
                == Some(1)
                && confirming_close.try_get_untracked().is_some()
            {
                on_close(false);
            }
            confirming_close.try_set(false);
        });
    });
    view! {
        <Modal label="签发设置" class="dlg wide nodes-dialog" on_close=close>
            <h3>"签发设置"</h3>
            <div class="notice-box warn">
                "签发机器必须配置 credential_file，否则重启后已签发的凭据会全部失效。"
            </div>
            <details class="nodes-details">
                <summary>"签发原理"</summary>
                <p class="muted small">
                    "选择持有 network_secret 的机器，通常是枢纽。Blazar 通过 SSH 调用 easytier-cli credential。"
                </p>
            </details>
            <fieldset class="nodes-form" disabled=move || busy.get()>
                <div class="grid2">
                    <label class="field">
                        "签发节点（SSH Host 或 local）"
                        <input
                            class="mono"
                            placeholder="hub-host"
                            prop:value=move || via.get()
                            on:input=move |e| via.set(event_target_value(&e))
                        />
                    </label>
                    <label class="field">
                        "Docker 容器名（可选）"
                        <input
                            class="mono"
                            placeholder="easytier"
                            prop:value=move || ct.get()
                            on:input=move |e| ct.set(event_target_value(&e))
                        />
                    </label>
                </div>
                <label class="field">
                    "入网地址"
                    <textarea
                        class="mono"
                        rows="3"
                        placeholder="tcp://203.0.113.10:11010"
                        prop:value=move || eps.get()
                        on:input=move |e| eps.set(event_target_value(&e))
                    ></textarea>
                    <span class="muted small">
                        "每行一个，必须公网可达。留空则从签发机器配置推导。"
                    </span>
                </label>
                <details class="nodes-details">
                    <summary>"当前推导地址"</summary>
                    <div class="mono small">{derived}</div>
                </details>
            </fieldset>
            <div class="dlg-foot">
                {configured
                    .then(|| {
                        view! {
                            <button
                                class="btn danger"
                                disabled=move || busy.get()
                                on:click=move |_| spawn_local(async move {
                                    if dialog::ask(
                                            "清除签发节点设置",
                                            "清除签发节点设置？回到「未配置」，已签发的邀请记录不动。",
                                            vec![Choice::plain("取消"), Choice::danger("清除")],
                                        )
                                        .await != Some(1)
                                    {
                                        return;
                                    }
                                    if busy.try_get_untracked().unwrap_or(true) {
                                        return;
                                    }
                                    busy.set(true);
                                    match api::send::<
                                        Value,
                                    >("DELETE", "/api/mesh/issuer", &json!({}))
                                        .await
                                    {
                                        Ok(_) => {
                                            toast("已清除");
                                            on_close(true);
                                        }
                                        Err(e) => toast(format!("清除失败：{e}")),
                                    }
                                    busy.try_set(false);
                                })
                            >
                                "清除设置"
                            </button>
                        }
                    })} <span class="grow"></span>
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
                        if via.get_untracked().trim().is_empty() {
                            toast("请填写签发节点");
                            return;
                        }
                        busy.set(true);
                        let body = json!(
                            { "via": via.get_untracked().trim(), "container": Some(ct.get_untracked().trim().to_owned()).filter(|x| !x.is_empty()),
                            "entry_points": eps.get_untracked().lines().map(str::trim).filter(|x| !x.is_empty()).collect::<Vec<_>>() }
                        );
                        spawn_local(async move {
                            match api::send::<Value>("PUT", "/api/mesh/issuer", &body).await {
                                Ok(_) => {
                                    toast("已保存");
                                    on_close(true);
                                }
                                Err(e) => toast(format!("保存失败：{e}")),
                            }
                            busy.try_set(false);
                        });
                    }
                >
                    {move || if busy.get() { "保存中…" } else { "保存" }}
                </button>
            </div>
        </Modal>
    }
}

#[component]
pub(super) fn Invites(m: Mesh) -> impl IntoView {
    let revoking = RwSignal::new(false);
    let v = LocalResource::new(move || {
        m.invites.track();
        api::get::<Value>("/api/mesh/invites")
    });
    let count = Signal::derive(move || {
        v.get()
            .and_then(Result::ok)
            .and_then(|v| v["invites"].as_array().map(Vec::len))
            .unwrap_or(0)
    });
    view! {
        <ListGroup title="邀请记录" count=count>
            {move || match v.get() {
                None => {
                    view! { <LoadingState text="正在读取邀请…" class="pref-empty" /> }.into_any()
                }
                Some(Err(e)) => {
                    view! {
                        <InlineError
                            class="pref-empty"
                            message=format!("无法读取邀请记录：{e}")
                            retry=Callback::new(move |()| m.invites.update(|n| *n += 1))
                        />
                    }
                        .into_any()
                }
                Some(Ok(v)) => {
                    let list: Vec<Value> = v["invites"].as_array().cloned().unwrap_or_default();
                    let lost = list.iter().filter(|i| i["state"] == "lost").count();
                    view! {
                        {(lost > 0)
                            .then(|| {
                                view! {
                                    <p class="pref-status bad">
                                        {format!(
                                            "{lost} 份邀请的凭据已丢失，需要重新签发（通常是签发节点重启、且没有配置 credential_file）。",
                                        )}
                                    </p>
                                }
                            })}
                        {(v["issuer_checked"].as_bool() != Some(true))
                            .then(|| {
                                view! { <p class="pref-status">"签发节点连不上，状态未核对"</p> }
                            })}
                        {if list.is_empty() {
                            view! {
                                <EmptyState
                                    title="还没有邀请"
                                    detail="生成邀请文件后，可在这里查看状态或吊销。"
                                    class="pref-empty"
                                />
                            }
                                .into_any()
                        } else {
                            view! {
                                <div class="nodes-table">
                                    <table class="tb">
                                        <thead>
                                            <tr>
                                                <th>"同事"</th>
                                                <th>"连接详情"</th>
                                                <th>"状态"</th>
                                                <th>"到期"</th>
                                                <th></th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {list
                                                .into_iter()
                                                .map(|i| {
                                                    let st = s(&i, "state");
                                                    let (tone, text) = match st.as_str() {
                                                        "online" => (Tone::Other, "已入网"),
                                                        "waiting" => (Tone::Other, "待使用"),
                                                        "offline" => (Tone::Other, "离线"),
                                                        "lost" => (Tone::Failed, "凭据丢失"),
                                                        "expired" => (Tone::Other, "已过期"),
                                                        "revoked" => (Tone::Other, "已吊销"),
                                                        _ => (Tone::Attention, "待核对"),
                                                    };
                                                    let live = matches!(
                                                        st.as_str(),
                                                        "online" | "waiting" | "offline" | "lost"
                                                    );
                                                    let exp = i["expires_at"].as_i64().unwrap_or(0);
                                                    let (id, who) = (s(&i, "id"), s(&i, "member"));
                                                    view! {
                                                        <tr>
                                                            <td title=s(&i, "note")>{s(&i, "member")}</td>
                                                            <td>
                                                                <details class="nodes-details">
                                                                    <summary>"查看"</summary>
                                                                    <div class="mono small">{s(&i, "hostname")}</div>
                                                                    <div class="mono small">
                                                                        {Some(s(&i, "ipv4"))
                                                                            .filter(|x| !x.is_empty())
                                                                            .unwrap_or_else(|| "动态地址".into())}
                                                                    </div>
                                                                    <div class="muted small">{s(&i, "note")}</div>
                                                                </details>
                                                            </td>
                                                            <td>
                                                                <Status tone label=text />
                                                            </td>
                                                            <td
                                                                class="muted small"
                                                                title=format!(
                                                                    "签发于 {}",
                                                                    date_of(i["issued_at"].as_i64().unwrap_or(0)),
                                                                )
                                                            >
                                                                {if st == "revoked" {
                                                                    "—".to_owned()
                                                                } else {
                                                                    format!(
                                                                        "{}{}",
                                                                        date_of(exp),
                                                                        if live {
                                                                            format!(" · {} 天", days_left(exp))
                                                                        } else {
                                                                            String::new()
                                                                        },
                                                                    )
                                                                }}
                                                            </td>
                                                            <td style="text-align:right">
                                                                {live
                                                                    .then(|| {
                                                                        view! {
                                                                            <button
                                                                                class="btn small danger"
                                                                                disabled=move || revoking.get()
                                                                                on:click=move |_| {
                                                                                    let (id, who) = (id.clone(), who.clone());
                                                                                    spawn_local(async move {
                                                                                        if dialog::ask(
                                                                                                "吊销邀请",
                                                                                                &format!(
                                                                                                    "吊销 {who} 的邀请？TA 的电脑会立即断开组网。",
                                                                                                ),
                                                                                                vec![Choice::plain("取消"), Choice::danger("吊销")],
                                                                                            )
                                                                                            .await != Some(1)
                                                                                        {
                                                                                            return;
                                                                                        }
                                                                                        if revoking.try_get_untracked().unwrap_or(true) {
                                                                                            return;
                                                                                        }
                                                                                        revoking.set(true);
                                                                                        match api::send::<
                                                                                            Value,
                                                                                        >(
                                                                                                "DELETE",
                                                                                                &format!("/api/mesh/invites/{}", api::enc(&id)),
                                                                                                &json!({}),
                                                                                            )
                                                                                            .await
                                                                                        {
                                                                                            Ok(_) => {
                                                                                                toast("已吊销");
                                                                                                m.invites.update(|n| *n += 1);
                                                                                            }
                                                                                            Err(e) => toast(format!("吊销失败：{e}")),
                                                                                        }
                                                                                        revoking.try_set(false);
                                                                                    });
                                                                                }
                                                                            >
                                                                                "吊销"
                                                                            </button>
                                                                        }
                                                                    })}
                                                            </td>
                                                        </tr>
                                                    }
                                                })
                                                .collect_view()}
                                        </tbody>
                                    </table>
                                </div>
                            }
                                .into_any()
                        }}
                    }
                        .into_any()
                }
            }}
        </ListGroup>
    }
}
