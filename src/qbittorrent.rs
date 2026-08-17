//! Minimal qBittorrent WebUI API v2 client.

use anyhow::{Context, Result, bail};
use reqwest::multipart::{Form, Part};

use crate::config::Config;

#[derive(Clone)]
pub struct QBittorrentClient {
    http: reqwest::Client,
    base_url: String,
    username: Option<String>,
    password: Option<String>,
}

pub enum TorrentSource {
    Magnet(String),
    File { name: String, bytes: Vec<u8> },
}

impl QBittorrentClient {
    pub fn new(config: &Config) -> Self {
        Self {
            http: reqwest::Client::builder()
                .cookie_store(true)
                .build()
                .expect("failed to build qBittorrent http client"),
            base_url: config.qbittorrent_url.trim_end_matches('/').to_string(),
            username: config.qbittorrent_username.clone(),
            password: config.qbittorrent_password.clone(),
        }
    }

    async fn login(&self) -> Result<()> {
        let mut form = vec![];
        if let (Some(u), Some(p)) = (&self.username, &self.password) {
            form.push(("username", u.clone()));
            form.push(("password", p.clone()));
        }
        let resp = self
            .http
            .post(format!("{}/api/v2/auth/login", self.base_url))
            .form(&form)
            .send()
            .await
            .context("qBittorrent login request failed")?;
        if !resp.status().is_success() {
            bail!("qBittorrent login failed: HTTP {}", resp.status());
        }
        let body = resp.text().await.context("failed to read qBittorrent login response")?;
        if body.trim() != "Ok." {
            bail!("qBittorrent login rejected: {body}");
        }
        Ok(())
    }

    /// Returns the qBittorrent application version. Used as a connectivity check.
    pub async fn version(&self) -> Result<String> {
        self.login().await?;
        let resp = self
            .http
            .get(format!("{}/api/v2/app/version", self.base_url))
            .send()
            .await
            .context("qBittorrent version request failed")?;
        if !resp.status().is_success() {
            bail!("qBittorrent version request failed: HTTP {}", resp.status());
        }
        Ok(resp.text().await?.trim().to_string())
    }

    /// Add a torrent by magnet link or .torrent file. Optionally override the save path.
    pub async fn add_torrent(&self, source: TorrentSource, save_path: Option<&str>) -> Result<()> {
        self.login().await?;
        let mut form = Form::new();
        match source {
            TorrentSource::Magnet(url) => {
                form = form.text("urls", url);
            }
            TorrentSource::File { name, bytes } => {
                let part = Part::bytes(bytes)
                    .file_name(name)
                    .mime_str("application/x-bittorrent")?;
                form = form.part("torrents", part);
            }
        }
        if let Some(path) = save_path {
            form = form.text("savepath", path.to_string());
        }
        let resp = self
            .http
            .post(format!("{}/api/v2/torrents/add", self.base_url))
            .multipart(form)
            .send()
            .await
            .context("qBittorrent add torrent request failed")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("qBittorrent add torrent failed: HTTP {status}: {body}");
        }
        if body.trim() != "Ok." {
            bail!("qBittorrent add torrent rejected: {body}");
        }
        Ok(())
    }
}
