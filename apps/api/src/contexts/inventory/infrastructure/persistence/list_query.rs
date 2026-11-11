//! Inventory list-query primitives.
//!
//! The generic, entity-agnostic list primitives ([`SortSpec`],
//! [`parse_csv_uuids`], [`parse_csv_strings`], the `push_*_in` IN-builders, and
//! [`MAX_FILTER_VALUES`]) live in [`crate::contexts::shared::list_query`] and are
//! re-exported here so inventory call sites keep a single import path.
//! Per-entity sort allowlists live next to their repository SELECTs as
//! `*_COLUMNS` registries.

pub use crate::contexts::shared::list_query::{
    CustomFieldListFilter, ListColumn, MAX_FILTER_VALUES, MAX_ID_FILTER_VALUES, SortSpec,
    parse_csv_strings, parse_csv_uuids, parse_custom_field_filters, parse_id_filter,
    push_custom_field_filters, push_id_filter, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in, search_pattern,
};

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_COLUMNS: &[ListColumn] = &[ListColumn::sortable("name", "d.name", "test.name")];

    #[test]
    fn sort_spec_absent_key_is_none() {
        let spec = SortSpec::parse_columns(None, Some("desc"), TEST_COLUMNS).unwrap();
        assert!(spec.is_none());
    }

    #[test]
    fn sort_spec_rejects_unknown_key() {
        let err = SortSpec::parse_columns(Some("DROP TABLE"), None, TEST_COLUMNS);
        assert!(matches!(err, Err(crate::errors::AppError::Validation(_))));
    }

    #[test]
    fn sort_spec_parses_desc() {
        let spec = SortSpec::parse_columns(Some("name"), Some("DESC"), TEST_COLUMNS)
            .unwrap()
            .expect("some spec");
        assert_eq!(spec.key, "name");
        assert!(spec.descending);
        assert_eq!(spec.sql_direction(), " DESC");
    }
}
