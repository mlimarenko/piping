use axum::{
    Json,
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Cloud's instances routes enforce this boundary before extracting their payload.
pub(crate) async fn require_cloud_csrf(request: Request<Body>, next: Next) -> Response {
    if request
        .headers()
        .get("x-piping-csrf")
        .and_then(|value| value.to_str().ok())
        != Some("1")
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "code": "CSRF_HEADER_MISSING",
                "reasonCode": "CSRF_HEADER_MISSING",
                "message": "Missing or invalid X-Piping-CSRF header"
            })),
        )
            .into_response();
    }
    next.run(request).await
}
