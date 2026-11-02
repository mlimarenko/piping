use sqlx::PgPool;
use uuid::Uuid;

use crate::api::rest::error_mapping::client_safe_message;
use crate::contexts::inventory::domain::content_type::{self};
use crate::contexts::inventory::infrastructure::persistence::lookup_content_type_id;
use crate::errors::AppError;

use super::types::BulkCreateError;

// The UUID parsers and the bulk-emptiness guard live in the crate-level
// `validation` module so every route — inventory and non-inventory alike —
// reports the same error. Re-export them here so the inventory handlers keep
// importing `super::errors::{parse_uuid, require_bulk_items}`.
pub(super) use crate::api::rest::routes::validation::{
    parse_optional_uuid, parse_uuid, require_bulk_items,
};

pub(super) fn parse_required_string(value: &str, label: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{label} is required")));
    }
    Ok(trimmed.to_string())
}

/// Validate a NetBox-style `object_type` token against an entity's allowed
/// target subset and the content-type registry, then resolve it to its seeded
/// `inventory_content_type` UUID (pure SELECT — never an INSERT).
///
/// Two gates: (a) token ∈ `allowed_subset` AND (b) token ∈ registry. An unknown
/// or out-of-subset token, or a token with no seeded row, is a client error
/// (400), never a write-time content-type creation.
pub(super) async fn resolve_object_type(
    pool: &PgPool,
    object_type: &str,
    allowed_subset: &[&str],
) -> Result<Uuid, AppError> {
    let trimmed = object_type.trim();
    if !allowed_subset.contains(&trimmed) {
        return Err(AppError::Validation(format!(
            "Unsupported object_type {object_type:?}: expected one of: {}",
            allowed_subset.join(", ")
        )));
    }
    let def = content_type::by_token(trimmed).ok_or_else(|| {
        AppError::Validation(format!(
            "Unsupported object_type {object_type:?}: not a registered content type"
        ))
    })?;
    lookup_content_type_id(pool, def.app_label, def.model)
        .await?
        .ok_or_else(|| {
            AppError::Validation(format!(
                "Unknown content type {object_type:?}: not a seeded inventory content type"
            ))
        })
}

pub(super) fn push_bulk_error(errors: &mut Vec<BulkCreateError>, index: usize, error: AppError) {
    push_bulk_error_with_hint(errors, index, None, error);
}

/// Attribute one shared failure to every row it affected.
///
/// The batch custom-field masking pass covers a whole `bulk_create` batch at
/// once, so its failure is not attributable to a single row — every created row
/// loses its masked view. Report it per index instead of dropping the detail.
pub(super) fn push_bulk_error_for_indices(
    errors: &mut Vec<BulkCreateError>,
    indices: &[usize],
    error: AppError,
) {
    let message = client_safe_message(error);
    for index in indices {
        let one_based = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
        errors.push(BulkCreateError {
            index: one_based,
            key_hint: None,
            error: message.clone(),
        });
    }
}

pub(super) fn push_bulk_error_with_hint(
    errors: &mut Vec<BulkCreateError>,
    index: usize,
    key_hint: Option<String>,
    error: AppError,
) {
    // Index is 1-based on the wire so external consumers can quote row
    // numbers that match human-friendly UIs without needing to add 1.
    let one_based = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
    errors.push(BulkCreateError {
        index: one_based,
        key_hint: key_hint.and_then(|h| {
            let trimmed = h.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }),
        error: client_safe_message(error),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bulk_error_sink_uses_the_canonical_client_message() {
        let mut errors = Vec::new();
        push_bulk_error(
            &mut errors,
            0,
            AppError::Database("synthetic-private-driver-detail".into()),
        );
        push_bulk_error_with_hint(
            &mut errors,
            2,
            Some(" row key ".into()),
            AppError::Config("synthetic-private-config-detail".into()),
        );
        push_bulk_error_for_indices(
            &mut errors,
            &[4, 6],
            AppError::Redis("synthetic-private-redis-detail".into()),
        );
        assert_eq!(
            errors.iter().map(|e| e.index).collect::<Vec<_>>(),
            vec![1, 3, 5, 7]
        );
        assert_eq!(errors[0].error, "Database error");
        assert_eq!(errors[1].error, "Configuration error");
        assert_eq!(errors[1].key_hint.as_deref(), Some("row key"));
        assert_eq!(errors[2].error, "Redis error");
        assert_eq!(errors[3].error, "Redis error");
        push_bulk_error(&mut errors, 0, AppError::Validation("Invalid name".into()));
        assert_eq!(errors[4].error, "Invalid name");
    }
}
