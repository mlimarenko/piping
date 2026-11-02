use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use tracing::{error, info};

use crate::errors::{AppError, http_error::map_app_error_to_http};

/// Middleware to map AppError into HTTP responses
pub async fn error_middleware(request: Request, next: Next) -> Response {
    // If response has an error status, try to extract AppError from extensions
    // Response mapping stays explicit in handlers and the shared error handler layer.
    next.run(request).await
}

/// Convert AppError to HTTP response
pub fn handle_app_error(error: AppError) -> Response {
    let error_display = error.to_string();
    let response = map_app_error_to_http(error);
    let status = response.status();

    // One tracing macro per function: the expanded `tracing` callsite carries
    // enough control flow that two of them in one body push clippy's
    // `cognitive_complexity` over the threshold.
    if status.is_server_error() {
        log_server_app_error(status, &error_display);
    } else {
        log_client_app_error(status, &error_display);
    }

    response
}

fn log_server_app_error(status: StatusCode, error_display: &str) {
    error!(
        target: "piping_backend::api::errors",
        %status,
        error = %error_display,
        "request handler returned AppError",
    );
}

fn log_client_app_error(status: StatusCode, error_display: &str) {
    info!(
        target: "piping_backend::api::errors",
        %status,
        error = %error_display,
        "request handler returned AppError",
    );
}
