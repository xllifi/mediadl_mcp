# syntax=docker/dockerfile:1

# ---- build stage -------------------------------------------------------------
# Pin builder and runtime to the same Debian release so the glibc matches.
FROM rust:trixie AS builder
WORKDIR /app

# Copy manifests first for dependency-layer caching.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# tests/fixtures are included so `cargo build --locked` stays consistent if the
# crate ever compiles tests in-image; they don't affect the release binary.
COPY tests ./tests

RUN cargo build --release --locked \
    && strip target/release/mediadl-mcp

# ---- runtime stage -----------------------------------------------------------
FROM debian:trixie-slim

# ca-certificates: TLS to indexers / qbittorrent. tini: proper PID 1 reaping.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

# Run as a non-root user.
RUN groupadd -r mediadl && useradd -r -g mediadl -u 10001 mediadl

# Tokens (and optionally the config) live here; mount a volume to persist them.
RUN mkdir -p /data && chown mediadl:mediadl /data
VOLUME ["/data"]

COPY --from=builder /app/target/release/mediadl-mcp /usr/local/bin/mediadl-mcp
RUN chmod 0755 /usr/local/bin/mediadl-mcp

USER mediadl
WORKDIR /data

# stdio MCP server: no ports are exposed.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/mediadl-mcp"]
CMD ["--config", "/data/config.json", "--tokens", "/data/tokens.json"]
