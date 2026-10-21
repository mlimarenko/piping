use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};

use crate::api::client_ip::extract_client_ip;
use crate::api::middleware::metrics::RequestMetricsContext;
use crate::api::middleware::request_id::RequestId;
use crate::observability::{
    EventFields, LogLevel, attach_parent_context_from_headers, http_request_span, instrument_span,
    log_error_event, log_event, log_operation_completion,
};

/// Middleware to create per-request tracing span and record request metadata
pub async fn tracing_middleware(request: Request, next: Next) -> Response {
    let fallback_started_at = Instant::now();
    let path = request.uri().path().to_string();
    let request_method = request.method().as_str().to_string();
    let metrics_context = request.extensions().get::<RequestMetricsContext>().cloned();
    let method = metrics_context
        .as_ref()
        .map(|value| value.method.clone())
        .unwrap_or(request_method);
    let route = metrics_context
        .as_ref()
        .map(|value| value.normalized_route.clone())
        .unwrap_or_else(|| path.clone());
    // Budget keys are map keys held for the process lifetime, so they must come
    // from a bounded set. `route` falls back to the raw path when no route
    // matched, and a scanner walking random URLs would then add one permanent
    // entry per URL. The raw path stays on the event for diagnosis; only the
    // key is collapsed.
    let route_noise_key = if metrics_context.is_some() {
        route.clone()
    } else {
        "unmatched".to_string()
    };
    let is_health_request = is_health_route(&route);
    let started_at = metrics_context
        .map(|value| value.started_at)
        .unwrap_or(fallback_started_at);
    let headers = request.headers();

    // Extract request_id from extensions if available
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .cloned()
        .map(|id| id.0)
        .unwrap_or_else(|| "unknown".to_string());
    let client_ip = extract_client_ip(headers);

    // Create per-request span
    let span = http_request_span(&request_id, &method, &route, client_ip.as_deref());
    attach_parent_context_from_headers(&span, headers);

    let receive_fields = EventFields::default()
        .with_correlation_id(request_id.clone())
        .with_operation("http.request")
        .with_resource("http_route", route.clone())
        .with_noise_key(format!("http.request.received.{method}.{route_noise_key}"))
        .with_context("method", method.clone())
        .with_context("path", path.clone())
        .with_context(
            "client_ip",
            client_ip.clone().unwrap_or_else(|| "unknown".to_string()),
        );
    {
        let _guard = span.enter();
        log_event(LogLevel::Debug, "http.request.received", receive_fields);
    }

    // Execute request handler within span
    let response = instrument_span(span.clone(), next.run(request)).await;
    let status = response.status().as_u16();

    if status >= 500 {
        let error_fields = EventFields::default()
            .with_correlation_id(request_id)
            .with_operation("http.request")
            .with_resource("http_route", route)
            .with_noise_key(format!("http.request.failed.{method}.{status}"))
            .with_status(status.to_string())
            .with_context("method", method)
            .with_context("path", path);
        {
            let _guard = span.enter();
            log_error_event(
                "http.request.failed",
                "http_server_error",
                format!("request completed with HTTP {}", status),
                error_fields.with_duration_ms(started_at.elapsed().as_secs_f64() * 1000.0),
            );
        }
        return response;
    }

    let complete_level = if status >= 400 {
        LogLevel::Warning
    } else if is_health_request {
        LogLevel::Debug
    } else {
        LogLevel::Info
    };
    let complete_fields = EventFields::default()
        .with_correlation_id(request_id)
        .with_operation("http.request")
        .with_resource("http_route", route)
        .with_noise_key(format!(
            "http.request.completed.{method}.{}",
            http_status_family(status)
        ))
        .with_context("method", method)
        .with_context("path", path);
    {
        let _guard = span.enter();
        log_operation_completion(
            complete_level,
            "http.request.completed",
            status.to_string(),
            started_at,
            complete_fields,
        );
    }

    response
}

fn is_health_route(route: &str) -> bool {
    matches!(
        route,
        "/health" | "/api/v1/health" | "/api/v1/live" | "/api/v1/ready"
    )
}

fn http_status_family(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}
