use anyhow::Result;
use blazar_hub::HubConfig;
use clap::Parser;

#[derive(Parser)]
#[command(name = "blazar-hub", version, about = "Blazar 中心服务")]
struct Cli {
    #[arg(long, env = "BLAZAR_DB")]
    db: Option<std::path::PathBuf>,

    #[arg(long, env = "BLAZAR_BIND", default_value = "127.0.0.1:7777")]
    bind: std::net::SocketAddr,

    #[arg(long, env = "BLAZAR_MESH_VIA", default_value = "local")]
    mesh_via: String,

    #[arg(long, env = "BLAZAR_MESH_CONTAINER")]
    mesh_container: Option<String>,

    #[arg(long, env = "BLAZAR_ENGINE_DIR")]
    engine_dir: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some(blazar_hub::office::MCP_SUBCOMMAND) {
        return blazar_hub::office::serve_mcp(&argv).await;
    }
    if argv.get(1).map(String::as_str) == Some(blazar_mcp::remote::SUBCOMMAND) {
        let target = blazar_mcp::remote::target_from_args(&argv).ok_or_else(|| {
            anyhow::anyhow!(
                "用法: {} --node <主机> --root <路径>",
                blazar_mcp::remote::SUBCOMMAND
            )
        })?;
        return blazar_mcp::remote::serve(target).await;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blazar=info,tower_http=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let db_path = match cli.db {
        Some(p) => p,
        None => blazar_hub::default_db_path()?,
    };
    let cfg = HubConfig {
        db_path,
        bind: cli.bind,
        mesh_via: cli.mesh_via,
        mesh_container: cli.mesh_container,
        engine_dir: cli.engine_dir,
    };

    let st = blazar_hub::build_state(&cfg).await?;
    let (addr, listener) = blazar_hub::bind(&cfg).await?;
    println!("Blazar hub 已启动  →  http://{addr}");
    println!("mesh 发现经由: {}", cfg.mesh_via);
    blazar_hub::serve(listener, blazar_hub::build_router(st)).await
}
