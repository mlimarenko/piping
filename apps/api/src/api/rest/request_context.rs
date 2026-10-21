use axum::{
    Extension,
    extract::FromRequestParts,
    http::{HeaderMap, request::Parts},
    response::Response,
};

use crate::api::client_ip::extract_client_ip;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::contexts::shared::request_context::{RequestAuth, RequestContext};
use crate::errors::AppError;

pub fn build_request_context(
    request_id: &RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: &HeaderMap,
) -> RequestContext {
    let auth_context = auth_context
        .map(|value| value.0)
        .unwrap_or_else(AuthContext::anonymous);
    let client_ip = extract_client_ip(headers);
    let api_token_id = auth_context.api_token_id.clone();
    let mut request_context = RequestContext::new(
        auth_context.user_id,
        auth_context.auth_method,
        client_ip,
        request_id.0.clone(),
        RequestAuth {
            oidc_groups: auth_context.oidc_groups,
            api_token_scopes: auth_context.api_token_scopes,
            api_token_scope_resource_override: auth_context.api_token_scope_resource_override,
        },
    );
    request_context.api_token_id = api_token_id;
    request_context
}

/// Extracts the request-scoped [`RequestContext`] as a single handler argument.
///
/// Bundles the request id, auth context and headers so a handler that also needs
/// a body extractor stays within the argument budget. `#[utoipa::path]` does not
/// treat it as a parameter, so the OpenAPI parameter surface is unchanged.
pub struct RequestContextExtractor(pub RequestContext);

impl<S> FromRequestParts<S> for RequestContextExtractor
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let request_id = parts
            .extensions
            .get::<RequestId>()
            .cloned()
            .ok_or_else(|| {
                handle_app_error(AppError::Internal(
                    "RequestId extension missing".to_string(),
                ))
            })?;
        let auth_context = parts
            .extensions
            .get::<AuthContext>()
            .cloned()
            .map(Extension);
        let headers = parts.headers.clone();
        Ok(Self(build_request_context(
            &request_id,
            auth_context,
            &headers,
        )))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::build_request_context;
    use crate::api::middleware::request_id::RequestId;

    #[test]
    fn build_request_context_uses_proxy_headers_for_request_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.25, 172.18.0.6"),
        );

        let request_context = build_request_context(&RequestId("rid".to_string()), None, &headers);

        assert_eq!(request_context.request_ip.as_deref(), Some("198.51.100.25"));
    }

    #[test]
    fn build_request_context_prefers_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.8"));
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.25, 172.18.0.6"),
        );

        let request_context = build_request_context(&RequestId("rid".to_string()), None, &headers);

        assert_eq!(request_context.request_ip.as_deref(), Some("203.0.113.8"));
    }
}
