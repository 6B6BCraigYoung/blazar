use super::*;

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
    let kb = |n: f64| {
        if n > 1_048_576.0 {
            format!("{:.1} GB", n / 1_048_576.0)
        } else if n > 1024.0 {
            format!("{:.0} MB", n / 1024.0)
        } else {
            format!("{n} KB")
        }
    };
    let ws_here = move || {
        app.workspaces()
            .into_iter()
            .filter(|w| w.node == nm.get_value())
            .collect::<Vec<_>>()
    };

    view! {
        <div class="page nodes-page">
            <div class="page-head">
                <a href="/nodes" class="crumb">
                    "机器与组网"
                </a>
                <span class="sep">"/"</span>
                <h1>{if name == "local" { "本机".to_owned() } else { name.clone() }}</h1>
                {move || {
                    let on = node.get().is_some_and(|n| n.status == "online");
                    view! {
                        <span class="state-pill" data-act=if on { "completed" } else { "idle" }>
                            {if on { "在线" } else { "离线" }}
                        </span>
                    }
                }}
                <span class="grow"></span>
                <button
                    class="btn primary"
                    on:click=move |_| {
                        app.new_ws_node.set(Some(nm.get_value()));
                        app.new_ws.set(true);
                    }
                >
                    "新建工作区"
                </button>
            </div>
            <section class="card pad">
                <h3>"工作区"</h3>
                {move || {
                    let list = ws_here();
                    if list.is_empty() {
                        return view! {
                            <EmptyState
                                title="还没有工作区"
                                detail="在这台机器上创建第一个工作区。"
                            />
                        }
                            .into_any();
                    }
                    view! {
                        <div class="nodes-table">
                            <table class="tb">
                                <tbody>
                                    {list
                                        .into_iter()
                                        .map(|w| {
                                            view! {
                                                <tr>
                                                    <td>
                                                        <span class="dot" data-act=w.activity.clone()></span>
                                                        " "
                                                        <a href=format!(
                                                            "/w/{}",
                                                            w.id,
                                                        )>{crate::fmt::ws_title(&w.name, &w.node)}</a>
                                                        {w
                                                            .parent
                                                            .as_deref()
                                                            .and_then(|p| app.workspace(p))
                                                            .map(|p| {
                                                                view! {
                                                                    <span class="muted small">
                                                                        {format!(
                                                                            " · 属于「{}」",
                                                                            crate::fmt::ws_title(&p.name, &p.node),
                                                                        )}
                                                                    </span>
                                                                }
                                                            })}
                                                    </td>
                                                    <td>
                                                        <details class="nodes-details">
                                                            <summary>"目录"</summary>
                                                            <div class="mono small">{w.path.clone()}</div>
                                                        </details>
                                                    </td>
                                                    <td class="muted" style="text-align:right">
                                                        {activity_label(&w.activity).to_owned()}
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
            </section>
            <section class="card pad">
                <div class="card-title">
                    <h3>"机器状态"</h3>
                    <span class="grow"></span>
                    <button
                        class="btn small"
                        disabled=move || probing.get()
                        on:click=move |_| probe()
                    >
                        {move || if probing.get() { "检查中…" } else { "检查机器" }}
                    </button>
                </div>
                {move || match hw.get() {
                    None if probing.get() => {
                        view! { <LoadingState text="正在检查机器…" class="muted small" /> }
                            .into_any()
                    }
                    None => {
                        view! {
                            <EmptyState
                                title="尚未检查"
                                detail="检查硬件资源和 AI 服务连接。"
                                class="muted small"
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
                        view! {
                            <div class="kpis">
                                <div>
                                    <span>"CPU"</span>
                                    <b>{c["cpus"].to_string()}</b>
                                </div>
                                <div>
                                    <span>"内存"</span>
                                    <b>{format!("{}G", c["mem_gb"])}</b>
                                </div>
                                <div>
                                    <span>"可用磁盘"</span>
                                    <b>{format!("{}G", c["disk_free_gb"])}</b>
                                </div>
                                <div>
                                    <span>"GPU"</span>
                                    <b>{gpus.len()}</b>
                                </div>
                            </div>
                            <details class="nodes-details">
                                <summary>"硬件与连接详情"</summary>
                                <div class="kv">
                                    <span class="k">"系统"</span>
                                    <span class="v mono">
                                        {format!("{} {}", s(&c, "os"), s(&c, "arch"))}
                                    </span>
                                </div>
                                <div class="kv">
                                    <span class="k">"负载"</span>
                                    <span class="v mono">{c["load1"].to_string()}</span>
                                </div>
                                {(!uniq.is_empty())
                                    .then(|| {
                                        view! { <div class="muted small">{uniq.join(" / ")}</div> }
                                    })}
                                <div class="row-actions">
                                    {eg
                                        .into_iter()
                                        .map(|(k, v)| {
                                            let ok = (200..300).contains(&v) || v == 401;
                                            view! {
                                                <span class=if ok {
                                                    "gchip ok"
                                                } else {
                                                    "gchip bad"
                                                }>{format!("{k} {v}")}</span>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            </details>
                            {(c["has_ai_egress"].as_bool() != Some(true))
                                .then(|| {
                                    view! {
                                        <div class="warn-tx small">
                                            "无法连接 AI 服务，请在「运行时 → 环境变量」中配置代理。"
                                        </div>
                                    }
                                })}
                        }
                            .into_any()
                    }
                }}
            </section>
            <section class="card pad">
                <div class="card-title">
                    <h3>"运行时"</h3>
                    <span class="grow"></span>
                    <button
                        class="btn small"
                        disabled=move || scanning.get()
                        on:click=move |_| scan()
                    >
                        {move || if scanning.get() { "读取中…" } else { "检查运行时" }}
                    </button>
                </div>
                {move || match agents.get() {
                    None if scanning.get() => {
                        view! {
                            <LoadingState text="正在读取运行时…" class="muted small" />
                        }
                            .into_any()
                    }
                    None => {
                        view! {
                            <EmptyState
                                title="尚未检查"
                                detail="读取已安装的运行时及登录状态。"
                                class="muted small"
                            />
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
                        if inst.is_empty() && missing.is_empty() {
                            return view! {
                                <EmptyState
                                    title="未发现运行时"
                                    detail="在这台机器安装 Claude Code 或 Codex 后，重新检查。"
                                    class="muted small"
                                />
                            }
                                .into_any();
                        }
                        view! {
                            <div class="nodes-table">
                                <table class="tb">
                                    <thead>
                                        <tr>
                                            <th>"运行时"</th>
                                            <th>"版本与路径"</th>
                                            <th>"登录状态"</th>
                                            <th>"操作"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {inst
                                            .into_iter()
                                            .map(|a| {
                                                let id = s(&a, "id");
                                                let v = vers[&id].clone();
                                                let behind = v["behind"].as_bool() == Some(true);
                                                let local_v = s(&v, "local");
                                                let busy = RwSignal::new(false);
                                                let (id2, n2, n3) = (id.clone(), n.clone(), n.clone());
                                                let authed = a["authed"].as_bool();
                                                view! {
                                                    <tr>
                                                        <td>{s(&a, "label")}</td>
                                                        <td>
                                                            <details class="nodes-details">
                                                                <summary>"查看详情"</summary>
                                                                <div class="kv">
                                                                    <span class="k">"版本"</span>
                                                                    <span class="v mono">
                                                                        {Some(s(&a, "version"))
                                                                            .filter(|x| !x.is_empty())
                                                                            .unwrap_or_else(|| "—".into())}
                                                                    </span>
                                                                </div>
                                                                <div class="kv">
                                                                    <span class="k">"路径"</span>
                                                                    <span class="v mono">{s(&a, "path")}</span>
                                                                </div>
                                                                <div class="kv">
                                                                    <span class="k">"本机版本"</span>
                                                                    <span class="v mono">
                                                                        {if local_v.is_empty() {
                                                                            "—".to_owned()
                                                                        } else {
                                                                            local_v.clone()
                                                                        }}
                                                                    </span>
                                                                </div>
                                                                <div class="muted small">{s(&a, "auth_hint")}</div>
                                                            </details>
                                                            {behind
                                                                .then(|| {
                                                                    view! {
                                                                        " "
                                                                        <span
                                                                            class="gchip warn"
                                                                            title=format!("本机是 {local_v}")
                                                                        >
                                                                            "可更新"
                                                                        </span>
                                                                        " "
                                                                        <button
                                                                            class="btn small"
                                                                            style="min-width:72px"
                                                                            disabled=move || busy.get()
                                                                            on:click=move |_| {
                                                                                if busy.get_untracked() {
                                                                                    return;
                                                                                }
                                                                                busy.set(true);
                                                                                let (id, n) = (id2.clone(), n2.clone());
                                                                                spawn_local(async move {
                                                                                    match api::send::<
                                                                                        Value,
                                                                                    >(
                                                                                            "POST",
                                                                                            &format!("/api/nodes/{}/update/{id}", api::enc(&n)),
                                                                                            &json!({}),
                                                                                        )
                                                                                        .await
                                                                                    {
                                                                                        Ok(r) => toast(s(&r, "message")),
                                                                                        Err(e) => toast(format!("更新失败：{e}")),
                                                                                    }
                                                                                    busy.try_set(false);
                                                                                    scan();
                                                                                });
                                                                            }
                                                                        >
                                                                            {move || {
                                                                                if busy.get() {
                                                                                    "更新中…".to_owned()
                                                                                } else {
                                                                                    "更新".to_owned()
                                                                                }
                                                                            }}
                                                                        </button>
                                                                    }
                                                                })}
                                                        </td>
                                                        <td>
                                                            {match authed {
                                                                Some(true) => {
                                                                    view! {
                                                                        <span class="state-pill" data-act="completed">
                                                                            "已登录"
                                                                        </span>
                                                                    }
                                                                        .into_any()
                                                                }
                                                                Some(false) => {
                                                                    view! {
                                                                        <span class="state-pill" data-act="idle">
                                                                            "未登录"
                                                                        </span>
                                                                    }
                                                                        .into_any()
                                                                }
                                                                None => {
                                                                    view! {
                                                                        <span class="state-pill" data-act="awaiting_approval">
                                                                            "待核对"
                                                                        </span>
                                                                    }
                                                                        .into_any()
                                                                }
                                                            }}
                                                        </td>
                                                        <td style="text-align:right">
                                                            {(id == "codex" && n3 != "local")
                                                                .then(|| {
                                                                    view! {
                                                                        <button
                                                                            class="btn small ghost"
                                                                            on:click=move |_| crate::pages::runtimes::node_login(
                                                                                n3.clone(),
                                                                                Callback::new(move |_| scan()),
                                                                            )
                                                                        >
                                                                            {if authed == Some(true) {
                                                                                "重新登录"
                                                                            } else {
                                                                                "登录"
                                                                            }}
                                                                        </button>
                                                                    }
                                                                })}
                                                        </td>
                                                    </tr>
                                                }
                                            })
                                            .collect_view()}
                                        {missing
                                            .into_iter()
                                            .map(|a| {
                                                let id = s(&a, "id");
                                                let n = nm.get_value();
                                                let busy = RwSignal::new(false);
                                                view! {
                                                    <tr>
                                                        <td>{s(&a, "label")}</td>
                                                        <td class="muted">"未安装"</td>
                                                        <td></td>
                                                        <td style="text-align:right">
                                                            <button
                                                                class="btn small"
                                                                style="min-width:72px"
                                                                title="把官方发布的版本装进这台机器的 ~/.blazar，和本机版本一致"
                                                                disabled=move || busy.get()
                                                                on:click=move |_| {
                                                                    if busy.get_untracked() {
                                                                        return;
                                                                    }
                                                                    busy.set(true);
                                                                    let (id, n) = (id.clone(), n.clone());
                                                                    spawn_local(async move {
                                                                        match api::send::<
                                                                            Value,
                                                                        >(
                                                                                "POST",
                                                                                &format!("/api/nodes/{}/update/{id}", api::enc(&n)),
                                                                                &json!({}),
                                                                            )
                                                                            .await
                                                                        {
                                                                            Ok(r) => toast(s(&r, "message")),
                                                                            Err(e) => toast(format!("安装失败：{e}")),
                                                                        }
                                                                        busy.try_set(false);
                                                                        scan();
                                                                    });
                                                                }
                                                            >
                                                                {move || if busy.get() { "安装中…" } else { "安装" }}
                                                            </button>
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
                    }
                }}
            </section>
            <details class="card pad nodes-maintenance">
                <summary>"清理残留目录"</summary>
                <p class="muted small">
                    "查找已不属于工作区的目录，确认后删除。"
                </p>
                <div class="card-title">
                    <h3>"残留目录"</h3>
                    <span class="grow"></span>
                    <button
                        class="btn small"
                        disabled=move || listing_left.get() || sweeping.get()
                        on:click=move |_| leftovers()
                    >
                        {move || if listing_left.get() { "查找中…" } else { "查找残留" }}
                    </button>
                </div>
                {move || match left.get() {
                    None if listing_left.get() => {
                        view! {
                            <LoadingState text="正在查找残留目录…" class="muted small" />
                        }
                            .into_any()
                    }
                    None => {
                        view! {
                            <EmptyState
                                title="尚未检查"
                                detail="查找磁盘上未被 Blazar 记录的 worktree 和私有目录。"
                                class="muted small"
                            />
                        }
                            .into_any()
                    }
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
                                <div class="muted small">
                                    {format!(
                                        "没有残留，已核对 {} 个目录。",
                                        r["owned_count"].as_u64().unwrap_or(0),
                                    )}
                                </div>
                            }
                                .into_any();
                        }
                        view! {
                            <div class="muted small">
                                {format!(
                                    "{} 个目录，共 {}。默认保留未提交的改动。",
                                    orphans.len(),
                                    kb(r["orphan_kb"].as_f64().unwrap_or(0.0)),
                                )}
                            </div>
                            <div class="nodes-table">
                                <table class="tb">
                                    <thead>
                                        <tr>
                                            <th></th>
                                            <th>"名字"</th>
                                            <th>"分支"</th>
                                            <th>"大小"</th>
                                            <th>"闲置"</th>
                                            <th>"状态"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {orphans
                                            .into_iter()
                                            .map(|l| {
                                                let leaf = s(&l, "leaf");
                                                let l2 = leaf.clone();
                                                let dirty = l["dirty"].as_u64().unwrap_or(0);
                                                view! {
                                                    <tr>
                                                        <td>
                                                            <input
                                                                type="checkbox"
                                                                aria-label=format!("选择目录 {leaf}")
                                                                disabled=move || sweeping.get()
                                                                prop:checked=move || checked.with(|c| c.contains(&l2))
                                                                on:change={
                                                                    let leaf = leaf.clone();
                                                                    move |_| {
                                                                        checked
                                                                            .update(|c| {
                                                                                if !c.remove(&leaf) {
                                                                                    c.insert(leaf.clone());
                                                                                }
                                                                            })
                                                                    }
                                                                }
                                                            />
                                                        </td>
                                                        <td class="mono" title=s(&l, "worktree")>
                                                            {leaf.clone()}
                                                        </td>
                                                        <td class="mono">
                                                            {Some(s(&l, "branch"))
                                                                .filter(|x| !x.is_empty())
                                                                .unwrap_or_else(|| "—".into())}
                                                        </td>
                                                        <td class="mono">
                                                            {kb(l["size_kb"].as_f64().unwrap_or(0.0))}
                                                        </td>
                                                        <td class="mono">
                                                            {format!("{} 天", l["idle_days"].as_i64().unwrap_or(0))}
                                                        </td>
                                                        <td>
                                                            {if dirty > 0 {
                                                                view! {
                                                                    <span class="gchip warn">
                                                                        {format!("{dirty} 处未提交")}
                                                                    </span>
                                                                }
                                                                    .into_any()
                                                            } else if l["broken"].as_bool() == Some(true) {
                                                                view! { <span class="gchip">"目录已损坏"</span> }
                                                                    .into_any()
                                                            } else if !l["worktree"].is_string() {
                                                                view! { <span class="gchip">"只剩私有目录"</span> }
                                                                    .into_any()
                                                            } else {
                                                                view! { <span class="gchip">"干净"</span> }.into_any()
                                                            }}
                                                        </td>
                                                    </tr>
                                                }
                                            })
                                            .collect_view()}
                                    </tbody>
                                </table>
                            </div>
                            <div class="row-actions">
                                <button
                                    class="btn small danger"
                                    disabled=move || sweeping.get()
                                    on:click=move |_| {
                                        if sweeping.get_untracked() {
                                            return;
                                        }
                                        let leaves: Vec<String> = checked
                                            .get_untracked()
                                            .into_iter()
                                            .collect();
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
                                                    vec![
                                                        Choice::plain("取消"),
                                                        Choice::danger("删除目录"),
                                                    ],
                                                )
                                                .await != Some(1)
                                            {
                                                return;
                                            }
                                            if sweeping.try_get_untracked().unwrap_or(true) {
                                                return;
                                            }
                                            sweeping.set(true);
                                            match api::send::<
                                                Value,
                                            >(
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
                                                    toast(
                                                        if skipped.is_empty() {
                                                            format!("清掉 {removed} 份")
                                                        } else {
                                                            format!(
                                                                "清掉 {removed} 份；跳过 {} 份：{}",
                                                                skipped.len(),
                                                                skipped.join("；"),
                                                            )
                                                        },
                                                    );
                                                    leftovers();
                                                }
                                                Err(e) => toast(format!("清扫失败：{e}")),
                                            }
                                            sweeping.try_set(false);
                                        });
                                    }
                                >
                                    {move || {
                                        if sweeping.get() {
                                            "删除中…"
                                        } else {
                                            "删除所选目录"
                                        }
                                    }}
                                </button>
                                <label class="chk">
                                    <input
                                        type="checkbox"
                                        disabled=move || sweeping.get()
                                        prop:checked=move || force.get()
                                        on:change=move |_| force.update(|f| *f = !*f)
                                    />
                                    "同时删除未提交的改动"
                                </label>
                            </div>
                        }
                            .into_any()
                    }
                }}
            </details>
        </div>
    }
}
