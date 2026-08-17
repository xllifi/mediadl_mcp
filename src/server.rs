//! The MCP tool surface: status, search, listing_info, prepare_download, confirm_download.

use std::collections::HashMap;
use std::path::PathBuf;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Serialize;

use crate::config::Config;
use crate::indexer::{self, Indexer};
use crate::qbittorrent::QBittorrentClient;
use crate::tokens::TokenStore;

#[derive(Clone)]
pub struct MediaDlServer {
    indexers: std::sync::Arc<HashMap<String, Box<dyn Indexer>>>,
    qbittorrent: QBittorrentClient,
    tokens: TokenStore,
}

impl MediaDlServer {
    pub async fn new(config_path: &PathBuf, tokens_path: PathBuf) -> anyhow::Result<Self> {
        let config = Config::load(config_path)?;
        let indexers = indexer::build_indexers(&config)?;
        let qbittorrent = QBittorrentClient::new(&config);
        let tokens = TokenStore::load(tokens_path).await?;
        Ok(Self {
            indexers: std::sync::Arc::new(indexers),
            qbittorrent,
            tokens,
        })
    }

    fn indexer(&self, id: &str) -> Result<&dyn Indexer, McpError> {
        self.indexers
            .get(id)
            .map(|i| i.as_ref())
            .ok_or_else(|| {
                let known: Vec<&str> = self.indexers.keys().map(String::as_str).collect();
                McpError::invalid_params(
                    format!("unknown indexer id {id:?}. Configured indexers: {}", known.join(", ")),
                    None,
                )
            })
    }

    /// Short catalog of configured indexers, embedded in the server instructions
    /// so the LLM knows which indexer ids exist.
    fn indexer_catalog(&self) -> String {
        let mut lines: Vec<String> = self
            .indexers
            .iter()
            .map(|(id, idx)| format!("- {id}: {}", idx.describe()))
            .collect();
        lines.sort();
        lines.join("\n")
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// Id of the indexer to search (see the server instructions for the list of configured ids).
    pub indexer_id: String,
    /// Free-text search query (movie / TV show / anime title).
    pub query: String,
    /// 1-based page number. Defaults to 1. Use `next_page` from a previous response to page.
    #[serde(default = "default_page")]
    pub page: u32,
}

fn default_page() -> u32 {
    1
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListingParams {
    /// Id of the indexer the listing belongs to.
    pub indexer_id: String,
    /// Listing id, as returned in `search` results.
    pub listing_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ConfirmParams {
    /// Confirmation token returned by `prepare_download`.
    pub token: String,
}

#[derive(Serialize, schemars::JsonSchema)]
struct StatusResponse {
    server_name: &'static str,
    server_version: &'static str,
    qbittorrent: QbittorrentStatus,
    indexers: Vec<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
enum QbittorrentStatus {
    Connected { version: String },
    Unreachable { error: String },
}

#[derive(Serialize, schemars::JsonSchema)]
struct PrepareResponse {
    confirmation_token: String,
    expires_at: String,
    ttl: &'static str,
    indexer_id: String,
    listing_id: String,
    title: String,
    source: &'static str,
    note: &'static str,
}

#[tool_router]
impl MediaDlServer {
    /// Server version and qBittorrent connection status.
    #[tool(description = "Get the mediadl-mcp server version and qBittorrent connection status.")]
    async fn status(&self) -> Result<CallToolResult, McpError> {
        let qb = match self.qbittorrent.version().await {
            Ok(version) => QbittorrentStatus::Connected { version },
            Err(e) => QbittorrentStatus::Unreachable {
                error: format!("{e:#}"),
            },
        };
        let mut indexers: Vec<String> = self.indexers.keys().cloned().collect();
        indexers.sort();
        structured(&StatusResponse {
            server_name: "mediadl-mcp",
            server_version: env!("CARGO_PKG_VERSION"),
            qbittorrent: qb,
            indexers,
        })
    }

    /// Search an indexer for movies / TV shows / anime.
    #[tool(description = "Search movies, TV shows or anime on a torrent indexer. Returns one page of results; pass `next_page` back as `page` to keep paging.")]
    async fn search(&self, Parameters(p): Parameters<SearchParams>) -> Result<CallToolResult, McpError> {
        let indexer = self.indexer(&p.indexer_id)?;
        match indexer.search(&p.query, p.page).await {
            Ok(results) => structured(&results),
            Err(e) => Ok(tool_error(format!("{e:#}"))),
        }
    }

    /// Fetch the raw HTML page of a listing.
    #[tool(description = "Get the raw HTML page of a listing on an indexer (full description, comments, file list, etc.).")]
    async fn listing_info(&self, Parameters(p): Parameters<ListingParams>) -> Result<CallToolResult, McpError> {
        let indexer = self.indexer(&p.indexer_id)?;
        match indexer.listing_info(&p.listing_id).await {
            Ok(html) => Ok(CallToolResult::success(vec![ContentBlock::text(html)])),
            Err(e) => Ok(tool_error(format!("{e:#}"))),
        }
    }

    /// Resolve a listing and create a confirmation token (nothing is downloaded yet).
    #[tool(description = "Prepare a download: resolves the listing to a magnet link / torrent file and returns a confirmation token valid for 24 hours. Nothing is sent to qBittorrent until `confirm_download` is called with the token.")]
    async fn prepare_download(&self, Parameters(p): Parameters<ListingParams>) -> Result<CallToolResult, McpError> {
        let indexer = self.indexer(&p.indexer_id)?;
        let data = match indexer.prepare_download(&p.listing_id).await {
            Ok(d) => d,
            Err(e) => return Ok(tool_error(format!("{e:#}"))),
        };
        let source = match &data.source {
            crate::tokens::PendingSource::Magnet { .. } => "magnet",
            crate::tokens::PendingSource::TorrentFile { .. } => "torrent_file",
        };
        let pending = match self
            .tokens
            .create(
                p.indexer_id.clone(),
                p.listing_id.clone(),
                data.title,
                data.source,
                data.save_path,
            )
            .await
        {
            Ok(p) => p,
            Err(e) => return Ok(tool_error(format!("failed to persist confirmation token: {e:#}"))),
        };
        structured(&PrepareResponse {
            confirmation_token: pending.token,
            expires_at: pending.expires_at.to_rfc3339(),
            ttl: "24h",
            indexer_id: pending.indexer_id,
            listing_id: pending.listing_id,
            title: pending.title,
            source,
            note: "Call confirm_download with this token to actually add the torrent to qBittorrent. The token is single-use and expires at the given timestamp.",
        })
    }

    /// Confirm a previously prepared download; adds the torrent to qBittorrent.
    #[tool(description = "Confirm a prepared download using its confirmation token. Adds the torrent to qBittorrent. The token is single-use; expired or unknown tokens return an error.")]
    async fn confirm_download(&self, Parameters(p): Parameters<ConfirmParams>) -> Result<CallToolResult, McpError> {
        let pending = match self.tokens.consume(&p.token).await {
            Ok(p) => p,
            Err(e) => return Ok(tool_error(format!("{e:#}"))),
        };
        let source = match pending.source.to_torrent_source() {
            Ok(s) => s,
            Err(e) => return Ok(tool_error(format!("stored download data is corrupt: {e:#}"))),
        };
        match self
            .qbittorrent
            .add_torrent(source, pending.save_path.as_deref())
            .await
        {
            Ok(()) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "Added \"{}\" ({}:{}) to qBittorrent.",
                pending.title, pending.indexer_id, pending.listing_id
            ))])),
            Err(e) => Ok(tool_error(format!("{e:#}"))),
        }
    }
}

#[tool_handler]
impl ServerHandler for MediaDlServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        let mut info = rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "mediadl-mcp",
            env!("CARGO_PKG_VERSION"),
        ));
        info.instructions = Some(format!(
            "Search torrent indexers for movies / TV shows / anime and download them via qBittorrent.\n\
             Workflow: search -> listing_info (optional, raw HTML) -> prepare_download -> confirm_download.\n\
             Downloads ALWAYS require the two-step confirmation: prepare_download returns a token valid for 24h, \
             confirm_download consumes it. Tokens survive server restarts.\n\
             Configured indexers:\n{}",
            self.indexer_catalog()
        ));
        info
    }
}

/// Serialize a value as the tool's structured JSON content.
fn structured<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Pass an operational error back to the LLM as tool content (isError=true).
fn tool_error(message: String) -> CallToolResult {
    tracing::warn!("tool error: {message}");
    CallToolResult::error(vec![ContentBlock::text(format!("error: {message}"))])
}
