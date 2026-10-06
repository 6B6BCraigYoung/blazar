use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "blazar-mcp", version, about = "Blazar 的 MCP server（stdio）")]
struct Cli {
    #[arg(long, env = "BLAZAR_HUB", default_value = "auto")]
    hub: String,

    #[arg(long, requires = "remote_root")]
    remote_node: Option<String>,

    #[arg(long, requires = "remote_node")]
    remote_root: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "blazar=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    if let (Some(node), Some(root)) = (cli.remote_node.clone(), cli.remote_root.clone()) {
        return blazar_mcp::remote::serve(blazar_mcp::remote::RemoteTarget { node, root }).await;
    }
    blazar_mcp::fleet::serve(&cli.hub).await
}
