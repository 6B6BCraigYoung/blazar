use super::*;

pub(super) fn remove_ssh(name: String) {
    spawn_local(async move {
        let body = "只是从 Blazar 的机器列表里去掉，~/.ssh/config 不动，以后还能再加回来。";
        let choices = vec![Choice::plain("取消"), Choice::danger("移除")];
        if dialog::ask(&format!("移除 {name}？"), body, choices).await != Some(1) {
            return;
        }
        match api::send::<Value>(
            "DELETE",
            &format!("/api/ssh-hosts/{}", api::enc(&name)),
            &json!({}),
        )
        .await
        {
            Ok(_) => toast(format!("已移除 {name}")),
            Err(e) => toast(format!("移除失败：{e}")),
        }
    });
}

#[component]
pub(super) fn SshPicker(#[prop(into)] on_close: Callback<()>) -> impl IntoView {
    let app = use_app();
    let rev = RwSignal::new(0u32);
    let hosts = LocalResource::new(move || {
        rev.track();
        api::get::<Vec<Value>>("/api/ssh-hosts")
    });
    let q = RwSignal::new(String::new());
    let picked = RwSignal::new(std::collections::BTreeSet::<String>::new());
    let busy = RwSignal::new(false);
    let add = move |_| {
        let names: Vec<String> = picked.get_untracked().into_iter().collect();
        if names.is_empty() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            match api::send::<Value>("POST", "/api/ssh-hosts", &json!({ "names": names })).await {
                Ok(r) => {
                    let n = r["added"].as_array().map_or(0, Vec::len);
                    toast(format!("已添加 {n} 台机器，正在检查能不能连上"));
                    app.load_state();
                    on_close.run(());
                }
                Err(e) => {
                    let _ = busy.try_set(false);
                    toast(format!("添加失败：{e}"));
                }
            }
        });
    };
    view! {
        <Modal label="添加 SSH 机器" class="dlg ssh-pick nodes-dialog" on_close=Callback::new(move |()| { if !busy.get_untracked() { on_close.run(()); } })>
                <h3>"添加 SSH 机器"</h3>
                <p class="muted small">"选择要使用的机器，即可创建工作区。"</p><details class="nodes-details"><summary>"连接方式"</summary><p class="muted small">"读取本机 ~/.ssh/config，沿用 SSH 密钥和 ProxyJump 设置。添加后无需加入组网，也能创建工作区、同步 Claude Code / Codex 版本。"</p></details>
                <input class="page-filter" aria-label="筛选 SSH 机器" placeholder="搜索名称或地址" prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
                <div class="ssh-list">
                    {move || match hosts.get() {
                        None => view! { <LoadingState text="正在读取 SSH 机器…"/> }.into_any(),
                        Some(Err(e)) => view! { <InlineError message=format!("无法读取 SSH 配置：{e}") retry=Callback::new(move |()| rev.update(|n| *n += 1))/> }.into_any(),
                        Some(Ok(list)) => {
                            let k = q.get().to_lowercase();
                            let rows: Vec<Value> = list.into_iter().filter(|h| k.is_empty() || s(h, "alias").to_lowercase().contains(&k) || s(h, "hostname").to_lowercase().contains(&k)).collect();
                            if rows.is_empty() {
                                return view! { <EmptyState title=if k.is_empty() { "没有可添加的机器" } else { "没有匹配的机器" } detail=if k.is_empty() { "先在 ~/.ssh/config 中添加 SSH Host，再重试。" } else { "换个名称或地址搜索。" }/> }.into_any();
                            }
                            rows.into_iter().map(|h| {
                                let alias = s(&h, "alias");
                                let added = h["added"].as_bool() == Some(true);
                                let mesh = h["in_mesh"].as_bool() == Some(true);
                                let user = s(&h, "user");
                                let port = s(&h, "port");
                                let target = format!("{}{}{}",
                                    if user.is_empty() { String::new() } else { format!("{user}@") },
                                    Some(s(&h, "hostname")).filter(|x| !x.is_empty()).unwrap_or_else(|| alias.clone()),
                                    if port.is_empty() { String::new() } else { format!(":{port}") });
                                let file = s(&h, "file");
                                let file = file.rsplit('/').next().unwrap_or_default().to_owned();
                                let a1 = alias.clone();
                                let a2 = alias.clone();
                                view! {
                                    <label class="ssh-row" data-off=(added || mesh).to_string()>
                                        <input type="checkbox" disabled=move || busy.get() || added || mesh
                                            prop:checked=move || added || picked.with(|p| p.contains(&a1))
                                            on:change=move |e| { let on = event_target_checked(&e); let a = a2.clone(); picked.update(|p| { if on { p.insert(a); } else { p.remove(&a); } }); }/>
                                        <b class="mono">{alias.clone()}</b>
                                        <span class="mono muted">{target}</span>
                                        <span class="grow"></span>
                                        {if mesh { Some(view! { <span class="gchip">"在组网里"</span> }.into_any()) } else if added { Some(view! { <span class="gchip">"已添加"</span> }.into_any()) } else { (file != "config").then(|| view! { <span class="muted small">{file}</span> }.into_any()) }}
                                    </label>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                </div>
                <div class="dlg-foot">
                    <button class="btn" disabled=move || busy.get() on:click=move |_| on_close.run(())>"取消"</button>
                    <button class="btn primary" disabled=move || busy.get() || picked.with(|p| p.is_empty()) on:click=add>
                        {move || { let n = picked.with(|p| p.len()); if busy.get() { "添加中…".to_owned() } else if n == 0 { "添加".to_owned() } else { format!("添加 {n} 台") } }}
                    </button>
                </div>
        </Modal>
    }
}
