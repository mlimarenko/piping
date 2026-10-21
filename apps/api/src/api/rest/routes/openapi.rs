use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
};

use crate::app::AppState;

/// Canonical OpenAPI YAML for the REST API.
///
/// Source file: `apps/api/contracts/piping.openapi.yaml`. Embedded at compile
/// time so the binary is the single artifact operators ship.
const OPENAPI_YAML: &str = include_str!("../../../../contracts/piping.openapi.yaml");

/// GET /api/v1/openapi/piping.openapi.yaml
pub async fn openapi_handler(State(_): State<AppState>) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/yaml; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    (StatusCode::OK, headers, OPENAPI_YAML)
}
