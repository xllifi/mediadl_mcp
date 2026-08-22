# mediadl-mcp

An [MCP](https://modelcontextprotocol.io) server that lets an LLM search torrent
indexers for movies / TV shows / anime and download them through **qBittorrent**,
behind a two-step confirmation flow.

Serves MCP over **streamable HTTP** at `/mcp`, protected by a static bearer
token (header or query-param).

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
  "http": {
    "listen": "0.0.0.0:8000",            // bind address for the MCP endpoint
    "token": "CHANGE_ME_LONG_RANDOM_SECRET", // bearer token (required)
    "allowed_hosts": ["mediadl.example.com"]  // public Host header (see below)
  },
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

The MCP endpoint is `http://<listen>/mcp` and requires a static auth token
(`http.token`), supplied either as `Authorization: Bearer <token>` **or** as an
`?api_key=<token>` query parameter (the query-param form exists because
ChatGPT's connector UI can't set request headers). The token is compared in
constant time. The server **refuses to start without a token** unless you set
`http.allow_insecure_no_auth: true` (not recommended — the endpoint can trigger
downloads). `/health` is unauthenticated so orchestrators can probe it.

`http.allowed_hosts` guards the `Host` header against DNS-rebinding attacks. It
defaults to loopback only, so when you serve behind a reverse proxy on a public
hostname you **must** add that hostname here (otherwise requests are rejected
with `403` and a `rejected request with disallowed Host header` warning). A bare
hostname matches any port. Use `["*"]` to disable the check entirely.

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
# listens on http://<http.listen>/mcp (default 0.0.0.0:8000)
```

### Wire it into an MCP client

Point your client at the HTTP endpoint with the bearer token:

```json
{
  "mcpServers": {
    "mediadl": {
      "type": "http",
      "url": "http://127.0.0.1:8000/mcp",
      "headers": {
        "Authorization": "Bearer CHANGE_ME_LONG_RANDOM_SECRET"
      }
    }
  }
}
```

### Public HTTPS with Caddy (for ChatGPT)

ChatGPT needs a **public HTTPS URL** and can't send auth headers, so the setup
is: Caddy terminates TLS in front of the server, and the token travels in the
`?api_key=` query param. `compose.yml` ships a ready Caddy service and a
`Caddyfile` (automatic Let's Encrypt certs).

1. Point a public DNS record (e.g. `mediadl.example.com`) at your host and open
   ports `80` + `443`.
2. In `config.json`, set `http.allowed_hosts` to that hostname
   (`["mediadl.example.com"]`) — this is required or rmcp rejects the public
   `Host` header.
3. Bring it up:
   ```sh
   MCP_DOMAIN=mediadl.example.com docker compose up -d
   ```
   Caddy gets the certificate and proxies `https://<domain>/mcp` to the app. The
   app's own port is not exposed to the host — only Caddy's 80/443 are.
4. In ChatGPT: **Settings → Apps → Advanced Options → enable Developer Mode**,
   then create a connector:
   * **URL:** `https://mediadl.example.com/mcp?api_key=CHANGE_ME_LONG_RANDOM_SECRET`
   * **Authentication:** `None` (the token is already in the URL)
5. Enable the connector in a chat and ask e.g. "search nyaa for Frieren".

ChatGPT fetches the tool list when you create the connector, so the URL must be
live and the token valid at that point. Note: developer mode disables ChatGPT's
memory feature; use a long random token since the endpoint can trigger downloads.

### Docker

```sh
docker build -t mediadl-mcp .
# or: docker compose build
```

The image runs as a non-root user, stores confirmation tokens in the `/data`
volume, and exposes port `8000`. Run it (or use `compose.yml`):

```sh
docker run -d --name mediadl-mcp \
  -p 8000:8000 \
  -v /path/to/config.json:/data/config.json:ro \
  -v mediadl-data:/data \
  mediadl-mcp:latest
```

`compose.yml` runs Caddy + the app by default. It also ships an optional
qBittorrent service:

```sh
docker compose --profile qbittorrent up   # brings up qBittorrent too
```

Set `"qbittorrent_url": "http://qbittorrent:8080"` in `config.json` to use it.
Then point your MCP client at `http://127.0.0.1:8000/mcp` with the bearer token.

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
  main.rs              binary entrypoint (args, logging, HTTP transport)
  lib.rs               library root
  auth.rs              HTTP token-auth middleware (constant-time)
  config.rs            JSON config (qBittorrent + HTTP/auth + per-indexer)
  qbittorrent.rs       qBittorrent WebUI API v2 client (login, version, add)
  tokens.rs            disk-persisted, single-use, 24h confirmation tokens
  server.rs            the 5 MCP tools
  indexer.rs           Indexer trait + shared types
  indexer/nyaa.rs      nyaa.si implementation
  indexer/kinozal.rs   kinozal.guru implementation
```
