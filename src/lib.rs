//! mediadl-mcp: an MCP server that searches torrent indexers and hands
//! downloads to qBittorrent behind a two-step confirmation flow.

pub mod config;
pub mod indexer;
pub mod qbittorrent;
pub mod server;
pub mod tokens;
