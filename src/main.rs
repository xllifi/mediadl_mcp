//! Binary entrypoint: parse args, load config, serve MCP over HTTP (streamable).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

use mediadl_mcp::auth;
use mediadl_mcp::config::Config;
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
    /// Override the bind address from the config (http.listen).
    #[arg(long)]
    listen: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let config = Config::load(&args.config)?;

    let server = MediaDlServer::new(&config, args.tokens).await?;

    let ct = CancellationToken::new();

    // Host-header (DNS-rebinding) handling. Default keeps loopback-only; setting
    // http.allowed_hosts lets a public reverse-proxied hostname through.
    let mut http_config = StreamableHttpServerConfig::default()
        .with_cancellation_token(ct.child_token())
        .with_json_response(true);
    match config.http.resolved_allowed_hosts() {
        Some(hosts) if hosts.is_empty() => {
            tracing::warn!("http.allowed_hosts allows any Host header");
            http_config = http_config.disable_allowed_hosts();
        }
        Some(hosts) => {
            tracing::info!("allowed Host headers: {}", hosts.join(", "));
            http_config = http_config.with_allowed_hosts(hosts);
        }
        None => {}
    }

    let mcp_service: StreamableHttpService<MediaDlServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            LocalSessionManager::default().into(),
            http_config,
        );

    // The MCP endpoint. Token auth (constant-time) is layered on when a token is
    // configured; /health stays open so orchestrators can probe it.
    let mcp_router = if config.http.auth_enabled() {
        let token = config.http.token.clone().unwrap();
        tracing::info!("HTTP token auth enabled (Bearer header or ?api_key= query param)");
        axum::Router::new()
            .nest_service("/mcp", mcp_service)
            .layer(axum::middleware::from_fn_with_state(
                token,
                auth::token_auth,
            ))
    } else {
        tracing::warn!("http auth disabled (allow_insecure_no_auth=true) — the endpoint can trigger downloads!");
        axum::Router::new().nest_service("/mcp", mcp_service)
    };

    let app = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "OK" }))
        .merge(mcp_router);

    let listen = args.listen.unwrap_or_else(|| config.http.listen.clone());
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("failed to bind {listen}"))?;
    tracing::info!("mediadl-mcp listening on http://{listen}/mcp");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
            ct.cancel();
        })
        .await?;

    Ok(())
}
