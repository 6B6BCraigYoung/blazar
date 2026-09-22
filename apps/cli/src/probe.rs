use std::sync::Arc;

use anyhow::Result;
use blazar_netmesh::{CliInvocation, EasyTierMesh};
use blazar_transport::{LocalTransport, NodeTransport, SshTransport};
use blazar_vfs::Vfs;

pub fn transport_for(host: &str) -> Arc<dyn NodeTransport> {
    if host == "local" {
        Arc::new(LocalTransport)
    } else {
        Arc::new(SshTransport::new(host))
    }
}

pub async fn mesh(via: &str, container: Option<&str>) -> Result<()> {
    let invocation = match container {
        Some(c) => CliInvocation::docker(c),
        None => CliInvocation::direct(),
    };
    let mesh = EasyTierMesh::new(transport_for(via), invocation);
    let peers = mesh.peers().await?;

    println!(
        "{:<20} {:<16} {:<8} {:>9} {:>7}  {:<16} 版本",
        "HOSTNAME", "IPV4", "连接", "延迟ms", "丢包", "NAT"
    );
    for p in &peers {
        println!(
            "{:<20} {:<16} {:<8} {:>9} {:>7}  {:<16} {}{}",
            p.hostname,
            p.ipv4,
            p.cost,
            p.latency_ms.map_or("-".into(), |v| format!("{v:.1}")),
            p.loss_rate
                .map_or("-".into(), |v| format!("{:.0}%", v * 100.0)),
            p.nat_type,
            p.version,
            if p.is_self() {
                "  ← 本节点"
            } else if !p.is_healthy() {
                "  ⚠ 链路不健康"
            } else {
                ""
            }
        );
    }
    println!("\n共 {} 个节点，其中可派活 {} 个", peers.len(), {
        peers
            .iter()
            .filter(|p| !p.is_self() && p.is_healthy())
            .count()
    });
    Ok(())
}

pub async fn health(host: &str) -> Result<()> {
    let t = transport_for(host);
    let h = t.health().await?;
    println!("主机     {host}");
    println!("系统     {} {}", h.os, h.arch);
    println!(
        "容量     {} 核 / {} GB 内存 / 根分区剩 {} GB / load {:.2}",
        h.cpus, h.mem_gb, h.disk_free_gb, h.load1
    );
    if h.gpus.is_empty() {
        println!("GPU      无");
    } else {
        for g in &h.gpus {
            println!("GPU      {} ({} MB)", g.name, g.vram_mb);
        }
    }
    print!("出口     ");
    let mut blocked = Vec::new();
    for (name, code) in &h.egress {
        let ok = matches!(code, 200..=299 | 401);
        print!("{name}:{code}{} ", if ok { "✓" } else { "✗" });
        if !ok {
            blocked.push(name.clone());
        }
    }
    println!();
    if !blocked.is_empty() {
        println!(
            "\n⚠ 无法直连: {} —— 该节点上的 agent 必须经出口代理",
            blocked.join(", ")
        );
    }
    Ok(())
}

pub async fn tree(host: &str, root: &str, limit: usize) -> Result<()> {
    let vfs = Vfs::new(transport_for(host), root);
    let entries = vfs.tree(None).await?;
    let files = entries.iter().filter(|e| !e.is_dir).count();
    let dirs = entries.len() - files;
    let changed = entries.iter().filter(|e| e.change.is_some()).count();

    println!("{host}:{root}\n{files} 个文件 / {dirs} 个目录 / {changed} 处改动\n");
    for e in entries.iter().filter(|e| !e.is_dir).take(limit) {
        let mark = match e.change {
            Some(blazar_vfs::ChangeKind::Modified) => "M ",
            Some(blazar_vfs::ChangeKind::Added) => "A ",
            Some(blazar_vfs::ChangeKind::Deleted) => "D ",
            Some(blazar_vfs::ChangeKind::Untracked) => "? ",
            None => "  ",
        };
        println!("{mark}{}", e.path);
    }
    if files > limit {
        println!("… 其余 {} 个文件省略", files - limit);
    }
    Ok(())
}

pub async fn search(host: &str, root: &str, query: &str, limit: usize) -> Result<()> {
    let vfs = Vfs::new(transport_for(host), root);
    let hits = vfs.search(query, limit).await?;
    println!("{host}:{root} 中搜索 {query:?} —— {} 条命中\n", hits.len());
    for h in &hits {
        println!("{}:{}  {}", h.path, h.line, h.text.trim());
    }
    Ok(())
}

pub async fn worktree(host: &str, repo: &str, action: &str, name: &str) -> Result<()> {
    use blazar_worktree::WorktreeManager;
    let m = WorktreeManager::new(transport_for(host), repo);
    match action {
        "create" => {
            let env = m.create(name).await?;
            println!("worktree  {}", env.worktree);
            println!("分支      {}", env.branch);
            println!("基线      {}", env.base_commit);
            println!("私有 tmp  {}", env.tmpdir);
            println!("私有配置  {}", env.config_root);
            println!("\n环境变量:");
            for (k, v) in env.env_vars() {
                println!("  {k}={v}");
            }
        }
        "branches" => {
            for b in m.branches().await? {
                println!("{b}");
            }
        }
        "gc" => {
            let n = m.gc(name.parse().unwrap_or(7)).await?;
            println!("回收了 {n} 个陈旧目录");
        }
        other => anyhow::bail!("未知动作 {other}（可用：create / branches / gc）"),
    }
    Ok(())
}

pub async fn agents(host: &str) -> Result<()> {
    let t = transport_for(host);
    let found = blazar_runtime::discover(&t).await?;
    println!("{host} 上的 agent CLI\n");
    println!(
        "{:<10} {:<10} {:<8} {:<40} 说明",
        "AGENT", "版本", "登录", "路径"
    );
    for a in &found {
        let auth = match a.authed {
            Some(true) => "✓",
            Some(false) => "✗",
            None if a.path.is_some() => "?",
            None => "-",
        };
        println!(
            "{:<10} {:<10} {:<8} {:<40} {}",
            a.label,
            a.version.as_deref().unwrap_or("-"),
            auth,
            a.path.as_deref().unwrap_or("未安装"),
            a.auth_hint.as_deref().unwrap_or(""),
        );
    }
    let usable = found.iter().filter(|a| a.is_usable()).count();
    println!("\n可用 {usable} / 共 {} 个", found.len());
    Ok(())
}
