//! nyaa.si indexer (public, no credentials needed). Anime-focused, also carries movies/TV.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use scraper::{Html, Selector};

use crate::config::NyaaConfig;
use crate::indexer::{DownloadData, Indexer, PagedResults, SearchResult};
use crate::tokens::PendingSource;

const DEFAULT_BASE_URL: &str = "https://nyaa.si";
const RESULTS_PER_PAGE: u64 = 75;

pub struct Nyaa {
    http: reqwest::Client,
    base_url: String,
}

impl Nyaa {
    pub fn new(config: NyaaConfig) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("mediadl-mcp/0.1 (https://nyaa.si)")
                .build()?,
            base_url: config
                .base_url
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
        })
    }

    async fn get_html(&self, url: &str) -> Result<String> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("nyaa request failed: {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("nyaa returned HTTP {status} for {url}");
        }
        Ok(resp.text().await?)
    }
}

fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("static selector must be valid")
}

#[async_trait]
impl Indexer for Nyaa {
    fn describe(&self) -> &'static str {
        "nyaa.si: public BitTorrent tracker focused on anime, also carries movies/TV and other East-Asian media. No credentials needed."
    }

    async fn search(&self, query: &str, page: u32) -> Result<PagedResults> {
        if query.trim().is_empty() {
            bail!("search query must not be empty");
        }
        let url = format!(
            "{}/?f=0&c=0_0&q={}&p={}",
            self.base_url,
            urlencoding(query),
            page.max(1)
        );
        let html = self.get_html(&url).await?;
        Ok(parse_search(&self.base_url, &html, page))
    }

    async fn listing_info(&self, listing_id: &str) -> Result<String> {
        let id: u64 = listing_id
            .trim()
            .parse()
            .with_context(|| format!("invalid nyaa listing id {listing_id:?} (expected a number)"))?;
        self.get_html(&format!("{}/view/{}", self.base_url, id)).await
    }

    async fn prepare_download(&self, listing_id: &str) -> Result<DownloadData> {
        let html = self.listing_info(listing_id).await?;
        parse_listing(listing_id, &html)
    }
}

#[doc(hidden)]
pub fn __test_parse_search(base_url: &str, html: &str, page: u32) -> PagedResults {
    parse_search(base_url, html, page)
}

#[doc(hidden)]
pub fn __test_parse_listing(listing_id: &str, html: &str) -> Result<DownloadData> {
    parse_listing(listing_id, html)
}

fn parse_search(base_url: &str, html: &str, page: u32) -> PagedResults {
    let doc = Html::parse_document(html);

    let row_sel = selector("table.torrent-list > tbody > tr");
    let link_sel = selector("td:nth-child(2) a[href^=\"/view/\"]");
    let size_sel = selector("td:nth-child(4)");
    let date_sel = selector("td:nth-child(5)");
    let seed_sel = selector("td:nth-child(6)");
    let leech_sel = selector("td:nth-child(7)");

    let mut results = Vec::new();
    for row in doc.select(&row_sel) {
        let Some(link) = row.select(&link_sel).last() else {
            continue;
        };
        let href = link.value().attr("href").unwrap_or_default();
        let id = href.trim_start_matches("/view/").to_string();
        let title = link
            .value()
            .attr("title")
            .map(str::to_string)
            .unwrap_or_else(|| link.text().collect::<String>().trim().to_string());
        let cell_text = |sel: &Selector| {
            row.select(sel)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
        };
        results.push(SearchResult {
            id,
            title,
            size: cell_text(&size_sel),
            seeders: cell_text(&seed_sel).and_then(|t| t.parse().ok()),
            leechers: cell_text(&leech_sel).and_then(|t| t.parse().ok()),
            date: cell_text(&date_sel),
            url: format!("{}{}", base_url, href),
        });
    }

    // "Displaying results 1-75 out of 1234 results."
    let mut total = None;
    let info_sel = selector(".pagination-page-info");
    if let Some(info) = doc.select(&info_sel).next() {
        let text = info.text().collect::<String>();
        if let Some(n) = text
            .split(" out of ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u64>().ok())
        {
            total = Some(n);
        }
    }
    let total_pages = total.map(|t| t.div_ceil(RESULTS_PER_PAGE) as u32);
    let next_page = match total_pages {
        Some(tp) if page < tp => Some(page + 1),
        _ => None,
    };

    PagedResults {
        page,
        results,
        next_page,
        total,
        total_pages,
    }
}

fn parse_listing(listing_id: &str, html: &str) -> Result<DownloadData> {
    let doc = Html::parse_document(html);

    let magnet_sel = selector("a[href^=\"magnet:?\"]");
    let magnet = doc
        .select(&magnet_sel)
        .next()
        .and_then(|a| a.value().attr("href"))
        .map(str::to_string)
        .with_context(|| {
            format!("no magnet link found on nyaa listing {listing_id} (dead torrent?)")
        })?;

    let title_sel = selector("h3.panel-title");
    let title = doc
        .select(&title_sel)
        .next()
        .map(|h| h.text().collect::<String>().trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("nyaa {listing_id}"));

    Ok(DownloadData {
        title,
        source: PendingSource::Magnet { url: magnet },
        save_path: None,
    })
}

/// Minimal percent-encoding for query strings (keep it simple, nyaa is lenient).
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
