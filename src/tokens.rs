//! Download confirmation tokens, persisted to disk so they survive restarts.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::TOKEN_TTL;
use crate::qbittorrent::TorrentSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDownload {
    pub token: String,
    pub indexer_id: String,
    pub listing_id: String,
    pub title: String,
    /// Magnet link, or base64-encoded .torrent file.
    pub source: PendingSource,
    /// Optional qBittorrent save path hint from the indexer config.
    #[serde(default)]
    pub save_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingSource {
    Magnet { url: String },
    TorrentFile { name: String, data_base64: String },
}

impl PendingSource {
    pub fn to_torrent_source(&self) -> Result<TorrentSource> {
        use base64::Engine;
        match self {
            PendingSource::Magnet { url } => Ok(TorrentSource::Magnet(url.clone())),
            PendingSource::TorrentFile { name, data_base64 } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .context("corrupt stored torrent file data")?;
                Ok(TorrentSource::File {
                    name: name.clone(),
                    bytes,
                })
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TokenFile {
    tokens: Vec<PendingDownload>,
}

#[derive(Clone)]
pub struct TokenStore {
    path: PathBuf,
    inner: Arc<Mutex<Vec<PendingDownload>>>,
}

impl TokenStore {
    /// Load tokens from disk (missing file => empty store).
    pub async fn load(path: PathBuf) -> Result<Self> {
        let tokens = match tokio::fs::read_to_string(&path).await {
            Ok(raw) => serde_json::from_str::<TokenFile>(&raw)
                .with_context(|| format!("failed to parse token store {}", path.display()))?
                .tokens,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("failed to read token store {}", path.display()));
            }
        };
        let store = Self {
            path,
            inner: Arc::new(Mutex::new(tokens)),
        };
        // Drop expired tokens on startup.
        store.prune_expired().await?;
        Ok(store)
    }

    async fn save(&self, tokens: &[PendingDownload]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let data = serde_json::to_string_pretty(&TokenFile {
            tokens: tokens.to_vec(),
        })?;
        // Write via temp file + rename to avoid corrupting the store on crash.
        let tmp = self.path.with_extension("tmp");
        tokio::fs::write(&tmp, data)
            .await
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .with_context(|| format!("failed to persist {}", self.path.display()))?;
        Ok(())
    }

    async fn prune_expired(&self) -> Result<()> {
        let mut tokens = self.inner.lock().await;
        let before = tokens.len();
        tokens.retain(|t| t.expires_at > Utc::now());
        if tokens.len() != before {
            self.save(&tokens).await?;
        }
        Ok(())
    }

    /// Create and persist a new confirmation token valid for [`TOKEN_TTL`].
    pub async fn create(
        &self,
        indexer_id: String,
        listing_id: String,
        title: String,
        source: PendingSource,
        save_path: Option<String>,
    ) -> Result<PendingDownload> {
        self.prune_expired().await?;
        let now = Utc::now();
        let pending = PendingDownload {
            token: Uuid::new_v4().to_string(),
            indexer_id,
            listing_id,
            title,
            source,
            save_path,
            created_at: now,
            expires_at: now + TOKEN_TTL,
        };
        let mut tokens = self.inner.lock().await;
        tokens.push(pending.clone());
        self.save(&tokens).await?;
        Ok(pending)
    }

    /// Consume a token: returns the pending download and removes it from the store.
    pub async fn consume(&self, token: &str) -> Result<PendingDownload> {
        self.prune_expired().await?;
        let mut tokens = self.inner.lock().await;
        let Some(pos) = tokens.iter().position(|t| t.token == token) else {
            bail!("unknown or expired confirmation token");
        };
        let pending = tokens.remove(pos);
        self.save(&tokens).await?;
        Ok(pending)
    }
}
