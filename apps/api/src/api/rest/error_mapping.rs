use std::future::Future;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{
    contexts::shared::domain::reason_codes,
    errors::{AppError, normalize_database_error},
    observability::current_trace_and_span_ids,
};

tokio::task_local! {
    static REQUEST_TRACE_ID: String;
}

#[derive(Debug)]
struct RestErrorMetadata {
    status: StatusCode,
    code: &'static str,
    message: String,
    reason_code: Option<String>,
    details: Option<serde_json::Value>,
}

/// Error envelope returned by all REST error responses.
///
/// Every 4XX/5XX response body serialises to this shape. `reason_code` is
/// always present (defaults to `code` when no domain-specific code applies);
/// `trace_id` is omitted when unavailable.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiErrorResponse {
    /// Machine-readable error code (e.g. `"VALIDATION_ERROR"`, `"NOT_FOUND"`).
    pub code: String,
    /// Human-readable description of the error.
    pub message: String,
    /// Domain-specific reason code for programmatic handling.
    pub reason_code: String,
    /// Distributed trace ID, present when telemetry is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Additional structured error details for UI-specific recovery flows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

pub async fn scope_request_trace_id<T, F>(trace_id: String, future: F) -> T
where
    F: Future<Output = T>,
{
    REQUEST_TRACE_ID.scope(trace_id, future).await
}

pub fn build_error_response(
    status: StatusCode,
    code: impl Into<String>,
    message: impl Into<String>,
    reason_code: impl Into<String>,
) -> Response {
    build_error_response_with_details(status, code, message, reason_code, None)
}

pub fn build_error_response_with_details(
    status: StatusCode,
    code: impl Into<String>,
    message: impl Into<String>,
    reason_code: impl Into<String>,
    details: Option<serde_json::Value>,
) -> Response {
    let envelope = ApiErrorResponse {
        code: code.into(),
        message: message.into(),
        reason_code: reason_code.into(),
        trace_id: current_http_trace_id(),
        details,
    };
    (status, Json(envelope)).into_response()
}

pub fn map_app_error_to_http(error: AppError) -> Response {
    let metadata = rest_error_metadata(error);
    build_error_response_with_details(
        metadata.status,
        metadata.code,
        metadata.message,
        metadata
            .reason_code
            .unwrap_or_else(|| metadata.code.to_string()),
        metadata.details,
    )
}

/// The message this error is allowed to show a client.
///
/// For infrastructure failures (`Database`, `Redis`, `Config`, `Internal`) the
/// canonical REST mapping deliberately replaces the underlying text with a
/// generic one, because the raw text is a driver-level string that can name
/// tables, columns, constraints, and connection endpoints. Any transport that
/// reports errors to a client — REST or MCP — must use this rather than
/// `AppError::to_string`, so the redaction boundary lives in exactly one place.
pub fn client_safe_message(error: AppError) -> String {
    rest_error_metadata(error).message
}

fn current_http_trace_id() -> Option<String> {
    let (trace_id, span_id) = current_trace_and_span_ids();
    REQUEST_TRACE_ID
        .try_with(Clone::clone)
        .ok()
        .or(trace_id)
        .or(span_id)
}

fn rest_error_metadata(error: AppError) -> RestErrorMetadata {
    let error = normalize_database_error(error);
    match error {
        AppError::LicenseIssuerDenied(reason) => RestErrorMetadata {
            status: StatusCode::from_u16(reason.http_status()).unwrap_or(StatusCode::FORBIDDEN),
            code: reason.code(),
            message: reason.message().to_string(),
            reason_code: Some(reason.code().to_string()),
            details: None,
        },
        AppError::LicenseResponseInvalid => RestErrorMetadata {
            status: StatusCode::BAD_GATEWAY,
            code: "CONTROL_PLANE_RESPONSE_INVALID",
            message: "The licensing service returned an unrecognized response.".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::LicenseEntitlementInvalid(message) => RestErrorMetadata {
            status: StatusCode::BAD_REQUEST,
            code: "VALIDATION_ERROR",
            message,
            reason_code: Some("ENTITLEMENT_INVALID".to_string()),
            details: None,
        },
        AppError::LicenseRefreshSuperseded => RestErrorMetadata {
            status: StatusCode::CONFLICT,
            code: "CONFLICT",
            message: "License refresh superseded by a newer refresh or activation.".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::Config(_) | AppError::ConfigSource(_) => RestErrorMetadata {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "CONFIG_ERROR",
            message: "Configuration error".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::Database(_) | AppError::DatabaseSource(_) => RestErrorMetadata {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "DATABASE_ERROR",
            message: "Database error".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::DatabaseUnavailable(_) => RestErrorMetadata {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "DATABASE_UNAVAILABLE",
            message: "Database temporarily unavailable".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::Redis(_) | AppError::RedisSource(_) => RestErrorMetadata {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "REDIS_ERROR",
            message: "Redis error".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::Auth(message) => {
            let (code, message, reason_code) = map_auth_error_message(&message);
            RestErrorMetadata {
                status: StatusCode::UNAUTHORIZED,
                code,
                message,
                reason_code,
                details: None,
            }
        }
        AppError::Validation(message) => {
            let (mapped_message, reason_code) = parse_embedded_reason_code(&message);
            RestErrorMetadata {
                status: StatusCode::BAD_REQUEST,
                code: "VALIDATION_ERROR",
                message: mapped_message,
                reason_code,
                details: None,
            }
        }
        AppError::ValidationJson(_) => RestErrorMetadata {
            status: StatusCode::BAD_REQUEST,
            code: "VALIDATION_ERROR",
            message: "Invalid JSON payload".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::ValidationUuid(_) => RestErrorMetadata {
            status: StatusCode::BAD_REQUEST,
            code: "VALIDATION_ERROR",
            message: "Invalid UUID".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::NotFound(message) => {
            let (mapped_message, reason_code) = parse_embedded_reason_code(&message);
            RestErrorMetadata {
                status: StatusCode::NOT_FOUND,
                code: "NOT_FOUND",
                message: mapped_message,
                reason_code,
                details: None,
            }
        }
        AppError::Forbidden(message) => {
            let (code, message, reason_code) = map_forbidden_error_message(&message);
            RestErrorMetadata {
                status: StatusCode::FORBIDDEN,
                code,
                message,
                reason_code,
                details: None,
            }
        }
        AppError::External(message) => RestErrorMetadata {
            status: StatusCode::BAD_GATEWAY,
            code: "EXTERNAL_ERROR",
            message,
            reason_code: None,
            details: None,
        },
        AppError::ExternalSource(_) => RestErrorMetadata {
            status: StatusCode::BAD_GATEWAY,
            code: "EXTERNAL_ERROR",
            message: "External service error".to_string(),
            reason_code: None,
            details: None,
        },
        AppError::Conflict(message) => {
            let (mapped_message, reason_code) = parse_embedded_reason_code(&message);
            RestErrorMetadata {
                status: StatusCode::CONFLICT,
                code: "CONFLICT",
                message: mapped_message,
                reason_code,
                details: None,
            }
        }
        AppError::DeleteBlocked(details) => RestErrorMetadata {
            status: StatusCode::CONFLICT,
            code: "DELETE_BLOCKED",
            message: "Cannot delete this record while related objects still exist".to_string(),
            reason_code: Some(reason_codes::DELETE_BLOCKED.to_string()),
            details: serde_json::to_value(details).ok(),
        },
        AppError::PayloadTooLarge(message) => RestErrorMetadata {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "PAYLOAD_TOO_LARGE",
            message,
            reason_code: None,
            details: None,
        },
        AppError::TooManyRequests(message) => RestErrorMetadata {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "TOO_MANY_REQUESTS",
            message,
            reason_code: None,
            details: None,
        },
        AppError::Internal(message) => RestErrorMetadata {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL_ERROR",
            message,
            reason_code: None,
            details: None,
        },
        AppError::InvalidConfig(message) => RestErrorMetadata {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "INVALID_CONFIG",
            message,
            reason_code: None,
            details: None,
        },
        AppError::MissingSecret(message) => RestErrorMetadata {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "MISSING_SECRET",
            message,
            reason_code: None,
            details: None,
        },
        AppError::Provider(message) => RestErrorMetadata {
            status: StatusCode::BAD_GATEWAY,
            code: "PROVIDER_ERROR",
            message,
            reason_code: None,
            details: None,
        },
    }
}

fn map_auth_error_message(message: &str) -> (&'static str, String, Option<String>) {
    match message {
        "TOKEN_INVALID" => ("TOKEN_INVALID", "Invalid API token".to_string(), None),
        "TOKEN_REVOKED" => ("TOKEN_REVOKED", "API token is revoked".to_string(), None),
        "TOKEN_EXPIRED" => ("TOKEN_EXPIRED", "API token is expired".to_string(), None),
        other => {
            let (mapped_message, reason_code) = parse_embedded_reason_code(other);
            ("401", mapped_message, reason_code)
        }
    }
}

fn map_forbidden_error_message(message: &str) -> (&'static str, String, Option<String>) {
    if let Some(detail) = message.strip_prefix("LICENSE_SEAT_LIMIT_EXCEEDED:") {
        return (
            "LICENSE_SEAT_LIMIT_EXCEEDED",
            detail.trim().to_string(),
            Some(reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED.to_string()),
        );
    }
    if let Some(detail) = message.strip_prefix("LICENSE_REFRESH_REQUIRED:") {
        return (
            "LICENSE_REFRESH_REQUIRED",
            detail.trim().to_string(),
            Some(reason_codes::ENTITLEMENT_EXPIRED.to_string()),
        );
    }
    if let Some(detail) = message.strip_prefix("LICENSE_GRACE_ENDED:") {
        return (
            "LICENSE_GRACE_ENDED",
            detail.trim().to_string(),
            Some(reason_codes::ENTITLEMENT_EXPIRED.to_string()),
        );
    }
    if let Some(detail) = message.strip_prefix("LICENSE_FINGERPRINT_REBIND_DENIED:") {
        return (
            "LICENSE_FINGERPRINT_REBIND_DENIED",
            detail.trim().to_string(),
            None,
        );
    }
    if let Some(detail) = message.strip_prefix("TOKEN_SCOPE_DENIED:") {
        return ("TOKEN_SCOPE_DENIED", detail.trim().to_string(), None);
    }
    let (mapped_message, reason_code) = parse_embedded_reason_code(message);
    ("403", mapped_message, reason_code)
}

fn parse_embedded_reason_code(message: &str) -> (String, Option<String>) {
    let Some((raw_code, raw_message)) = message.split_once(':') else {
        return (message.to_string(), None);
    };
    let reason_code = raw_code.trim();
    let mapped_message = raw_message.trim();
    if mapped_message.is_empty() {
        return (message.to_string(), None);
    }
    if reason_codes::is_canonical_reason_code(reason_code) || is_endpoint_reason_code(reason_code) {
        return (mapped_message.to_string(), Some(reason_code.to_string()));
    }

    (message.to_string(), None)
}

fn is_endpoint_reason_code(reason_code: &str) -> bool {
    reason_code.starts_with("endpoint_")
        && reason_code
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::Value;
    use uuid::Uuid;

    use super::{build_error_response, map_app_error_to_http, parse_embedded_reason_code};
    use crate::{
        contexts::shared::domain::reason_codes,
        errors::{AppError, DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup},
    };

    #[test]
    fn parses_endpoint_reason_code_from_validation_message() {
        let (message, reason_code) = parse_embedded_reason_code(
            "endpoint_not_bound_to_target: Connection endpoint is not bound",
        );
        assert_eq!(message, "Connection endpoint is not bound");
        assert_eq!(reason_code.as_deref(), Some("endpoint_not_bound_to_target"));
    }

    #[test]
    fn parses_canonical_reason_code_from_validation_message() {
        let (message, reason_code) =
            parse_embedded_reason_code("SEAT_VERSION_CONFLICT: Seat mutation requires refresh");
        assert_eq!(message, "Seat mutation requires refresh");
        assert_eq!(
            reason_code.as_deref(),
            Some(reason_codes::SEAT_VERSION_CONFLICT)
        );
    }

    #[test]
    fn ignores_non_reason_prefixes() {
        let (message, reason_code) = parse_embedded_reason_code("validation: Invalid payload");
        assert_eq!(message, "validation: Invalid payload");
        assert!(reason_code.is_none());
    }

    #[tokio::test]
    async fn mapped_http_error_uses_top_level_reason_code_and_trace_id() {
        let response = super::scope_request_trace_id("req-123".to_string(), async {
            map_app_error_to_http(AppError::Validation(
                "SEAT_VERSION_CONFLICT: refresh and retry".to_string(),
            ))
        })
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("body should be readable");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(payload["code"], "VALIDATION_ERROR");
        assert_eq!(payload["message"], "refresh and retry");
        assert_eq!(payload["reason_code"], reason_codes::SEAT_VERSION_CONFLICT);
        assert_eq!(payload["trace_id"], "req-123");
    }

    #[tokio::test]
    async fn explicit_error_builder_emits_canonical_envelope() {
        let response = super::scope_request_trace_id("req-456".to_string(), async {
            build_error_response(
                StatusCode::FORBIDDEN,
                "LICENSE_SEAT_LIMIT_EXCEEDED",
                "Seat limit reached",
                reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED,
            )
        })
        .await;

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(
            payload["reason_code"],
            reason_codes::ENTITLEMENT_SEAT_LIMIT_REACHED
        );
        assert_eq!(payload["trace_id"], "req-456");
    }

    #[tokio::test]
    async fn transfer_policy_errors_map_to_canonical_status_codes() {
        let too_large = map_app_error_to_http(AppError::PayloadTooLarge(
            "File exceeds configured limit".to_string(),
        ));
        assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let payload: Value = serde_json::from_slice(
            &axum::body::to_bytes(too_large.into_body(), usize::MAX)
                .await
                .expect("body should be readable"),
        )
        .expect("valid json");
        assert_eq!(payload["code"], "PAYLOAD_TOO_LARGE");
        assert_eq!(payload["reason_code"], "PAYLOAD_TOO_LARGE");

        let saturated = map_app_error_to_http(AppError::TooManyRequests(
            "Too many SSH file operations".to_string(),
        ));
        assert_eq!(saturated.status(), StatusCode::TOO_MANY_REQUESTS);
        let payload: Value = serde_json::from_slice(
            &axum::body::to_bytes(saturated.into_body(), usize::MAX)
                .await
                .expect("body should be readable"),
        )
        .expect("valid json");
        assert_eq!(payload["code"], "TOO_MANY_REQUESTS");
        assert_eq!(payload["reason_code"], "TOO_MANY_REQUESTS");
    }

    #[tokio::test]
    async fn a_text_only_unique_name_error_maps_to_user_facing_conflict() {
        let response = map_app_error_to_http(AppError::Database(
            "Failed to update connection-config record: error returned from database: duplicate key value violates unique constraint \"connconf_record_name_key\"".to_string(),
        ));
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(payload["code"], "CONFLICT");
        assert_eq!(payload["message"], "Name already exists");
        assert_eq!(payload["reason_code"], "CONFLICT");
    }

    #[tokio::test]
    async fn a_text_only_foreign_key_error_maps_to_validation() {
        let response = map_app_error_to_http(AppError::Database(
            "Failed to assign record: error returned from database: insert or update on table \"connconf_assignment\" violates foreign key constraint \"connconf_assignment_record_id_fkey\"".to_string(),
        ));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(payload["code"], "VALIDATION_ERROR");
        assert_eq!(
            payload["message"],
            "Referenced resource does not exist or cannot be used here"
        );
        assert_eq!(payload["reason_code"], "VALIDATION_ERROR");
    }

    #[tokio::test]
    async fn a_text_only_pool_timeout_maps_to_database_unavailable() {
        let response = map_app_error_to_http(AppError::Database(
            "Failed to list records: pool timed out while waiting for an open connection"
                .to_string(),
        ));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(payload["code"], "DATABASE_UNAVAILABLE");
        assert_eq!(payload["message"], "Database temporarily unavailable");
        assert_eq!(payload["reason_code"], "DATABASE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn delete_blocked_error_maps_to_structured_conflict_payload() {
        let device_id =
            Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").expect("valid uuid");
        let response = map_app_error_to_http(AppError::DeleteBlocked(DeleteBlockedDetails {
            total_count: 2,
            blockers: vec![DeleteBlockedGroup {
                table: "dcim_device".to_string(),
                entity_key: Some("Devices".to_string()),
                count: 2,
                examples: vec![DeleteBlockedExample {
                    id: Some(device_id),
                    label: "router01".to_string(),
                }],
            }],
        }));
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(payload["code"], "DELETE_BLOCKED");
        assert_eq!(payload["reason_code"], reason_codes::DELETE_BLOCKED);
        assert_eq!(payload["details"]["total_count"], 2);
        assert_eq!(payload["details"]["blockers"][0]["table"], "dcim_device");
        assert_eq!(payload["details"]["blockers"][0]["entity_key"], "Devices");
        assert_eq!(
            payload["details"]["blockers"][0]["examples"][0]["id"],
            device_id.to_string()
        );
        assert_eq!(
            payload["details"]["blockers"][0]["examples"][0]["label"],
            "router01"
        );
    }
}
