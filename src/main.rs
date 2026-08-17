//! Binary entrypoint: parse args, load config, serve MCP over stdio.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};

use mediadl_mcp::server::MediaDlServer;

#[derive(Parser)]
#[command(about = "MCP server for searching torrent indexers and downloading via qBittorrent")]
struct Args {
    /// Path to the JSON config file.
    #[arg(short, long, default_value = "config.json")]
    config: PathBuf,
    /// Path of the confirmation token store.
    #[arg(long, default_value = "tokens.json")]
    tokens: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    // MCP runs over stdio, so logs must go to stderr.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let server = MediaDlServer::new(&args.config, args.tokens).await?;

    tracing::info!("mediadl-mcp serving on stdio");
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
