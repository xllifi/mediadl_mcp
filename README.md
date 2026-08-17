# mediadl-mcp

An [MCP](https://modelcontextprotocol.io) server that lets an LLM search torrent
indexers for movies / TV shows / anime and download them through **qBittorrent**,
behind a two-step confirmation flow.

Transports over **stdio** (the standard way MCP servers are launched by clients).

# ⚠️ WARNING: FULLY VIBECODED SLOP ⚠️
Ye be warned.

## Tools

| Tool | Request | Response |
|------|---------|----------|
| `status` | — | Server version + qBittorrent connection status |
| `search` | `indexer_id`, `query`, `page?` (1-based) | One page of results (`id`, `title`, `size`, `seeders`, `leechers`, `date`, `url`) plus `next_page` / `total` / `total_pages` |
| `listing_info` | `indexer_id`, `listing_id` | Raw HTML of the listing page |
| `prepare_download` | `indexer_id`, `listing_id` | `confirmation_token` + `expires_at` (24h). Nothing is downloaded yet |
| `confirm_download` | `token` | Adds the torrent to qBittorrent |

Any operational error (site down, bad credentials, dead torrent, expired token,
qBittorrent unreachable, …) is returned to the LLM as tool error content
(`isError: true`) so the model can see and react to it. Only protocol-level
problems (e.g. an unknown `indexer_id`) come back as JSON-RPC errors.

### Download confirmation flow

Downloads never happen directly. `prepare_download` resolves the listing to a
magnet link or `.torrent` file and stores a **single-use confirmation token**
that expires after **24 hours**. Tokens are **persisted to disk** (`tokens.json`
by default), so they survive server crashes and restarts; expired tokens are
pruned on startup and on every use. `confirm_download` consumes the token and
adds the torrent to qBittorrent.

## Configuration

Copy `config.example.json` to `config.json` and edit it. Every indexer is a
plain JSON object under `indexers`, keyed by the id you'll pass as `indexer_id`.
Indexer implementations are **hardcoded** (one Rust module per site) and
selected via the `type` field — there is no DSL and no scraping rules in the
config, only credentials and per-indexer options.

```jsonc
{
  "qbittorrent_url": "http://127.0.0.1:8080",
  "qbittorrent_username": "admin",        // optional if qBittorrent auth is bypassed
  "qbittorrent_password": "adminadmin",
  "indexers": {
    "nyaa":    { "type": "nyaa" },
    "kinozal": {
      "type": "kinozal",
      "username": "YOUR_KINOZAL_USERNAME",
      "password": "YOUR_KINOZAL_PASSWORD"
    }
  }
}
```

### Indexers

| `type` | Site | Credentials | Notes |
|--------|------|-------------|-------|
| `nyaa` | [nyaa.si](https://nyaa.si) | none | Public anime tracker. Downloads via magnet links. |
| `kinozal` | [kinozal.guru](https://kinozal.guru) | `username`, `password` | Russian semi-private tracker (movies / TV / cartoons). Downloads `.torrent` files bound to your account passkey; the site enforces a daily download limit. |

Both accept an optional `base_url` override to point at a mirror.

## Build & run

```sh
cargo build --release
./target/release/mediadl-mcp --config config.json --tokens tokens.json
```

### Docker

```sh
docker build -t mediadl-mcp .
# or: docker compose build
```

The image runs as a non-root user and stores confirmation tokens in the `/data`
volume. It's a stdio server, so it's normally spawned by an MCP client; point
the client at the container:

```json
{
  "mcpServers": {
    "mediadl": {
      "command": "docker",
      "args": [
        "run", "--rm", "-i",
        "-v", "/path/to/config.json:/data/config.json:ro",
        "-v", "mediadl-data:/data",
        "mediadl-mcp:latest"
      ]
    }
  }
}
```

`compose.yml` also ships an optional qBittorrent service:

```sh
docker compose --profile qbittorrent up   # brings up qBittorrent too
```

Set `"qbittorrent_url": "http://qbittorrent:8080"` in `config.json` to use it.

### Wire it into an MCP client (example: Claude Desktop / claude_desktop_config.json)

```json
{
  "mcpServers": {
    "mediadl": {
      "command": "/path/to/mediadl-mcp",
      "args": ["--config", "/path/to/config.json", "--tokens", "/path/to/tokens.json"]
    }
  }
}
```

## Development

```sh
cargo test    # parser fixtures + token store
cargo clippy
```

HTML parsing is covered by tests against captured fixtures in `tests/fixtures/`.
Because the indexers scrape HTML, a site redesign can break a selector — the
error is surfaced to the LLM rather than crashing the server.

## Layout

```
src/
  main.rs              binary entrypoint (args, logging, stdio transport)
  lib.rs               library root
  config.rs            JSON config (qBittorrent + per-indexer)
  qbittorrent.rs       qBittorrent WebUI API v2 client (login, version, add)
  tokens.rs            disk-persisted, single-use, 24h confirmation tokens
  server.rs            the 5 MCP tools
  indexer.rs           Indexer trait + shared types
  indexer/nyaa.rs      nyaa.si implementation
  indexer/kinozal.rs   kinozal.guru implementation
```
