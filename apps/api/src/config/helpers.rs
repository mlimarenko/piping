//! Configuration helper functions.

use axum::http::HeaderMap;

use super::Settings;

/// Returns the backend's own base URL for proxy routing when backend acts as proxy.
///
/// Proxy URLs are business logic (from DB, selected in UI). This is used when the
/// selected proxy is "backend" (default). Derives `http://127.0.0.1:{port}` from `server.bind`.
pub fn effective_proxy_base_url(settings: &Settings) -> String {
    let port = extract_port_from_bind(&settings.server.bind);
    format!("http://127.0.0.1:{}", port)
}

/// Derives the public-facing base URL from request headers so browser-visible
/// WebSocket URLs point at the edge proxy (e.g. nginx on :9080) rather than the
/// backend's internal bind address. Honours `X-Forwarded-Host` / `X-Forwarded-Proto`
/// and falls back to `Host`.
pub fn public_base_url_from_headers(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    Some(format!("{}://{}", scheme, host))
}

fn extract_port_from_bind(bind: &str) -> u16 {
    bind.rsplit(':')
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(super::constants::DEFAULT_SERVER_PORT)
}
