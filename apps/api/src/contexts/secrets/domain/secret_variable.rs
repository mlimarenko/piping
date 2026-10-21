use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

/// Machine key prefix reserved for secret-field intent rows. Named secrets may
/// never use it — that namespace belongs to `secret_field_intent_service`.
pub const SECRET_FIELD_KEY_PREFIX: &str = "secret-field/";

/// Discriminates a secret created and managed by name through the admin
/// "Automation secrets" API from a machine `secret-field/...` row.
pub const SECRET_KIND_FIELD: &str = "field";
pub const SECRET_KIND_NAMED: &str = "named";

/// Maximum length of a named-secret key (mirrors the validation regex bound and
/// stays within the `secrets_variable.key varchar(255)` column).
pub const NAMED_SECRET_KEY_MAX_LEN: usize = 200;

/// Secret variable domain model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretVariable {
    pub id: Uuid,
    pub key: String,
    pub is_masked: bool,
    pub is_protected: bool,
    /// `field` (machine secret-field row) or `named` (admin-managed automation
    /// secret).
    pub kind: String,
    /// Optional human description, only meaningful for `named` secrets.
    pub description: Option<String>,
    /// Internal authority to retire a machine variable with its last field usage.
    #[serde(skip)]
    pub retire_with_field_usage: bool,
}

/// Read-model for the admin "Automation secrets" list/detail surface. Carries
/// derived facts (`has_value`, `usage_count`, `created_at`) that are not part of
/// the core [`SecretVariable`] identity but are computed per query. The secret
/// value is NEVER included — only whether one is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedSecretSummary {
    pub id: Uuid,
    pub key: String,
    pub description: Option<String>,
    pub is_masked: bool,
    pub created_at: DateTime<Utc>,
    /// True when a value exists in `secrets_store` for this secret.
    pub has_value: bool,
    /// Number of `secret_usage_binding` rows referencing this secret.
    pub usage_count: i64,
}

/// Validates and normalizes a named-secret key.
///
/// Contract: `[a-z0-9]([a-z0-9._-]{0,198}[a-z0-9])?` — lowercase
/// alphanumeric plus `. _ -` in the interior, first and last characters
/// alphanumeric, length 1..=200. The reserved `secret-field/` prefix is
/// rejected so named secrets can never collide with machine intent rows.
///
/// Implemented by hand (no `regex` dependency in production code) and returns
/// the trimmed key on success.
pub fn validate_named_secret_key(raw: &str) -> Result<String, AppError> {
    let key = raw.trim();
    if key.is_empty() {
        return Err(AppError::Validation("secret key is required".to_string()));
    }
    if key.len() > NAMED_SECRET_KEY_MAX_LEN {
        return Err(AppError::Validation(format!(
            "secret key must be at most {NAMED_SECRET_KEY_MAX_LEN} characters"
        )));
    }
    if key.starts_with(SECRET_FIELD_KEY_PREFIX) {
        return Err(AppError::Validation(format!(
            "secret key must not start with the reserved '{SECRET_FIELD_KEY_PREFIX}' prefix"
        )));
    }

    let is_edge = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let is_interior = |c: char| is_edge(c) || matches!(c, '.' | '_' | '-');

    let bytes = key.as_bytes();
    let first = bytes[0] as char;
    let last = bytes[bytes.len() - 1] as char;
    if !is_edge(first) || !is_edge(last) {
        return Err(AppError::Validation(
            "secret key must start and end with a lowercase letter or digit".to_string(),
        ));
    }
    if !key.chars().all(is_interior) {
        return Err(AppError::Validation(
            "secret key may contain only lowercase letters, digits, '.', '_' and '-'".to_string(),
        ));
    }

    Ok(key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_lowercase_key() {
        assert_eq!(
            validate_named_secret_key("grad-api-token").unwrap(),
            "grad-api-token"
        );
    }

    #[test]
    fn accepts_interior_dot_underscore_dash_and_digits() {
        assert_eq!(validate_named_secret_key("a1.b_2-c3").unwrap(), "a1.b_2-c3");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(validate_named_secret_key("  token  ").unwrap(), "token");
    }

    #[test]
    fn accepts_single_alphanumeric_character() {
        assert_eq!(validate_named_secret_key("x").unwrap(), "x");
        assert_eq!(validate_named_secret_key("7").unwrap(), "7");
    }

    #[test]
    fn rejects_empty_and_whitespace_only() {
        assert!(validate_named_secret_key("").is_err());
        assert!(validate_named_secret_key("   ").is_err());
    }

    #[test]
    fn rejects_uppercase() {
        assert!(validate_named_secret_key("Grad-Token").is_err());
    }

    #[test]
    fn rejects_leading_or_trailing_separator() {
        assert!(validate_named_secret_key("-token").is_err());
        assert!(validate_named_secret_key("token-").is_err());
        assert!(validate_named_secret_key(".token").is_err());
        assert!(validate_named_secret_key("_token").is_err());
    }

    #[test]
    fn rejects_disallowed_characters() {
        assert!(validate_named_secret_key("token/name").is_err());
        assert!(validate_named_secret_key("token name").is_err());
        assert!(validate_named_secret_key("tökn").is_err());
    }

    #[test]
    fn rejects_reserved_secret_field_prefix() {
        assert!(validate_named_secret_key("secret-field/x").is_err());
    }

    #[test]
    fn rejects_over_length_key() {
        let too_long = "a".repeat(NAMED_SECRET_KEY_MAX_LEN + 1);
        assert!(validate_named_secret_key(&too_long).is_err());
        let at_limit = "a".repeat(NAMED_SECRET_KEY_MAX_LEN);
        assert!(validate_named_secret_key(&at_limit).is_ok());
    }
}
