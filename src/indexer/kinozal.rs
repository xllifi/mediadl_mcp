//! kinozal.guru indexer (Russian semi-private tracker for movies / TV / cartoons).
//!
//! The site speaks windows-1251, requires an account (login via takelogin.php,
//! session cookie), search is browse.php, listings are details.php?id=N and
//! downloads are .torrent files at download.php?id=N tied to the account passkey.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use scraper::{Html, Selector};
use tokio::sync::Mutex;

use crate::config::KinozalConfig;
use crate::indexer::{DownloadData, Indexer, PagedResults, SearchResult};
use crate::tokens::PendingSource;

const DEFAULT_BASE_URL: &str = "https://kinozal.guru";
const RESULTS_PER_PAGE: u64 = 50;

pub struct Kinozal {
    http: reqwest::Client,
    base_url: String,
    username: String,
    password: String,
    /// Session cache: we re-login lazily when a request bounces to the login page.
    logged_in: Mutex<bool>,
}

impl Kinozal {
    pub fn new(config: KinozalConfig) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .cookie_store(true)
                .user_agent("mediadl-mcp/0.1")
                .build()?,
            base_url: config
                .base_url
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            username: config.username,
            password: config.password,
            logged_in: Mutex::new(false),
        })
    }

    /// Perform the login POST. kinozal answers with a session cookie; the body is windows-1251.
    async fn login(&self) -> Result<()> {
        let resp = self
            .http
            .post(format!("{}/takelogin.php", self.base_url))
            .form(&[("username", &self.username), ("password", &self.password)])
            .send()
            .await
            .context("kinozal login request failed")?;
        let status = resp.status();
        let body = decode_windows1251(&resp.bytes().await?)?;
        if !status.is_success() {
            bail!("kinozal login failed: HTTP {status}");
        }
        // Failed logins render the login form / an error box again instead of
        // redirecting into the site.
        if body.contains("takelogin.php")
            || body.contains("name=\"username\"")
            || body.contains("name='username'")
        {
            bail!("kinozal login failed: bad username or password");
        }
        *self.logged_in.lock().await = true;
        Ok(())
    }

    /// GET a page, ensuring we are authenticated first. Retries once after re-login
    /// if the site drops us back to the login page (expired session).
    async fn get_html(&self, url: &str) -> Result<String> {
        for attempt in 0..2 {
            if !*self.logged_in.lock().await {
                self.login().await?;
            }
            let resp = self
                .http
                .get(url)
                .send()
                .await
                .with_context(|| format!("kinozal request failed: {url}"))?;
            let status = resp.status();
            if !status.is_success() {
                bail!("kinozal returned HTTP {status} for {url}");
            }
            let body = decode_windows1251(&resp.bytes().await?)?;
            let bounced = body.contains("action=\"takelogin.php\"")
                || body.contains("action='takelogin.php'");
            if bounced && attempt == 0 {
                // Session expired; force re-login and retry once.
                *self.logged_in.lock().await = false;
                continue;
            }
            if bounced {
                bail!("kinozal keeps redirecting to the login page; check credentials");
            }
            return Ok(body);
        }
        unreachable!()
    }

    /// Parse a browse.php results page.
    fn parse_search(base_url: &str, html: &str, page: u32) -> PagedResults {
        let doc = Html::parse_document(html);

        let row_sel = selector("tr:has(td.bt)");
        let link_sel = selector("td.nam a[href*=\"details.php\"]");
        let size_sel = selector("td:nth-child(4)");
        let seed_sel = selector("td:nth-child(5)");
        let leech_sel = selector("td:nth-child(6)");
        let date_sel = selector("td:nth-child(7)");

        let mut results = Vec::new();
        for row in doc.select(&row_sel) {
            let Some(link) = row.select(&link_sel).next() else {
                continue;
            };
            let href = link.value().attr("href").unwrap_or_default();
            let Some(id) = href
                .split("id=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .map(str::to_string)
            else {
                continue;
            };
            let title = link.text().collect::<String>().trim().to_string();
            let cell_text = |sel: &Selector| {
                row.select(sel)
                    .next()
                    .map(|e| e.text().collect::<String>().trim().to_string())
            };
            results.push(SearchResult {
                id,
                title,
                size: cell_text(&size_sel),
                seeders: parse_u32(cell_text(&seed_sel)),
                leechers: parse_u32(cell_text(&leech_sel)),
                date: cell_text(&date_sel),
                url: format!("{}/{}", base_url, href.trim_start_matches('/')),
            });
        }

        // Pagination: pages are rendered as ?page=N links (0-based); the max one
        // tells us how many pages exist.
        let page_link_sel = selector("a[href*=\"page=\"]");
        let max_page = doc
            .select(&page_link_sel)
            .filter_map(|a| {
                a.value()
                    .attr("href")
                    .and_then(|h| h.split("page=").nth(1))
                    .and_then(|s| s.split('&').next())
                    .and_then(|s| s.parse::<u32>().ok())
            })
            .max();
        let total_pages = max_page.map(|m| m + 1); // 0-based -> count
        let next_page = match total_pages {
            Some(tp) if page < tp => Some(page + 1),
            None if results.len() as u64 == RESULTS_PER_PAGE => Some(page + 1),
            _ => None,
        };

        PagedResults {
            page,
            results,
            next_page,
            total: None,
            total_pages,
        }
    }

    #[doc(hidden)]
    pub fn __test_parse_search(base_url: &str, html: &str, page: u32) -> PagedResults {
        Self::parse_search(base_url, html, page)
    }

    fn parse_title(html: &str) -> Option<String> {
        let doc = Html::parse_document(html);
        let title_sel = selector("h1");
        doc.select(&title_sel)
            .next()
            .map(|h| h.text().collect::<String>().trim().to_string())
            .filter(|t| !t.is_empty())
    }
}

/// kinozal serves windows-1251.
fn decode_windows1251(bytes: &[u8]) -> Result<String> {
    let (cow, _, had_errors) = encoding_rs::WINDOWS_1251.decode(bytes);
    if had_errors {
        tracing::warn!("kinozal page contained undecodable windows-1251 sequences");
    }
    Ok(cow.into_owned())
}

fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("static selector must be valid")
}

fn parse_u32(text: Option<String>) -> Option<u32> {
    text.and_then(|t| t.trim().parse().ok())
}

#[async_trait]
impl Indexer for Kinozal {
    fn describe(&self) -> &'static str {
        "kinozal.guru: Russian semi-private tracker for movies, TV shows and cartoons (incl. anime). Requires account credentials. Downloads are .torrent files bound to the account passkey."
    }

    async fn search(&self, query: &str, page: u32) -> Result<PagedResults> {
        if query.trim().is_empty() {
            bail!("search query must not be empty");
        }
        // browse.php: s=query, g=0 (search in titles), c/v/d=0 (all categories/formats/years),
        // w=0 (no filter), t=0 (sort by date), f=0 (desc), page is 0-based.
        let url = format!(
            "{}/browse.php?s={}&g=0&c=0&v=0&d=0&w=0&t=0&f=0&page={}",
            self.base_url,
            urlencoding(query),
            page.max(1) - 1
        );
        let html = self.get_html(&url).await?;
        Ok(Self::parse_search(&self.base_url, &html, page))
    }

    async fn listing_info(&self, listing_id: &str) -> Result<String> {
        let id: u64 = listing_id
            .trim()
            .parse()
            .with_context(|| format!("invalid kinozal listing id {listing_id:?} (expected a number)"))?;
        self.get_html(&format!("{}/details.php?id={}", self.base_url, id))
            .await
    }

    async fn prepare_download(&self, listing_id: &str) -> Result<DownloadData> {
        // Grab the listing title first (also validates the id and the session).
        let html = self.listing_info(listing_id).await?;
        let title = Self::parse_title(&html).unwrap_or_else(|| format!("kinozal {listing_id}"));

        // download.php?id=N serves the .torrent file bound to this account's passkey.
        let url = format!("{}/download.php?id={}", self.base_url, listing_id.trim());
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("kinozal download request failed")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("kinozal torrent download failed: HTTP {status}");
        }
        let bytes = resp.bytes().await?.to_vec();
        // A real .torrent file is bencode and starts with 'd'. An HTML response
        // means the session died or the daily download limit was hit.
        if bytes.first() != Some(&b'd') {
            let body = decode_windows1251(&bytes).unwrap_or_default();
            bail!(
                "kinozal did not return a .torrent file (session expired or daily download limit reached): {}",
                body.chars().take(300).collect::<String>()
            );
        }
        use base64::Engine;
        Ok(DownloadData {
            title,
            source: PendingSource::TorrentFile {
                name: format!("kinozal-{listing_id}.torrent"),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            save_path: None,
        })
    }
}

/// Minimal percent-encoding for query strings.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
