//! Extractors that keep extractor-level rejections inside the documented API
//! error envelope.
//!
//! Axum's stock `Json`/`Query` extractors answer malformed input with a
//! `text/plain` body, which breaks the OpenAPI contract promise that every 4XX
//! carries an [`crate::api::rest::error_mapping::ApiErrorResponse`]. The
//! wrappers here forward to the stock extractors and convert their rejections
//! through the same [`AppError`] mapping every handler-level error takes.
//!
//! [`RequireAuthenticated`] additionally lets a handler run its
//! caller-is-authenticated check during the extraction phase — i.e. BEFORE a
//! following body/query extractor touches the payload — so an anonymous caller
//! receives the standard 403 envelope instead of body-shape diagnostics.

use axum::{
    Extension,
    extract::{FromRequest, FromRequestParts, Json, Path as AxumPath, Query as AxumQuery, Request},
    http::request::Parts,
    response::Response,
};
use serde::de::DeserializeOwned;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::errors::AppError;

/// JSON body extractor whose rejection is the standard API error envelope.
pub struct EnvelopeJson<T>(pub T);

impl<S, T> FromRequest<S> for EnvelopeJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(handle_app_error(AppError::Validation(
                rejection.body_text(),
            ))),
        }
    }
}

/// Query-string extractor whose rejection is the standard API error envelope.
///
/// Deliberately named `Query` (shadowing `axum::extract::Query` at the call
/// site): utoipa's `axum_extras` feature resolves `parameter_in` for
/// `#[utoipa::path]` by matching the LAST SEGMENT of the extractor's type
/// name against `"Query"`/`"Path"`. Any other name silently flips every
/// derived parameter to `in: path` in the generated OpenAPI document (the
/// `committed_openapi_is_generated_from_rest_annotations` gate catches this).
/// `EnvelopeJson` keeps its distinct name because request bodies are always
/// declared explicitly via `request_body` and axum's `Json` stays in scope
/// for building responses.
pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match AxumQuery::<T>::from_request_parts(parts, state).await {
            Ok(AxumQuery(value)) => Ok(Self(value)),
            Err(rejection) => Err(handle_app_error(AppError::Validation(
                rejection.body_text(),
            ))),
        }
    }
}

/// Path-parameter extractor whose rejection is the standard API error
/// envelope.
///
/// Deliberately named `Path` (shadowing `axum::extract::Path` at the call
/// site) for the same utoipa `axum_extras` reason as [`Query`] above: the
/// LAST SEGMENT of the extractor's type name decides `parameter_in`.
///
/// `Path<String>` extraction is infallible, so for the majority of handlers
/// this is behaviorally identical to axum's extractor. It exists for the
/// typed parameters (`Path<Uuid>`, tuples with `i32`), whose parse failure
/// used to answer with axum's `text/plain` 400 instead of the documented
/// envelope. All route modules import this one — enforced by
/// `compliance_envelope_path_extractor` — so the fallible/infallible
/// distinction never has to be re-litigated per call site.
pub struct Path<T>(pub T);

impl<S, T> FromRequestParts<S> for Path<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match AxumPath::<T>::from_request_parts(parts, state).await {
            Ok(AxumPath(value)) => Ok(Self(value)),
            Err(rejection) => Err(handle_app_error(AppError::Validation(
                rejection.body_text(),
            ))),
        }
    }
}

/// Authentication gate that rejects anonymous callers with the standard 403
/// envelope during extraction, before any body/query extractor that follows it
/// in the handler signature. Fine-grained per-target RBAC still runs in the
/// handler prologue; this only fixes the rejection ORDER for anonymous calls.
///
/// # Why "not anonymous" and not "has a user session"
///
/// The safe form of an extraction-phase pre-gate rejects only a *subset* of
/// what the handler's own authorization rejects — otherwise it would deny a
/// caller the handler would have served. Every non-public REST module already
/// denies anonymous callers (deny-by-default RBAC, enforced by the
/// `compliance_handler_authz` gate), so a gate that rejects *only* the
/// anonymous principal never changes an outcome the handler wouldn't have
/// reached anyway.
///
/// A `user_id`-only check would be narrower than that: a **playbook job token**
/// (see `auth_middleware`) is a legitimate authenticated principal that carries
/// an `api_token_id` but no `user_id`, and it is the auth method for the entire
/// `/api/v1/inventory/*` surface. Gating those routes on `user_id` would 403 a
/// valid job token. This gate therefore keys off "not anonymous"
/// (`user_id.is_some() || api_token_id.is_some()`) — the coarse authentication
/// boundary — leaving any principal-specific decision to the handler prologue.
///
/// This is intentionally distinct from the `require_authenticated_user` helper
/// used inside some handlers, which is `user_id`-based: that helper is a
/// handler-level authorization choice, whereas this extractor is only the
/// coarse "some authenticated principal reached us" pre-gate.
///
/// The auth middleware always inserts an [`AuthContext`] (anonymous for
/// unauthenticated requests) on routes it guards, so the gate keys off the
/// principal fields rather than the extension's presence. A missing extension
/// (an auth-skipped endpoint, e.g. the monitoring-token or playbook job-token
/// callbacks) is treated as anonymous and rejected — such endpoints must not
/// use this gate. The wrapped value is exposed as `Extension<AuthContext>` so
/// call sites can hand it to
/// [`crate::api::rest::request_context::build_request_context`] unchanged.
pub struct RequireAuthenticated(pub Extension<AuthContext>);

impl<S> FromRequestParts<S> for RequireAuthenticated
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<AuthContext>() {
            Some(context) if context.user_id.is_some() || context.api_token_id.is_some() => {
                Ok(Self(Extension(context.clone())))
            }
            // 401, not 403: this gate fires when NO authenticated principal
            // reached us, which is "who are you?", not "you may not do that".
            // The handler-level `require_authenticated_user` helper has always
            // answered 401 for the same condition, so a caller's status
            // depended on which of the two a route happened to use.
            _ => Err(handle_app_error(AppError::Auth(
                "Authentication required".to_string(),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::extract::{FromRequest, FromRequestParts, Request};
    use axum::http::request::Parts;
    use axum::http::{StatusCode, header::CONTENT_TYPE};
    use axum::response::Response;
    use serde::Deserialize;
    use serde_json::Value;

    use super::{EnvelopeJson, Query, RequireAuthenticated};
    use crate::api::middleware::auth::AuthContext;

    #[derive(Debug, Deserialize)]
    struct BodyProbe {
        name: String,
    }

    #[derive(Debug, Deserialize)]
    struct QueryProbe {
        count: u32,
    }

    async fn envelope_of(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("rejection body should be readable");
        let payload: Value = serde_json::from_slice(&bytes)
            .expect("extractor rejection must be a JSON error envelope, not text/plain");
        (status, payload)
    }

    fn parts_with(context: Option<AuthContext>) -> Parts {
        let request = Request::builder()
            .body(Body::empty())
            .expect("request builds");
        let (mut parts, _body) = request.into_parts();
        if let Some(context) = context {
            parts.extensions.insert(context);
        }
        parts
    }

    #[tokio::test]
    async fn malformed_json_body_rejects_with_error_envelope() {
        let request = Request::builder()
            .method("POST")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{ definitely not valid json"))
            .expect("request builds");

        let rejection = EnvelopeJson::<BodyProbe>::from_request(request, &())
            .await
            .err()
            .expect("malformed JSON must be rejected");

        let (status, payload) = envelope_of(rejection).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["code"], "VALIDATION_ERROR");
        assert_eq!(payload["reason_code"], "VALIDATION_ERROR");
        assert!(payload["message"].is_string());
    }

    #[tokio::test]
    async fn missing_required_field_rejects_with_error_envelope() {
        let request = Request::builder()
            .method("POST")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .expect("request builds");

        let rejection = EnvelopeJson::<BodyProbe>::from_request(request, &())
            .await
            .err()
            .expect("a body missing a required field must be rejected");

        let (status, payload) = envelope_of(rejection).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["code"], "VALIDATION_ERROR");
    }

    #[tokio::test]
    async fn malformed_query_rejects_with_error_envelope() {
        let request = Request::builder()
            .uri("/?count=not-a-number")
            .body(Body::empty())
            .expect("request builds");
        let (mut parts, _body) = request.into_parts();

        let rejection = Query::<QueryProbe>::from_request_parts(&mut parts, &())
            .await
            .err()
            .expect("a query that fails to deserialize must be rejected");

        let (status, payload) = envelope_of(rejection).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["code"], "VALIDATION_ERROR");
    }

    #[tokio::test]
    async fn well_formed_body_deserializes_through_envelope() {
        let request = Request::builder()
            .method("POST")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"probe"}"#))
            .expect("request builds");

        let EnvelopeJson(probe) = EnvelopeJson::<BodyProbe>::from_request(request, &())
            .await
            .expect("a well-formed body must deserialize");
        assert_eq!(probe.name, "probe");
    }

    #[tokio::test]
    async fn well_formed_query_deserializes_through_envelope() {
        let request = Request::builder()
            .uri("/?count=3")
            .body(Body::empty())
            .expect("request builds");
        let (mut parts, _body) = request.into_parts();

        let Query(probe) = Query::<QueryProbe>::from_request_parts(&mut parts, &())
            .await
            .expect("a well-formed query must deserialize");
        assert_eq!(probe.count, 3);
    }

    #[tokio::test]
    async fn require_authenticated_rejects_absent_context() {
        let mut parts = parts_with(None);
        let rejection = RequireAuthenticated::from_request_parts(&mut parts, &())
            .await
            .err()
            .expect("a request with no auth context must be rejected");
        assert_eq!(rejection.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_authenticated_rejects_anonymous_context() {
        let mut parts = parts_with(Some(AuthContext::anonymous()));
        let rejection = RequireAuthenticated::from_request_parts(&mut parts, &())
            .await
            .err()
            .expect("an anonymous auth context must be rejected");
        assert_eq!(rejection.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_authenticated_admits_session_user() {
        let context = AuthContext {
            user_id: Some(uuid::Uuid::nil()),
            auth_method: "session".to_string(),
            ..AuthContext::anonymous()
        };
        let mut parts = parts_with(Some(context));
        RequireAuthenticated::from_request_parts(&mut parts, &())
            .await
            .expect("a session-authenticated caller must be admitted");
    }

    #[tokio::test]
    async fn require_authenticated_admits_job_token_without_user_id() {
        // A playbook job token is authenticated but carries no user_id. This is
        // the regression guard: a user_id-only gate would 403 the entire
        // /api/v1/inventory/* surface. "Not anonymous" must admit it.
        let context = AuthContext {
            user_id: None,
            auth_method: "playbook_job_token".to_string(),
            api_token_id: Some("job-token-1".to_string()),
            ..AuthContext::anonymous()
        };
        let mut parts = parts_with(Some(context));
        RequireAuthenticated::from_request_parts(&mut parts, &())
            .await
            .expect("an authenticated machine principal (job token) must be admitted");
    }
}
