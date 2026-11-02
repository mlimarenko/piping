use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};
use uuid::Uuid;

use crate::observability::metrics;

#[derive(Clone, Debug)]
pub struct RequestMetricsContext {
    pub method: String,
    pub normalized_route: String,
    pub started_at: Instant,
}

/// Middleware that records Prometheus HTTP metrics for each request.
pub async fn metrics_middleware(mut request: Request, next: Next) -> Response {
    let started_at = Instant::now();
    let method = request.method().as_str().to_string();
    let normalized_route = normalize_route(request.uri().path());
    let request_context = RequestMetricsContext {
        method: method.clone(),
        normalized_route: normalized_route.clone(),
        started_at,
    };
    request.extensions_mut().insert(request_context);

    metrics::inc_in_flight(&method, &normalized_route);
    let response = next.run(request).await;
    metrics::dec_in_flight(&method, &normalized_route);

    let status = response.status().as_u16().to_string();
    let duration_seconds = started_at.elapsed().as_secs_f64();
    metrics::observe_http_request(&method, &normalized_route, &status, duration_seconds);

    response
}

fn normalize_route(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }

    let mut normalized = String::new();
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        normalized.push('/');
        if Uuid::parse_str(segment).is_ok() {
            normalized.push_str(":uuid");
        } else if segment.chars().all(|char| char.is_ascii_digit()) {
            normalized.push_str(":id");
        } else {
            normalized.push_str(segment);
        }
    }

    if normalized.is_empty() {
        "/".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_route;

    #[test]
    fn normalize_route_replaces_dynamic_segments() {
        let uuid = "3f1639f6-bf95-4cad-b697-4cbe2d3766fb";
        let route = format!("/api/v1/connections/targets/{uuid}/details/42");
        assert_eq!(
            normalize_route(&route),
            "/api/v1/connections/targets/:uuid/details/:id"
        );
    }
}
