//! Token authentication middleware for the MCP endpoint.
//!
//! Accepts a static shared secret via either:
//!   * the `Authorization: Bearer <token>` header (MCP clients that can set headers), or
//!   * an `?api_key=<token>` query parameter — required for ChatGPT's web UI, which
//!     cannot set request headers and is configured with authentication "None".
//!
//! The comparison is constant-time.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use subtle::ConstantTimeEq;

#[derive(Debug, Default, Deserialize)]
pub struct ApiKeyParam {
    api_key: Option<String>,
}

/// Axum middleware enforcing token auth. `expected` is the secret token.
pub async fn token_auth(
    State(expected): State<String>,
    Query(params): Query<ApiKeyParam>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let provided = extract_bearer(request.headers()).or(params.api_key.as_deref());
    let ok = provided
        .map(|token| ct_eq(token.as_bytes(), expected.as_bytes()))
        .unwrap_or(false);
    if ok {
        next.run(request).await
    } else {
        unauthorized()
    }
}

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"mediadl-mcp\"")],
        "unauthorized",
    )
        .into_response()
}

/// Constant-time comparison. Both sides are hashed first so the compared length
/// is fixed regardless of token length (avoids a length side-channel).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use sha2::{Digest, Sha256};
    let digest = |d: &[u8]| -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(d);
        h.finalize().into()
    };
    digest(a).ct_eq(&digest(b)).into()
}
