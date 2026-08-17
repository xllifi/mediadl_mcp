//! Indexer abstraction plus hardcoded indexer implementations.

use std::collections::HashMap;

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::Serialize;

use crate::config::{Config, IndexerConfig};
use crate::tokens::PendingSource;

pub mod kinozal;
pub mod nyaa;

/// A single search result on an indexer.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    /// Indexer-specific listing id (used by `listing_info` / `prepare_download`).
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeders: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leechers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// URL of the listing page on the indexer.
    pub url: String,
}

/// One page of search results.
#[derive(Debug, Clone, Serialize)]
pub struct PagedResults {
    pub page: u32,
    pub results: Vec<SearchResult>,
    /// Page number of the next page, if the indexer reported one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<u32>,
    /// Total number of matching listings, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Total number of pages, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_pages: Option<u32>,
}

/// What the indexer hands over for a confirmed download.
pub struct DownloadData {
    pub title: String,
    pub source: PendingSource,
    /// Optional qBittorrent save path hint from the indexer config.
    pub save_path: Option<String>,
}

#[async_trait]
pub trait Indexer: Send + Sync {
    /// Short human-readable description shown to the LLM.
    fn describe(&self) -> &'static str;

    /// Search the indexer. `page` is 1-based.
    async fn search(&self, query: &str, page: u32) -> Result<PagedResults>;

    /// Fetch the raw HTML page of a listing.
    async fn listing_info(&self, listing_id: &str) -> Result<String>;

    /// Resolve a listing into something qBittorrent can consume.
    async fn prepare_download(&self, listing_id: &str) -> Result<DownloadData>;
}

/// Build the enabled indexer set from the config.
pub fn build_indexers(config: &Config) -> Result<HashMap<String, Box<dyn Indexer>>> {
    let mut indexers: HashMap<String, Box<dyn Indexer>> = HashMap::new();
    for (id, idx_config) in &config.indexers {
        let indexer: Box<dyn Indexer> = match idx_config {
            IndexerConfig::Nyaa(cfg) => Box::new(nyaa::Nyaa::new(cfg.clone())?),
            IndexerConfig::Kinozal(cfg) => Box::new(kinozal::Kinozal::new(cfg.clone())?),
        };
        indexers.insert(id.clone(), indexer);
    }
    if indexers.is_empty() {
        bail!("no indexers configured");
    }
    Ok(indexers)
}
