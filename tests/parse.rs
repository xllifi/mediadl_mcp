//! Unit tests for the HTML parsers (using captured fixtures) and the token store.

use mediadl_mcp::indexer;
use mediadl_mcp::indexer::kinozal::Kinozal;
use mediadl_mcp::tokens;

#[test]
fn nyaa_search_parses_rows_and_pagination() {
    let html = include_str!("fixtures/nyaa_search.html");
    let page = indexer::nyaa::__test_parse_search("https://nyaa.si", html, 1);
    assert_eq!(page.results.len(), 2);
    assert_eq!(page.results[0].id, "1950001");
    assert_eq!(
        page.results[0].title,
        "[SubsPlease] Sousou no Frieren - 28 (1080p) [ABCD1234].mkv"
    );
    assert_eq!(page.results[0].size.as_deref(), Some("1.3 GiB"));
    assert_eq!(page.results[0].seeders, Some(1523));
    assert_eq!(page.results[0].leechers, Some(42));
    assert_eq!(page.results[0].url, "https://nyaa.si/view/1950001");
    assert_eq!(page.total, Some(322));
    assert_eq!(page.total_pages, Some(5)); // ceil(322 / 75)
    assert_eq!(page.next_page, Some(2));
}

#[test]
fn nyaa_listing_extracts_magnet_and_title() {
    let html = include_str!("fixtures/nyaa_view.html");
    let data = indexer::nyaa::__test_parse_listing("1950001", html).unwrap();
    assert_eq!(
        data.title,
        "[SubsPlease] Sousou no Frieren - 28 (1080p) [ABCD1234].mkv"
    );
    match data.source {
        tokens::PendingSource::Magnet { url } => {
            assert!(url.starts_with("magnet:?xt=urn:btih:aaaa"));
            // &amp; entities must be decoded by the HTML parser.
            assert!(!url.contains("&amp;"));
            assert!(url.contains("&dn="));
        }
        _ => panic!("expected magnet source"),
    }
}

#[test]
fn kinozal_search_parses_rows_and_pagination() {
    let html = include_str!("fixtures/kinozal_browse.html");
    let page = Kinozal::__test_parse_search("https://kinozal.guru", html, 1);
    assert_eq!(page.results.len(), 2);
    assert_eq!(page.results[0].id, "1928374");
    assert!(page.results[0].title.contains("Мастер и Маргарита"));
    assert_eq!(page.results[0].size.as_deref(), Some("2.18 ГБ"));
    assert_eq!(page.results[0].seeders, Some(153));
    assert_eq!(page.results[0].leechers, Some(12));
    assert_eq!(page.results[1].id, "1928375");
    assert_eq!(page.results[1].seeders, Some(89));
    // pagination links page=0..2 (0-based) => 3 pages, next page after 1 is 2
    assert_eq!(page.total_pages, Some(3));
    assert_eq!(page.next_page, Some(2));
}

#[tokio::test]
async fn token_store_create_consume_and_expire() {
    let dir = std::env::temp_dir().join(format!("mediadl-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("tokens.json");
    let store = tokens::TokenStore::load(path.clone()).await.unwrap();

    let pending = store
        .create(
            "nyaa".into(),
            "1950001".into(),
            "Some title".into(),
            tokens::PendingSource::Magnet {
                url: "magnet:?xt=urn:btih:abc".into(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(pending.expires_at - pending.created_at == chrono::Duration::hours(24));

    // Survives a reload from disk.
    let store2 = tokens::TokenStore::load(path.clone()).await.unwrap();
    let got = store2.consume(&pending.token).await.unwrap();
    assert_eq!(got.listing_id, "1950001");

    // Single-use: a second consume fails.
    assert!(store2.consume(&pending.token).await.is_err());

    std::fs::remove_dir_all(&dir).ok();
}
