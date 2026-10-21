use thiserror::Error;

use config::ConfigError;
use redis::RedisError;
use reqwest::Error as ReqwestError;
use serde::{Deserialize, Serialize};
use serde_json::Error as SerdeJsonError;
use sqlx::Error as SqlxError;
use sqlx::error::DatabaseError;
use uuid::Error as UuidError;
use uuid::Uuid;

use crate::config::ConfigValidationError;
use crate::contexts::access_control::domain::ResourceIdError;

/// Application error types
#[derive(Debug, Error)]
pub enum AppError {
    #[error("License issuer refusal: {0}")]
    LicenseIssuerDenied(crate::contexts::licensing::domain::issuer_refusal::IssuerRefusal),

    #[error("Invalid license control-plane response")]
    LicenseResponseInvalid,

    #[error("Invalid license entitlement: {0}")]
    LicenseEntitlementInvalid(String),

    #[error("License refresh superseded by a newer refresh or activation")]
    LicenseRefreshSuperseded,
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Database error (string-backed form, prefer DatabaseSource)
    #[error("Database error: {0}")]
    Database(String),

    /// Database error (typed)
    #[error("Database error: {0}")]
    DatabaseSource(#[from] SqlxError),

    /// Database is unavailable (pool exhausted/closed, connection/IO/TLS
    /// failure, or background worker crash). Maps to HTTP 503 so callers can
    /// treat it as a transient, retryable outage rather than a generic 500.
    #[error("Database unavailable: {0}")]
    DatabaseUnavailable(String),

    /// Redis error (string-backed form, prefer RedisSource)
    #[error("Redis error: {0}")]
    Redis(String),

    /// Redis error (typed)
    #[error("Redis error: {0}")]
    RedisSource(#[from] RedisError),

    /// Authentication error
    #[error("Authentication error: {0}")]
    Auth(String),

    /// Validation error
    #[error("Validation error: {0}")]
    Validation(String),

    /// Validation error (JSON)
    #[error("Validation error: {0}")]
    ValidationJson(#[from] SerdeJsonError),

    /// Validation error (UUID parsing)
    #[error("Validation error: {0}")]
    ValidationUuid(#[from] UuidError),

    /// Config error
    #[error("Configuration error: {0}")]
    ConfigSource(#[from] ConfigError),

    /// External request error
    #[error("External service error: {0}")]
    ExternalSource(#[from] ReqwestError),
    /// Invalid configuration error
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// Missing secret error
    #[error("Missing secret: {0}")]
    MissingSecret(String),

    /// Resource not found
    #[error("Resource not found: {0}")]
    NotFound(String),

    /// Forbidden access
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// External service error
    #[error("External service error: {0}")]
    External(String),

    /// Conflict error (e.g. concurrent operation)
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Delete is blocked by related records.
    #[error("Delete blocked by related records")]
    DeleteBlocked(DeleteBlockedDetails),

    /// Request payload is larger than the configured application policy.
    #[error("Payload too large: {0}")]
    PayloadTooLarge(String),

    /// Request could not start because a scoped resource limit is saturated.
    #[error("Too many requests: {0}")]
    TooManyRequests(String),

    /// Provider error
    #[error("Provider error: {0}")]
    Provider(String),

    /// Internal server error
    #[error("Internal server error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteBlockedDetails {
    pub total_count: i64,
    pub blockers: Vec<DeleteBlockedGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteBlockedGroup {
    pub table: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_key: Option<String>,
    pub count: i64,
    pub examples: Vec<DeleteBlockedExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteBlockedExample {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    pub label: String,
}

impl From<ConfigValidationError> for AppError {
    fn from(error: ConfigValidationError) -> Self {
        AppError::InvalidConfig(error.to_string())
    }
}

impl From<ResourceIdError> for AppError {
    fn from(error: ResourceIdError) -> Self {
        AppError::Validation(error.to_string())
    }
}

/// Classify a raw [`sqlx::Error`] into a semantic [`AppError`] so REST callers
/// get an accurate HTTP status instead of a blanket 500.
///
/// Mapping:
/// - unique / primary-key violation (SQLSTATE `23505`) -> [`AppError::Conflict`] (409)
/// - foreign-key / not-null / check violations -> [`AppError::Validation`] (400)
/// - exclusion/concurrent write conflicts -> [`AppError::Conflict`] (409)
/// - pool timeout/closed, connection/IO/TLS failure, worker crash ->
///   [`AppError::DatabaseUnavailable`] (503, retryable)
/// - everything else -> [`AppError::Database`] (500, generic)
///
/// `RowNotFound` is intentionally **not** mapped to 404 here: most callers use
/// `fetch_optional` and decide "missing" semantics themselves, so blindly
/// turning a stray `RowNotFound` into a 404 would mask real bugs. Repositories
/// that genuinely want lookup-miss-as-404 should map it explicitly at the call
/// site (see `map_provider_write_error` / `map_policy_binding_write_error` for
/// constraint-aware variants).
///
/// MIGRATION: the bulk of the codebase still builds DB errors via
/// `AppError::Database(format!("... {e}"))`, which throws away the sqlx
/// structure and collapses every failure to 500. New and touched repository
/// write paths should route their `sqlx::Error` through this classifier
/// (`.map_err(classify_sqlx_error_owned)` or `classify_sqlx_error(&e)`) so a
/// duplicate row surfaces as 409 and an outage surfaces as 503. Constraint-name
/// specific messages (e.g. "Provider name already exists") should still be
/// produced by per-repository mappers that fall back to this classifier.
#[must_use]
pub fn classify_sqlx_error(error: &SqlxError) -> AppError {
    match error {
        SqlxError::Database(db_error) => classify_database_error(db_error.as_ref()),
        SqlxError::Io(_)
        | SqlxError::Tls(_)
        | SqlxError::PoolTimedOut
        | SqlxError::PoolClosed
        | SqlxError::WorkerCrashed => {
            AppError::DatabaseUnavailable("Database temporarily unavailable".to_string())
        }
        other => AppError::Database(other.to_string()),
    }
}

/// Owned variant of [`classify_sqlx_error`] for ergonomic use in
/// `.map_err(classify_sqlx_error_owned)` where the error is moved.
#[must_use]
pub fn classify_sqlx_error_owned(error: SqlxError) -> AppError {
    classify_sqlx_error(&error)
}

/// Normalize database-backed [`AppError`] values that may have been produced by
/// older repository code before it adopted [`classify_sqlx_error`].
#[must_use]
pub fn normalize_database_error(error: AppError) -> AppError {
    match error {
        AppError::DatabaseSource(source) => classify_sqlx_error(&source),
        AppError::Database(message) => {
            classify_database_message_text(&message).unwrap_or(AppError::Database(message))
        }
        other => other,
    }
}

fn classify_database_error(db_error: &(dyn DatabaseError + Send + Sync + 'static)) -> AppError {
    match db_error.kind() {
        sqlx::error::ErrorKind::UniqueViolation => {
            AppError::Conflict(duplicate_message(db_error.constraint(), db_error.message()))
        }
        sqlx::error::ErrorKind::ForeignKeyViolation => AppError::Validation(
            "Referenced resource does not exist or cannot be used here".to_string(),
        ),
        sqlx::error::ErrorKind::NotNullViolation => {
            AppError::Validation("Required field is missing".to_string())
        }
        sqlx::error::ErrorKind::CheckViolation => {
            AppError::Validation("Value does not satisfy database constraints".to_string())
        }
        sqlx::error::ErrorKind::Other => classify_database_code(db_error.code().as_deref())
            .unwrap_or_else(|| AppError::Database(db_error.message().to_string())),
        _ => AppError::Database(db_error.message().to_string()),
    }
}

fn classify_database_code(code: Option<&str>) -> Option<AppError> {
    match code? {
        "23505" => Some(AppError::Conflict("Value already exists".to_string())),
        // ON DELETE RESTRICT raises 23001 (restrict_violation), not 23503, and
        // sqlx's ErrorKind does not cover it — without this arm a blocked
        // delete surfaces as a raw 500 instead of a conflict.
        "23001" => Some(AppError::Conflict(
            "Record is still referenced by other records".to_string(),
        )),
        "23503" => Some(AppError::Validation(
            "Referenced resource does not exist or cannot be used here".to_string(),
        )),
        "23502" => Some(AppError::Validation(
            "Required field is missing".to_string(),
        )),
        "23514" => Some(AppError::Validation(
            "Value does not satisfy database constraints".to_string(),
        )),
        "23P01" => Some(AppError::Conflict(
            "Record conflicts with an existing record".to_string(),
        )),
        "40001" | "40P01" => Some(AppError::Conflict(
            "Database write conflict. Please retry.".to_string(),
        )),
        _ => None,
    }
}

fn classify_database_message_text(message: &str) -> Option<AppError> {
    let lower = message.to_ascii_lowercase();
    if !looks_like_a_database_message(&lower) {
        return None;
    }
    if looks_like_database_unavailable(&lower) {
        return Some(AppError::DatabaseUnavailable(
            "Database temporarily unavailable".to_string(),
        ));
    }
    if lower.contains("23505")
        || lower.contains("unique constraint")
        || lower.contains("unique violation")
        || lower.contains("unique constraint failed")
        || lower.contains("duplicate entry")
        || lower.contains("duplicate key value")
    {
        return Some(AppError::Conflict(duplicate_message(None, message)));
    }
    if lower.contains("23503")
        || lower.contains("foreign key constraint")
        || lower.contains("foreign key constraint failed")
        || lower.contains("cannot add or update a child row")
    {
        return Some(AppError::Validation(
            "Referenced resource does not exist or cannot be used here".to_string(),
        ));
    }
    if lower.contains("23502")
        || lower.contains("not-null constraint")
        || lower.contains("not null constraint failed")
    {
        return Some(AppError::Validation(
            "Required field is missing".to_string(),
        ));
    }
    if lower.contains("23514")
        || lower.contains("check constraint")
        || lower.contains("check constraint failed")
    {
        return Some(AppError::Validation(
            "Value does not satisfy database constraints".to_string(),
        ));
    }
    if lower.contains("23p01") || lower.contains("exclusion constraint") {
        return Some(AppError::Conflict(
            "Record conflicts with an existing record".to_string(),
        ));
    }
    if lower.contains("40001")
        || lower.contains("40p01")
        || lower.contains("deadlock detected")
        || lower.contains("could not serialize access")
    {
        return Some(AppError::Conflict(
            "Database write conflict. Please retry.".to_string(),
        ));
    }

    None
}

fn looks_like_a_database_message(lower: &str) -> bool {
    lower.contains("error returned from database")
        || lower.contains("database error")
        || lower.contains("sqlstate")
        || lower.contains("sqlx")
        || lower.contains("pool timed out")
        || lower.contains("pool closed")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connection timed out")
        || lower.contains("could not connect")
        || lower.contains("too many connections")
        || lower.contains("23505")
        || lower.contains("23503")
        || lower.contains("23502")
        || lower.contains("23514")
        || lower.contains("23p01")
        || lower.contains("40001")
        || lower.contains("40p01")
        || lower.contains("constraint")
        || lower.contains("duplicate key value")
        || lower.contains("duplicate entry")
        || lower.contains("cannot add or update a child row")
        || lower.contains("deadlock detected")
        || lower.contains("could not serialize access")
}

fn looks_like_database_unavailable(lower: &str) -> bool {
    lower.contains("pool timed out")
        || lower.contains("pool closed")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connection closed")
        || lower.contains("connection terminated")
        || lower.contains("connection timed out")
        || lower.contains("could not connect")
        || lower.contains("failed to connect")
        || lower.contains("error communicating with database")
        || lower.contains("server closed the connection")
        || lower.contains("database system is starting up")
        || lower.contains("too many connections")
        || lower.contains("remaining connection slots are reserved")
        || lower.contains("timeout while waiting for an open connection")
}

fn duplicate_message(constraint: Option<&str>, message: &str) -> String {
    if constraint_mentions_name(constraint) || message_mentions_name(message) {
        "Name already exists".to_string()
    } else {
        "Record already exists".to_string()
    }
}

fn constraint_mentions_name(constraint: Option<&str>) -> bool {
    let Some(constraint) = constraint else {
        return false;
    };
    let lower = constraint.to_ascii_lowercase();
    lower == "name"
        || lower.contains("_name_")
        || lower.ends_with("_name")
        || lower.ends_with("_name_key")
        || lower.ends_with("_name_idx")
        || lower.ends_with("_name_unique")
}

fn message_mentions_name(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("key (name)=")
        || lower.contains("key (name)")
        || lower.contains(".name")
        || lower.contains(" name_unique")
        || lower.contains("_name_")
        || lower.contains("_name_key")
        || lower.contains("_name_idx")
        || lower.contains("name_lower")
}

#[cfg(test)]
mod classifier_tests {
    use std::error::Error as StdError;
    use std::fmt;

    use sqlx::Error as SqlxError;
    use sqlx::error::{DatabaseError, ErrorKind};

    use super::{AppError, classify_sqlx_error, normalize_database_error};

    /// Minimal `DatabaseError` test double so the classifier can be exercised
    /// for constraint-violation handling without a live database. The default
    /// `is_unique_violation()` on the trait delegates to `kind()`.
    #[derive(Debug)]
    struct FakeDbError {
        kind: ErrorKind,
        message: String,
        code: Option<String>,
        constraint: Option<String>,
    }

    impl fmt::Display for FakeDbError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl StdError for FakeDbError {}

    impl DatabaseError for FakeDbError {
        fn message(&self) -> &str {
            &self.message
        }

        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            self.code.as_deref().map(std::borrow::Cow::Borrowed)
        }

        fn constraint(&self) -> Option<&str> {
            self.constraint.as_deref()
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            // `ErrorKind` is `#[non_exhaustive]` and not `Copy`/`Clone`, so
            // rebuild a fresh value from the stored discriminant.
            match self.kind {
                ErrorKind::UniqueViolation => ErrorKind::UniqueViolation,
                ErrorKind::ForeignKeyViolation => ErrorKind::ForeignKeyViolation,
                ErrorKind::NotNullViolation => ErrorKind::NotNullViolation,
                ErrorKind::CheckViolation => ErrorKind::CheckViolation,
                _ => ErrorKind::Other,
            }
        }
    }

    fn db_error(kind: ErrorKind, constraint: Option<&str>) -> SqlxError {
        SqlxError::Database(Box::new(FakeDbError {
            kind,
            message: "duplicate key value violates unique constraint".to_string(),
            code: None,
            constraint: constraint.map(ToString::to_string),
        }))
    }

    #[test]
    fn pool_timeout_classifies_as_database_unavailable() {
        let error = classify_sqlx_error(&SqlxError::PoolTimedOut);
        assert!(
            matches!(error, AppError::DatabaseUnavailable(_)),
            "pool timeout should map to DatabaseUnavailable (503), got {error:?}"
        );
    }

    #[test]
    fn pool_closed_classifies_as_database_unavailable() {
        let error = classify_sqlx_error(&SqlxError::PoolClosed);
        assert!(matches!(error, AppError::DatabaseUnavailable(_)));
    }

    #[test]
    fn io_error_classifies_as_database_unavailable() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset");
        let error = classify_sqlx_error(&SqlxError::Io(io));
        assert!(
            matches!(error, AppError::DatabaseUnavailable(_)),
            "io error should map to DatabaseUnavailable (503), got {error:?}"
        );
    }

    #[test]
    fn unique_violation_classifies_as_conflict() {
        let error = classify_sqlx_error(&db_error(
            ErrorKind::UniqueViolation,
            Some("connconf_record_name_key"),
        ));
        assert!(
            matches!(error, AppError::Conflict(ref message) if message == "Name already exists"),
            "unique violation should map to Conflict (409), got {error:?}"
        );
    }

    #[test]
    fn unique_violation_with_name_suffix_constraint_classifies_as_name_conflict() {
        let error = classify_sqlx_error(&db_error(
            ErrorKind::UniqueViolation,
            Some("uq_connconf_record_name"),
        ));
        assert!(
            matches!(error, AppError::Conflict(ref message) if message == "Name already exists"),
            "unique violation should map to name Conflict (409), got {error:?}"
        );
    }

    #[test]
    fn foreign_key_violation_classifies_as_validation() {
        let error = classify_sqlx_error(&db_error(ErrorKind::ForeignKeyViolation, None));
        assert!(
            matches!(error, AppError::Validation(_)),
            "foreign key violations should map to Validation (400), got {error:?}"
        );
    }

    #[test]
    fn restrict_violation_code_classifies_as_conflict() {
        // ON DELETE RESTRICT raises SQLSTATE 23001, which sqlx reports as
        // ErrorKind::Other — the classifier must catch it by code, or a
        // blocked delete becomes a raw 500.
        let error = classify_sqlx_error(&SqlxError::Database(Box::new(FakeDbError {
            kind: ErrorKind::Other,
            message: "update or delete violates RESTRICT setting of foreign key constraint"
                .to_string(),
            code: Some("23001".to_string()),
            constraint: None,
        })));
        assert!(
            matches!(error, AppError::Conflict(_)),
            "RESTRICT violations should map to Conflict (409), got {error:?}"
        );
    }

    #[test]
    fn other_errors_fall_back_to_generic_database() {
        let error = classify_sqlx_error(&SqlxError::RowNotFound);
        assert!(
            matches!(error, AppError::Database(_)),
            "non-constraint, non-connection errors stay on the generic 500 path, got {error:?}"
        );
    }

    #[test]
    fn text_only_unique_name_violation_classifies_as_conflict() {
        let error = normalize_database_error(AppError::Database(
            "Failed to update connection-config record: error returned from database: duplicate key value violates unique constraint \"connconf_record_name_key\"".to_string(),
        ));
        assert!(
            matches!(error, AppError::Conflict(ref message) if message == "Name already exists")
        );
    }

    #[test]
    fn text_only_composite_unique_name_violation_classifies_as_name_conflict() {
        let error = normalize_database_error(AppError::Database(
            "update connconf_record: error returned from database: duplicate key value violates unique constraint \"ux_connconf_record_protocol_name_lower\"".to_string(),
        ));
        assert!(
            matches!(error, AppError::Conflict(ref message) if message == "Name already exists")
        );
    }

    #[test]
    fn text_only_deadlock_classifies_as_retryable_conflict() {
        let error = normalize_database_error(AppError::Database(
            "Failed to write row: error returned from database: deadlock detected".to_string(),
        ));
        assert!(
            matches!(error, AppError::Conflict(ref message) if message == "Database write conflict. Please retry.")
        );
    }

    #[test]
    fn text_only_pool_timeout_classifies_as_database_unavailable() {
        let error = normalize_database_error(AppError::Database(
            "Failed to list records: pool timed out while waiting for an open connection"
                .to_string(),
        ));
        assert!(matches!(error, AppError::DatabaseUnavailable(_)));
    }

    #[test]
    fn text_only_connection_refused_classifies_as_database_unavailable() {
        let error = normalize_database_error(AppError::Database(
            "Failed to create record: error communicating with database: connection refused"
                .to_string(),
        ));
        assert!(matches!(error, AppError::DatabaseUnavailable(_)));
    }

    #[test]
    fn non_database_duplicate_key_text_stays_generic_database_error() {
        let error = normalize_database_error(AppError::Database(
            "Failed to process request: duplicate key in input map".to_string(),
        ));
        assert!(matches!(error, AppError::Database(_)));
    }
}
