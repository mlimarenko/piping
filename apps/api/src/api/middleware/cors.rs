use axum::http::{
    Method,
    header::{HeaderName, HeaderValue},
};
use tower_http::cors::{AllowHeaders, AllowOrigin, Any, CorsLayer};

use crate::config::Settings;

/// Build CORS layer using config settings.
///
/// - Empty `cors_allowed_origins` → allow all origins (development mode).
/// - Non-empty → restrict to listed origins and enable credentials (cookie-based auth).
pub fn build_cors_layer(settings: &Settings) -> CorsLayer {
    let accept_header = HeaderName::from_static("accept");
    let authorization_header = HeaderName::from_static("authorization");
    let cache_control_header = HeaderName::from_static("cache-control");
    let content_type_header = HeaderName::from_static("content-type");
    let origin_header = HeaderName::from_static("origin");
    let mcp_protocol_version_header = HeaderName::from_static("mcp-protocol-version");
    let pragma_header = HeaderName::from_static("pragma");
    let request_id_header = HeaderName::from_static("x-request-id");
    let requested_with_header = HeaderName::from_static("x-requested-with");
    let traceparent_header = HeaderName::from_static("traceparent");
    let tracestate_header = HeaderName::from_static("tracestate");

    let mut layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .expose_headers([
            request_id_header.clone(),
            traceparent_header.clone(),
            tracestate_header.clone(),
        ]);

    let origins = &settings.server.cors_allowed_origins;
    if origins.is_empty() {
        layer = layer
            .allow_origin(Any)
            .allow_credentials(false)
            .allow_headers(AllowHeaders::mirror_request());
    } else {
        let header_values: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|s| HeaderValue::try_from(s.as_str()).ok())
            .collect();
        if header_values.is_empty() {
            layer = layer
                .allow_origin(Any)
                .allow_credentials(false)
                .allow_headers(AllowHeaders::mirror_request());
        } else {
            layer = layer
                .allow_origin(AllowOrigin::list(header_values))
                .allow_headers([
                    accept_header,
                    authorization_header,
                    cache_control_header,
                    content_type_header,
                    mcp_protocol_version_header,
                    origin_header,
                    pragma_header,
                    request_id_header,
                    requested_with_header,
                    traceparent_header,
                    tracestate_header,
                ])
                .allow_credentials(true);
        }
    }

    layer
}
