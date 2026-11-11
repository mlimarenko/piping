//! Shared request-validation helpers for REST route handlers.
//!
//! # Why this module exists
//!
//! Copying these parsers into each route module drifts: a copy that skips the
//! trim / empty-check, renders `AppError::ValidationUuid` (a context-free
//! `"Invalid UUID"` HTTP body that drops the field name), or disagrees on
//! whether a blank string means "absent" or "malformed" reports UUID errors
//! differently. This module is the single home for that logic so every route
//! reports UUID errors identically.
//!
//! # The canonical contract
//!
//! On a bad UUID we return [`AppError::Validation`] with the message
//! `"Invalid {label}: {error}"`. That variant maps to HTTP 400 **and keeps the
//! field label in the response body** (e.g. `"Invalid device_id: ..."`), which
//! is strictly more useful to an API consumer than a generic `"Invalid UUID"`.
//! Input is trimmed and rejected when blank.
//!
//! # Extending this module
//!
//! Two optional shapes are intentionally distinct — do not collapse them:
//! - [`parse_optional_uuid`] is *strict*: `Some("")` is an error.
//! - [`parse_uuid_or_blank`] treats a blank string as `None` (for query/body
//!   fields that clear a value by sending `""`).
//!
//! New shared validators (non-UUID scalars, bounded integers, enum tokens)
//! belong here too when they are duplicated across ≥2 route modules. Keep them
//! language-neutral and structural — never encode a domain keyword list.

use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use crate::errors::AppError;

/// Preserve omission versus explicit null on a nullable update field.
/// Use with `serde(default)` so only a present key calls this adapter.
pub(crate) fn deserialize_nullable_patch<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

/// Reject an empty required collection on a batch/bulk write request.
///
/// A batch request whose set of things to act on is empty is a client mistake,
/// never a successful no-op: it would answer `200` while doing nothing and
/// emitting no audit record. This is the one bulk-emptiness helper — the
/// inventory `bulk_create_*` handlers and the non-inventory batch DTOs all call
/// it. It accepts anything that borrows as a slice (`Vec<T>`, `&[T]`, arrays)
/// and returns the input unchanged, so a handler that owns the `items` can
/// destructure them in one step while a `&self` DTO validator passes a borrow
/// without cloning the request. Call it before any side effect.
pub(crate) fn require_bulk_items<T, I>(items: I) -> Result<I, AppError>
where
    I: AsRef<[T]>,
{
    if items.as_ref().is_empty() {
        return Err(AppError::Validation("items must not be empty".to_string()));
    }
    Ok(items)
}

/// Serde adapter for a required `items` array on a bulk-create body.
///
/// The legacy inventory bulk payloads (`{ "items": [ … ] }`) are deserialized
/// through this so an empty array is rejected during extraction — before any
/// handler runs, so before auth-independent side effects or audit. It is a
/// one-line delegation to [`require_bulk_items`], not a second copy of the
/// rule. Use via
/// `#[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]`.
pub(crate) fn deserialize_bulk_items<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let items = Vec::<T>::deserialize(deserializer)?;
    require_bulk_items(&items).map_err(|error| serde::de::Error::custom(error.to_string()))?;
    Ok(items)
}

/// Parse a required UUID, trimming surrounding whitespace and rejecting blank
/// input. On failure returns a 400 [`AppError::Validation`] whose message names
/// `label` (e.g. `"Invalid device_id: ..."`).
pub(crate) fn parse_uuid(value: &str, label: &str) -> Result<Uuid, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{label} cannot be empty")));
    }
    Uuid::parse_str(trimmed)
        .map_err(|error| AppError::Validation(format!("Invalid {label}: {error}")))
}

/// Strict optional UUID: `None` → `None`; `Some(raw)` must be a non-blank,
/// well-formed UUID or a 400 is returned. Use for optional foreign keys where
/// an explicit blank value is a client mistake, not a clear-to-null.
pub(crate) fn parse_optional_uuid(
    value: Option<String>,
    label: &str,
) -> Result<Option<Uuid>, AppError> {
    match value {
        Some(raw) => parse_uuid(&raw, label).map(Some),
        None => Ok(None),
    }
}

/// Optional UUID where a blank/whitespace string is treated as absent (`None`),
/// not an error. Use for query/body fields whose contract is "send `""` to
/// clear the value".
pub(crate) fn parse_uuid_or_blank(
    value: Option<String>,
    label: &str,
) -> Result<Option<Uuid>, AppError> {
    match value {
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => parse_uuid(&raw, label).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uuid_trims_and_rejects_blank() {
        assert!(parse_uuid("   ", "device_id").is_err());
        let id = Uuid::now_v7();
        assert_eq!(parse_uuid(&format!("  {id}  "), "device_id").unwrap(), id);
    }

    #[test]
    fn parse_uuid_error_names_the_field() {
        let error = parse_uuid("not-a-uuid", "device_id").unwrap_err();
        assert!(
            error.to_string().contains("device_id"),
            "error must name the field, got: {error}"
        );
    }

    #[test]
    fn require_bulk_items_rejects_an_empty_collection() {
        let empty: Vec<u8> = Vec::new();
        let error = require_bulk_items(empty).unwrap_err();
        assert!(
            matches!(error, AppError::Validation(ref message) if message == "items must not be empty"),
            "empty batch must be the canonical validation error, got: {error}"
        );
        assert!(require_bulk_items([1_u8]).is_ok());
        // A borrow is accepted too, so a `&self` DTO validator can call the one
        // helper without cloning the request.
        let owned = vec![1_u8];
        assert!(require_bulk_items(owned.as_slice()).is_ok());
    }

    #[test]
    fn optional_is_strict_but_or_blank_is_lenient() {
        assert!(parse_optional_uuid(Some(String::new()), "x").is_err());
        assert_eq!(parse_optional_uuid(None, "x").unwrap(), None);
        assert_eq!(
            parse_uuid_or_blank(Some("  ".to_string()), "x").unwrap(),
            None
        );
        assert_eq!(parse_uuid_or_blank(None, "x").unwrap(), None);
    }

    #[test]
    fn nullable_updates_distinguish_omission_null_and_value() -> Result<(), serde_json::Error> {
        #[derive(Deserialize)]
        struct Update {
            #[serde(default, deserialize_with = "deserialize_nullable_patch")]
            association: Option<Option<Uuid>>,
        }
        let absent: Update = serde_json::from_value(serde_json::json!({}))?;
        let clear: Update = serde_json::from_value(serde_json::json!({"association":null}))?;
        let id = Uuid::now_v7();
        let assign: Update = serde_json::from_value(serde_json::json!({"association":id}))?;
        assert_eq!(absent.association, None);
        assert_eq!(clear.association, Some(None));
        assert_eq!(assign.association, Some(Some(id)));
        Ok(())
    }
}
