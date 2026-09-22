use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use blazar_netmesh::config_import::ImportedConfig;
use blazar_netmesh::engine::{self, BundledEngine, Elevation, EngineLayout};
use blazar_netmesh::invite::{Invite, MAX_INVITE_BYTES};
use chrono::Utc;

fn staging_parent() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("找不到用户主目录")?;
    Ok(PathBuf::from(home).join(".blazar"))
}

fn fmt_time(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map_or_else(|| t.to_string(), |d| d.format("%Y-%m-%d").to_string())
}

fn confirm(yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    print!("继续？[y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

async fn join_with_config(file: &Path, engine_dir: Option<&Path>, yes: bool) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("读不到 {}", file.display()))?;
    let cfg = ImportedConfig::parse(&text)?;
    let bundle = BundledEngine::locate(engine_dir)
        .context("找不到组网引擎。用 --engine 指定含 easytier-core / easytier-cli 的目录")?;
    let s = &cfg.summary;
    println!("按 EasyTier 配置加入组网");
    println!("  网络      {}", s.network_name);
    println!(
        "  主机名    {}",
        s.hostname.as_deref().unwrap_or("（系统主机名）")
    );
    println!(
        "  地址      {}",
        s.ipv4
            .as_deref()
            .unwrap_or(if s.dhcp { "自动分配" } else { "无" })
    );
    println!("  对端      {}", s.peers.join(", "));
    if !s.listeners.is_empty() {
        println!("  监听      {}", s.listeners.join(", "));
    }
    if !s.features.is_empty() {
        println!("  功能      {}", s.features.join("；"));
    }
    for w in &s.warnings {
        println!("\n注意：{w}");
    }
    if !confirm(yes)? {
        println!("已取消");
        return Ok(());
    }
    let layout = EngineLayout::system();
    let st = engine::install(
        cfg.engine_config(),
        &s.network_name,
        &bundle,
        &layout,
        &staging_parent()?,
        Elevation::Terminal,
    )
    .await?;
    let ip = st
        .node
        .as_ref()
        .map_or("", |n| n.virtual_ipv4.as_str())
        .to_owned();
    println!("已加入 {}，本机地址 {ip}", s.network_name);
    Ok(())
}

pub async fn join(file: &Path, engine_dir: Option<&Path>, yes: bool) -> Result<()> {
    if file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
    {
        return join_with_config(file, engine_dir, yes).await;
    }
    let meta = std::fs::metadata(file).with_context(|| format!("读不到 {}", file.display()))?;
    if meta.len() > MAX_INVITE_BYTES as u64 {
        bail!("{} 不是 Blazar 邀请文件", file.display());
    }
    let text = std::fs::read_to_string(file)?;
    let inv = Invite::parse(&text, Utc::now().timestamp())?;
    let bundle = BundledEngine::locate(engine_dir).context(
        "找不到组网引擎。用 --engine 指定含 easytier-core / easytier-cli 的目录，\
         或在源码仓库里先运行 scripts/fetch-easytier.sh",
    )?;

    println!("加入团队组网");
    println!("  网络        {}", inv.network_name);
    println!("  本机主机名  {}", inv.hostname);
    println!(
        "  本机地址    {}",
        inv.ipv4.as_deref().unwrap_or("自动分配")
    );
    println!("  入网地址    {}", inv.peers.join(", "));
    println!("  有效期至    {}", fmt_time(inv.expires_at));
    if !inv.issued_by.is_empty() {
        println!("  签发人      {}", inv.issued_by);
    }
    let layout = EngineLayout::system();
    let st = engine::local_status(
        &layout,
        Some(&bundle),
        &engine::ExternalHints {
            rpc_candidates: vec!["127.0.0.1:15888".into()],
            ..Default::default()
        },
    )
    .await;
    if st.joined {
        println!("\n注意：本机已经在组网里，继续会换成这份邀请的身份。");
    }
    if let Some(ext) = &st.external {
        println!(
            "\n注意：本机已经通过 {} 在组网里（{}）。要改由 Blazar 管理，先退出它，否则两个实例会抢路由。",
            ext.label,
            ext.virtual_ipv4.as_deref().unwrap_or("地址未知")
        );
    }
    println!(
        "\n将以管理员身份把 EasyTier {} 装成系统服务（{}）。",
        engine::ENGINE_VERSION,
        layout.root.display()
    );
    if !confirm(yes)? {
        println!("已取消");
        return Ok(());
    }
    let st = engine::join(
        &inv,
        &bundle,
        &layout,
        &staging_parent()?,
        Elevation::Terminal,
    )
    .await?;
    let ip = st
        .node
        .as_ref()
        .map_or("", |n| n.virtual_ipv4.as_str())
        .to_owned();
    println!(
        "已加入 {}，本机地址 {ip}。打洞需要几秒，稍后用 `blazar mesh-status` 查看可见节点。",
        inv.network_name
    );
    Ok(())
}

pub async fn leave() -> Result<()> {
    let layout = EngineLayout::system();
    if !layout.joined() {
        println!("本机没有经 Blazar 加入组网");
        return Ok(());
    }
    engine::leave(&layout, &staging_parent()?, Elevation::Terminal).await?;
    println!("已退出组网，本机的组网凭据已删除");
    Ok(())
}

pub async fn status() -> Result<()> {
    let layout = EngineLayout::system();
    let st = engine::local_status(
        &layout,
        BundledEngine::locate(None).as_ref(),
        &engine::ExternalHints {
            rpc_candidates: vec!["127.0.0.1:15888".into()],
            ..Default::default()
        },
    )
    .await;
    println!("{}", serde_json::to_string_pretty(&st)?);
    Ok(())
}
