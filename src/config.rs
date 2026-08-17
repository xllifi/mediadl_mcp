//! JSON configuration for the server, qBittorrent and indexers.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// How long a prepared download confirmation token stays valid.
pub const TOKEN_TTL: chrono::Duration = chrono::Duration::hours(24);

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Absolute or relative URL of the qBittorrent WebUI, e.g. "http://127.0.0.1:8080".
    pub qbittorrent_url: String,
    #[serde(default)]
    pub qbittorrent_username: Option<String>,
    #[serde(default)]
    pub qbittorrent_password: Option<String>,
    /// Indexers keyed by their public id (used in tool requests).
    #[serde(default)]
    pub indexers: HashMap<String, IndexerConfig>,
    /// HTTP server + auth settings.
    #[serde(default)]
    pub http: HttpConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpConfig {
    /// Address to bind the MCP HTTP endpoint to, e.g. "0.0.0.0:8000".
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Shared-secret auth token. Accepted as `Authorization: Bearer <token>` or
    /// `?api_key=<token>`. Required unless `allow_insecure_no_auth` is set.
    #[serde(default)]
    pub token: Option<String>,
    /// Escape hatch to run without any auth (NOT recommended; the endpoint can
    /// trigger downloads). Defaults to false.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            token: None,
            allow_insecure_no_auth: false,
        }
    }
}

fn default_listen() -> String {
    "0.0.0.0:8000".to_string()
}

impl HttpConfig {
    pub fn validate(&self) -> Result<()> {
        match &self.token {
            Some(t) if !t.is_empty() => Ok(()),
            _ if self.allow_insecure_no_auth => Ok(()),
            _ => bail!(
                "http.token is required (set http.allow_insecure_no_auth=true to run without auth)"
            ),
        }
    }

    /// Whether token auth is enforced.
    pub fn auth_enabled(&self) -> bool {
        matches!(&self.token, Some(t) if !t.is_empty())
    }
}

/// Per-indexer configuration. All indexer-specific data (credentials etc.)
/// lives in here as plain JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IndexerConfig {
    Nyaa(NyaaConfig),
    Kinozal(KinozalConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct NyaaConfig {
    /// Base URL override (e.g. a mirror). Defaults to https://nyaa.si
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KinozalConfig {
    /// Base URL override (e.g. https://kinozal.me). Defaults to https://kinozal.guru
    #[serde(default)]
    pub base_url: Option<String>,
    pub username: String,
    pub password: String,
}

impl Config {
    pub fn load(path: &PathBuf) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let config: Config = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()))?;
        config.http.validate()?;
        Ok(config)
    }
}
