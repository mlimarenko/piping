//! Canonical pagination primitives.
//!
//! One helper, one shape, one source of truth for list-endpoint pagination
//! across REST handlers and repositories.
//!
//! Policy:
//! - Default limit: [`DEFAULT_PAGINATION_LIMIT`] (50)
//! - Maximum limit: [`MAX_PAGINATION_LIMIT`] (1000)
//! - Minimum limit: 1 (clamped up)
//! - Default offset: 0
//!
//! There are intentionally no per-context pagination helpers. If a concrete
//! endpoint needs different bounds, it must state why and that policy must
//! live here, not in a parallel module.

use crate::config::constants::{DEFAULT_PAGINATION_LIMIT, MAX_PAGINATION_LIMIT};

/// Clamps a user-provided pagination limit into the canonical range.
///
/// Returns [`DEFAULT_PAGINATION_LIMIT`] if `limit` is `None`; otherwise
/// clamps to `[1, MAX_PAGINATION_LIMIT]`.
#[must_use]
pub fn clamp_limit(limit: Option<u32>) -> u32 {
    clamp_limit_with_max(limit, MAX_PAGINATION_LIMIT)
}

/// Clamps a user-provided pagination limit into `[1, max]`, defaulting to
/// [`DEFAULT_PAGINATION_LIMIT`] when `limit` is `None`.
///
/// Almost every endpoint uses [`clamp_limit`] (capped at
/// [`MAX_PAGINATION_LIMIT`]). An endpoint may pass a different `max` only when
/// its access pattern justifies it, and that ceiling must be a named constant in
/// `config::constants` next to the other pagination policy — not an inline magic
/// number. The connection-target workspace list does this with
/// `MAX_CONNECTION_TARGET_LIST_LIMIT`: the connections tree needs the full target
/// set and paging recomputes the expensive UNION read-model per page, so it
/// fetches the whole list in one request.
#[must_use]
pub fn clamp_limit_with_max(limit: Option<u32>, max: u32) -> u32 {
    limit.unwrap_or(DEFAULT_PAGINATION_LIMIT).min(max).max(1)
}

/// Clamps a user-provided pagination offset into the canonical range.
///
/// Returns 0 if `offset` is `None`. `u32` is already non-negative.
#[must_use]
pub fn clamp_offset(offset: Option<u32>) -> u32 {
    offset.unwrap_or(0)
}

/// Canonical combined normalizer for `(limit, offset)` query parameters.
///
/// Returns `(limit, offset)` with the same semantics as calling
/// [`clamp_limit`] and [`clamp_offset`] individually. Prefer this form in
/// handlers that accept both parameters as a pair.
#[must_use]
pub fn normalize_page(limit: Option<u32>, offset: Option<u32>) -> (u32, u32) {
    (clamp_limit(limit), clamp_offset(offset))
}

/// Canonical page result envelope returned by repository list methods.
///
/// This is the single shape every `list_*_page` method returns. REST handlers
/// re-serialize it into response DTOs, but the domain-side shape is always this one.
#[derive(Debug, Clone)]
pub struct PageResult<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limit_defaults_when_none() {
        assert_eq!(clamp_limit(None), DEFAULT_PAGINATION_LIMIT);
    }

    #[test]
    fn clamp_limit_caps_at_max() {
        assert_eq!(clamp_limit(Some(10_000)), MAX_PAGINATION_LIMIT);
    }

    #[test]
    fn clamp_limit_raises_zero_to_one() {
        assert_eq!(clamp_limit(Some(0)), 1);
    }

    #[test]
    fn clamp_offset_defaults_to_zero() {
        assert_eq!(clamp_offset(None), 0);
        assert_eq!(clamp_offset(Some(42)), 42);
    }

    #[test]
    fn normalize_page_combines_helpers() {
        assert_eq!(normalize_page(None, None), (DEFAULT_PAGINATION_LIMIT, 0));
        assert_eq!(normalize_page(Some(25), Some(10)), (25, 10));
        assert_eq!(
            normalize_page(Some(5_000), Some(0)),
            (MAX_PAGINATION_LIMIT, 0)
        );
    }
}
