use axum::{extract::Request, middleware::Next, response::Response};
use uuid::Uuid;

use crate::api::rest::error_mapping::scope_request_trace_id;

#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Extension key for request ID
pub const REQUEST_ID_KEY: &str = "request_id";

fn resolve_request_id(headers: &axum::http::HeaderMap) -> String {
    const MAX_REQUEST_ID_LEN: usize = 128;

    if let Some(value) = headers.get("x-request-id")
        && let Ok(value) = value.to_str()
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() && trimmed.len() <= MAX_REQUEST_ID_LEN {
            return trimmed.to_string();
        }
    }

    Uuid::now_v7().to_string()
}

/// Middleware to generate UUIDv7 correlation ID and store in request extensions
pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    // Preserve valid upstream request ID when provided, otherwise generate UUIDv7.
    let request_id = RequestId(resolve_request_id(request.headers()));

    // Store in request extensions
    request.extensions_mut().insert(request_id.clone());

    // Add to response headers for client correlation
    let mut response = scope_request_trace_id(request_id.0.clone(), next.run(request)).await;
    response.headers_mut().insert(
        axum::http::header::HeaderName::from_static("x-request-id"),
        axum::http::HeaderValue::from_str(&request_id.0)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
    );

    response
}

#[cfg(test)]
mod tests {
    use super::resolve_request_id;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn keeps_valid_upstream_request_id() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req-123"));
        assert_eq!(resolve_request_id(&headers), "req-123");
    }

    #[test]
    fn generates_uuid_when_header_missing() {
        let headers = HeaderMap::new();
        let request_id = resolve_request_id(&headers);
        assert!(!request_id.is_empty());
        assert_eq!(request_id.len(), 36);
    }

    #[test]
    fn generates_uuid_when_header_too_long() {
        let mut headers = HeaderMap::new();
        let too_long = "x".repeat(200);
        headers.insert(
            "x-request-id",
            HeaderValue::from_str(&too_long).expect("valid header value"),
        );

        let request_id = resolve_request_id(&headers);
        assert_ne!(request_id, too_long);
        assert_eq!(request_id.len(), 36);
    }
}
