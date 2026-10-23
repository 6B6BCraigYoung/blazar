use super::*;
use crate::components::prefs::{PrefAdvanced, PrefGroup, PrefRow};

#[component]
pub fn NodeDetailPage() -> impl IntoView {
    let params = use_params_map();
    let name = move || {
        params
            .read()
            .get("name")
            .map(|n| {
                js_sys::decode_uri_component(&n)
                    .map(String::from)
                    .unwrap_or(n)
            })
            .unwrap_or_default()
    };
    move || {
        let name = name();
        view! { <NodeDetail name /> }
    }
}

fn kb(n: f64) -> String {
    if n > 1_048_576.0 {
        format!("{:.1} GB", n / 1_048_576.0)
    } else if n > 1024.0 {
        format!("{:.0} MB", n / 1024.0)
    } else {
        format!("{n} KB")
    }
}

fn orphan_state(l: &Value) -> String {
    let dirty = l["dirty"].as_u64().unwrap_or(0);
    if dirty > 0 {
        format!("{dirty} 处未提交")
    } else if l["broken"].as_bool() == Some(true) {
        "目录已损坏".into()
    } else if !l["worktree"].is_string() {
        "只剩私有目录".into()
    } else {
        "干净".into()
    }
}

#[component]
fn NodeDetail(name: String) -> impl IntoView {
    let app = use_app();
    let n2 = name.clone();
    let node = Memo::new(move |_| {
        app.state.with(|s| {
            s.as_ref()
                .and_then(|s| s.nodes.iter().find(|n| n.name == n2).cloned())
        })
    });
    let hw = RwSignal::new(None::<Result<Value, String>>);
    let agents = RwSignal::new(None::<Result<(Vec<Value>, Value), String>>);
    let left = RwSignal::new(None::<Result<Value, String>>);
    let checked = RwSignal::new(std::collections::HashSet::<String>::new());
    let force = RwSignal::new(false);
    let nm = StoredValue::new(name.clone());
    let probing = RwSignal::new(false);
    let scanning = RwSignal::new(false);
    let listing_left = RwSignal::new(false);
    let sweeping = RwSignal::new(false);
    let probe = move || {
        if probing.get_untracked() {
            return;
        }
        probing.set(true);
        hw.set(None);
        let n = nm.get_value();
        spawn_local(async move {
            let r = api::send::<Value>(
                "POST",
                &format!("/api/nodes/{}/probe", api::enc(&n)),
                &json!({}),
            )
            .await
            .map_err(|e| e.to_string());
            let _ = hw.try_set(Some(r));
            probing.try_set(false);
            app.load_state();
        });
    };
    let scan = move || {
        let Some(n) = nm.try_get_value() else {
            return;
        };
        if scanning.try_get_untracked().unwrap_or(true) {
            return;
        }
        scanning.set(true);
        agents.set(None);
        spawn_local(async move {
            let found = api::get::<Vec<Value>>(&format!("/api/nodes/{}/agents", api::enc(&n)))
                .await
                .map_err(|e| e.to_string());
            let vers = if n == "local" {
                Value::Null
            } else {
                api::get::<Value>(&format!("/api/nodes/{}/cli-versions", api::enc(&n)))
                    .await
                    .unwrap_or(Value::Null)
            };
            let _ = agents.try_set(Some(found.map(|f| (f, vers))));
            scanning.try_set(false);
        });
    };
    let leftovers = move || {
        let Some(n) = nm.try_get_value() else {
            return;
        };
        if listing_left.try_get_untracked().unwrap_or(true) {
            return;
        }
        listing_left.set(true);
        force.set(false);
        left.set(None);
        spawn_local(async move {
            let r = api::get::<Value>(&format!("/api/nodes/{}/leftovers", api::enc(&n)))
                .await
                .map_err(|e| e.to_string());
            if let Ok(v) = &r {
                checked.try_set(
                    v["orphans"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter(|l| l["dirty"].as_u64().unwrap_or(0) == 0)
                                .map(|l| s(l, "leaf"))
                                .collect()
                        })
                        .unwrap_or_default(),
                );
            }
            let _ = left.try_set(Some(r));
            listing_left.try_set(false);
        });
    };
    let sweep = move || {
        if sweeping.get_untracked() {
            return;
        }
        let leaves: Vec<String> = checked.get_untracked().into_iter().collect();
        if leaves.is_empty() {
            toast("请先选择目录");
            return;
        }
        let f = force.get_untracked();
        let n = nm.get_value();
        spawn_local(async move {
            let warning = if f {
                "所选目录及其中未提交的改动会永久删除，无法撤销。"
            } else {
                "所选残留目录会永久删除；有未提交改动的目录将被跳过。"
            };
            if dialog::ask(
                "删除残留目录",
                warning,
                vec![Choice::plain("取消"), Choice::danger("删除目录")],
            )
            .await
                != Some(1)
            {
                return;
            }
            if sweeping.try_get_untracked().unwrap_or(true) {
                return;
            }
            sweeping.set(true);
            match api::send::<Value>(
                "POST",
                &format!("/api/nodes/{}/sweep", api::enc(&n)),
                &json!({ "leaves": leaves, "force": f }),
            )
            .await
            {
                Ok(s2) => {
                    let removed = s2["removed"].as_array().map_or(0, Vec::len);
                    let skipped: Vec<String> = s2["skipped"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|x| {
                                    format!(
                                        "{}（{}）",
                                        x[0].as_str().unwrap_or(""),
                                        x[1].as_str().unwrap_or(""),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    toast(if skipped.is_empty() {
                        format!("清掉 {removed} 份")
                    } else {
                        format!(
                            "清掉 {removed} 份；跳过 {} 份：{}",
                            skipped.len(),
                            skipped.join("；"),
                        )
                    });
                    leftovers();
                }
                Err(e) => toast(format!("清扫失败：{e}")),
            }
            sweeping.try_set(false);
        });
    };
    let update = move |id: String, busy: RwSignal<bool>, verb: &'static str| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let n = nm.get_value();
        spawn_local(async move {
            match api::send::<Value>(
                "POST",
                &format!("/api/nodes/{}/update/{id}", api::enc(&n)),
                &json!({}),
            )
            .await
            {
                Ok(r) => toast(s(&r, "message")),
                Err(e) => toast(format!("{verb}失败：{e}")),
            }
            busy.try_set(false);
            scan();
        });
    };
    let online = move || node.with(|n| n.as_ref().is_some_and(|n| n.status == "online"));
    let probed = RwSignal::new(false);
    Effect::new(move |_| {
        if online() && !probed.get_untracked() {
            probed.set(true);
            probe();
        }
    });
    crate::refocus::on_refocus(move || {
        if online() {
            probe();
        }
    });
    let ws_here = move || {
        app.workspaces()
            .into_iter()
            .filter(|w| w.node == nm.get_value())
            .collect::<Vec<_>>()
    };
    let title = crate::fmt::machine(&name);

    view! {
        <div class="page nodes-page detail-page">
            <PageHead
                parent=("机器与组网", "/nodes")
                icon=|| view! { <span class="ic" inner_html=icon::svg(icon::NODES)></span> }
                title=title
                status=move || {
                    let on = node.get().is_some_and(|n| n.status == "online");
                    let seen = node
                        .get()
                        .and_then(|n| n.last_seen_at)
                        .filter(|_| !on)
                        .map(|t| format!("最后在线 {}", crate::fmt::ago(&t)));
                    view! {
                        <StateOf state=if on { "online" } else { "offline" } />
                        {seen.map(|t| view! { <span class="page-note">{t}</span> })}
                        <Checking busy=probing />
                    }
                }
            >
                <button
                    type="button"
                    class="btn primary"
                    aria-haspopup="dialog"
                    disabled=move || !online()
                    title=move || (!online()).then_some("机器离线，恢复在线后才能新建工作区")
                    on:click=move |_| {
                        app.new_ws_node.set(Some(nm.get_value()));
                        app.new_ws.set(true);
                    }
                >
                    "新建工作区"
                </button>
            </PageHead>
            <div class="detail-body">
                <PrefGroup title="工作区">
                    {move || {
                        let list = ws_here();
                        if list.is_empty() {
                            return view! {
                                <PrefRow
                                    label="还没有工作区"
                                    detail="在这台机器上选一个代码目录，智能体就能在这里干活。"
                                />
                            }
                                .into_any();
                        }
                        list.into_iter()
                            .map(|w| {
                                let parent = w
                                    .parent
                                    .as_deref()
                                    .and_then(|p| app.workspace(p))
                                    .map(|p| {
                                        format!("属于「{}」 · ", crate::fmt::ws_title(&p.name, &p.node))
                                    })
                                    .unwrap_or_default();
                                view! {
                                    <PrefRow
                                        label=crate::fmt::ws_title(&w.name, &w.node)
                                        detail=format!("{parent}{}", w.path)
                                    >
                                        <StateOf state=w.activity.clone() />
                                        <a class="btn" href=format!("/w/{}", w.id)>
                                            "打开"
                                        </a>
                                    </PrefRow>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </PrefGroup>
                <PrefGroup title="机器状态" note="硬件资源和 AI 服务连接，进入页面和回到窗口时自动检查。">
                    {move || match hw.get() {
                        None if probing.get() => {
                            view! { <p class="pref-status">"正在检查机器…"</p> }.into_any()
                        }
                        None => {
                            view! {
                                <PrefRow
                                    label="尚未检查"
                                    detail="机器在线后自动读取 CPU、内存、磁盘、GPU 和 AI 服务连接。"
                                />
                            }
                                .into_any()
                        }
                        Some(Err(e)) => {
                            view! {
                                <InlineError
                                    message=format!("检查失败：{e}")
                                    retry=Callback::new(move |()| probe())
                                />
                            }
                                .into_any()
                        }
                        Some(Ok(c)) => {
                            let gpus: Vec<String> = c["gpus"]
                                .as_array()
                                .map(|a| {
                                    a.iter().filter_map(Value::as_str).map(str::to_owned).collect()
                                })
                                .unwrap_or_default();
                            let mut uniq = gpus.clone();
                            uniq.sort();
                            uniq.dedup();
                            let eg: Vec<(String, i64)> = c["egress"]
                                .as_object()
                                .map(|o| {
                                    o.iter()
                                        .map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0)))
                                        .collect()
                                })
                                .unwrap_or_default();
                            let egress_ok = c["has_ai_egress"].as_bool() == Some(true);
                            let gpu_text = if gpus.is_empty() {
                                "无".to_owned()
                            } else {
                                format!("{} 块 · {}", gpus.len(), uniq.join(" / "))
                            };
                            let system = format!("{} {}", s(&c, "os"), s(&c, "arch"));
                            let load = c["load1"].to_string();
                            let cpu = format!("{} 核", c["cpus"]);
                            let mem = format!("{} GB", c["mem_gb"]);
                            let disk = format!("{} GB", c["disk_free_gb"]);
                            view! {
                                <PrefRow label="CPU">
                                    <span class="pref-text">{cpu}</span>
                                </PrefRow>
                                <PrefRow label="内存">
                                    <span class="pref-text">{mem}</span>
                                </PrefRow>
                                <PrefRow label="可用磁盘">
                                    <span class="pref-text">{disk}</span>
                                </PrefRow>
                                <PrefRow label="GPU">
                                    <span class="pref-text">{gpu_text}</span>
                                </PrefRow>
                                <PrefRow label="AI 服务">
                                    <span class="pref-text" class:bad=!egress_ok>
                                        {if egress_ok {
                                            "能连上"
                                        } else {
                                            "连不上，请在运行时的「高级配置」里配置代理。"
                                        }}
                                    </span>
                                </PrefRow>
                                <PrefAdvanced title="硬件与连接详情">
                                    {
                                        let system = system.clone();
                                        view! {
                                            <PrefRow label="系统">
                                                <code>{system}</code>
                                            </PrefRow>
                                        }
                                    }
                                    {
                                        let load = load.clone();
                                        view! {
                                            <PrefRow label="负载">
                                                <code>{load}</code>
                                            </PrefRow>
                                        }
                                    }
                                    {
                                        let eg = eg.clone();
                                        eg.into_iter()
                                            .map(|(k, v)| {
                                                let ok = (200..300).contains(&v) || v == 401;
                                                view! {
                                                    <PrefRow label=k>
                                                        <span class=if ok {
                                                            "gchip ok"
                                                        } else {
                                                            "gchip bad"
                                                        }>{format!("HTTP {v}")}</span>
                                                    </PrefRow>
                                                }
                                            })
                                            .collect_view()
                                    }
                                </PrefAdvanced>
                            }
                                .into_any()
                        }
                    }}
                </PrefGroup>
                <PrefGroup title="运行时" note="这台机器上装了哪些运行时，以及登录状态。">
                    {move || match agents.get() {
                        None if scanning.get() => {
                            view! { <p class="pref-status">"正在读取运行时…"</p> }.into_any()
                        }
                        None => {
                            view! {
                                <PrefRow label="尚未检查" detail="读取已安装的运行时及登录状态。">
                                    <button
                                        type="button"
                                        class="btn"
                                        disabled=move || !online()
                                        title=move || (!online()).then_some("机器离线，恢复在线后才能检查")
                                        on:click=move |_| scan()
                                    >
                                        "检查运行时"
                                    </button>
                                </PrefRow>
                            }
                                .into_any()
                        }
                        Some(Err(e)) => {
                            view! {
                                <InlineError
                                    message=format!("无法读取运行时：{e}")
                                    retry=Callback::new(move |()| scan())
                                />
                            }
                                .into_any()
                        }
                        Some(Ok((found, vers))) => {
                            let n = nm.get_value();
                            let missing: Vec<Value> = found
                                .iter()
                                .filter(|a| {
                                    n != "local" && !a["path"].is_string()
                                        && matches!(s(a, "id").as_str(), "claude" | "codex")
                                })
                                .cloned()
                                .collect();
                            let inst: Vec<Value> = found
                                .into_iter()
                                .filter(|a| a["path"].is_string())
                                .collect();
                            let rescan = view! {
                                <PrefRow label="重新读取" detail="安装或登录后，重新检查一次。">
                                    <button
                                        type="button"
                                        class="btn"
                                        disabled=move || scanning.get() || !online()
                                        on:click=move |_| scan()
                                    >
                                        "检查运行时"
                                    </button>
                                </PrefRow>
                            };
                            if inst.is_empty() && missing.is_empty() {
                                return view! {
                                    <PrefRow
                                        label="未发现运行时"
                                        detail="在这台机器安装 Claude Code 或 Codex 后，重新检查。"
                                    />
                                    {rescan}
                                }
                                    .into_any();
                            }
                            view! {
                                {inst
                                    .into_iter()
                                    .map(|a| {
                                        let id = s(&a, "id");
                                        let v = vers[&id].clone();
                                        let behind = v["behind"].as_bool() == Some(true);
                                        let local_v = s(&v, "local");
                                        let busy = RwSignal::new(false);
                                        let (id2, n3) = (id.clone(), n.clone());
                                        let authed = a["authed"].as_bool();
                                        let detail = [
                                            Some(s(&a, "version")).filter(|x| !x.is_empty()),
                                            Some(s(&a, "path")),
                                            Some(s(&a, "auth_hint")).filter(|x| !x.is_empty()),
                                            behind.then(|| format!("本机是 {local_v}")),
                                        ]
                                            .into_iter()
                                            .flatten()
                                            .collect::<Vec<_>>()
                                            .join(" · ");
                                        view! {
                                            <PrefRow label=s(&a, "label") detail=detail>
                                                {match authed {
                                                    Some(true) => {
                                                        view! { <Status tone=Tone::Other label="已登录" /> }
                                                            .into_any()
                                                    }
                                                    Some(false) => {
                                                        view! { <Status tone=Tone::Attention label="未登录" /> }
                                                            .into_any()
                                                    }
                                                    None => {
                                                        view! { <Status tone=Tone::Attention label="待核对" /> }
                                                            .into_any()
                                                    }
                                                }}
                                                {behind
                                                    .then(|| {
                                                        view! {
                                                            <button
                                                                type="button"
                                                                class="btn"
                                                                disabled=move || busy.get()
                                                                on:click=move |_| update(id2.clone(), busy, "更新")
                                                            >
                                                                {move || if busy.get() { "更新中…" } else { "更新" }}
                                                            </button>
                                                        }
                                                    })}
                                                {(id == "codex" && n3 != "local")
                                                    .then(|| {
                                                        view! {
                                                            <button
                                                                type="button"
                                                                class="btn"
                                                                on:click=move |_| crate::pages::runtimes::node_login(
                                                                    n3.clone(),
                                                                    Callback::new(move |_| scan()),
                                                                )
                                                            >
                                                                {if authed == Some(true) { "重新登录" } else { "登录" }}
                                                            </button>
                                                        }
                                                    })}
                                            </PrefRow>
                                        }
                                    })
                                    .collect_view()}
                                {missing
                                    .into_iter()
                                    .map(|a| {
                                        let id = s(&a, "id");
                                        let busy = RwSignal::new(false);
                                        view! {
                                            <PrefRow
                                                label=s(&a, "label")
                                                detail="未安装。把官方发布的版本装进这台机器的 ~/.blazar，和本机版本一致。"
                                            >
                                                <button
                                                    type="button"
                                                    class="btn"
                                                    disabled=move || busy.get()
                                                    on:click=move |_| update(id.clone(), busy, "安装")
                                                >
                                                    {move || if busy.get() { "安装中…" } else { "安装" }}
                                                </button>
                                            </PrefRow>
                                        }
                                    })
                                    .collect_view()}
                                {rescan}
                            }
                                .into_any()
                        }
                    }}
                </PrefGroup>
                <PrefGroup title="维护" note="查找已不属于工作区的目录，确认后删除。">
                    <PrefRow label="残留目录" detail="磁盘上未被 Blazar 记录的 worktree 和私有目录。">
                        <button
                            type="button"
                            class="btn"
                            disabled=move || listing_left.get() || sweeping.get()
                            on:click=move |_| leftovers()
                        >
                            {move || if listing_left.get() { "查找中…" } else { "查找残留" }}
                        </button>
                    </PrefRow>
                    {move || match left.get() {
                        None if listing_left.get() => {
                            view! { <p class="pref-status">"正在查找残留目录…"</p> }.into_any()
                        }
                        None => ().into_any(),
                        Some(Err(e)) => {
                            view! {
                                <InlineError
                                    message=format!("查找失败：{e}")
                                    retry=Callback::new(move |()| leftovers())
                                />
                            }
                                .into_any()
                        }
                        Some(Ok(r)) => {
                            let orphans: Vec<Value> = r["orphans"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                            if orphans.is_empty() {
                                return view! {
                                    <p class="pref-status">
                                        {format!(
                                            "没有残留，已核对 {} 个目录。",
                                            r["owned_count"].as_u64().unwrap_or(0),
                                        )}
                                    </p>
                                }
                                    .into_any();
                            }
                            let summary = format!(
                                "{} 个目录，共 {}。默认保留未提交的改动。",
                                orphans.len(),
                                kb(r["orphan_kb"].as_f64().unwrap_or(0.0)),
                            );
                            view! {
                                <p class="pref-status">{summary}</p>
                                {orphans
                                    .into_iter()
                                    .map(|l| {
                                        let leaf = s(&l, "leaf");
                                        let (l2, l3) = (leaf.clone(), leaf.clone());
                                        let detail = [
                                            Some(s(&l, "branch")).filter(|x| !x.is_empty()),
                                            Some(kb(l["size_kb"].as_f64().unwrap_or(0.0))),
                                            Some(format!("闲置 {} 天", l["idle_days"].as_i64().unwrap_or(0))),
                                            Some(orphan_state(&l)),
                                        ]
                                            .into_iter()
                                            .flatten()
                                            .collect::<Vec<_>>()
                                            .join(" · ");
                                        view! {
                                            <PrefRow label=leaf.clone() detail=detail>
                                                <input
                                                    type="checkbox"
                                                    aria-label=format!("选择目录 {leaf}")
                                                    disabled=move || sweeping.get()
                                                    prop:checked=move || checked.with(|c| c.contains(&l2))
                                                    on:change=move |_| {
                                                        checked
                                                            .update(|c| {
                                                                if !c.remove(&l3) {
                                                                    c.insert(l3.clone());
                                                                }
                                                            })
                                                    }
                                                />
                                            </PrefRow>
                                        }
                                    })
                                    .collect_view()}
                                <PrefRow
                                    label="删除所选目录"
                                    detail="永久删除，无法撤销；有未提交改动的目录默认跳过。"
                                >
                                    <label class="chk">
                                        <input
                                            type="checkbox"
                                            disabled=move || sweeping.get()
                                            prop:checked=move || force.get()
                                            on:change=move |_| force.update(|f| *f = !*f)
                                        />
                                        "同时删除未提交的改动"
                                    </label>
                                    <button
                                        type="button"
                                        class="btn danger"
                                        disabled=move || sweeping.get()
                                        on:click=move |_| sweep()
                                    >
                                        {move || if sweeping.get() { "删除中…" } else { "删除所选目录" }}
                                    </button>
                                </PrefRow>
                            }
                                .into_any()
                        }
                    }}
                </PrefGroup>
            </div>
        </div>
    }
}
