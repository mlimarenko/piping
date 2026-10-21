//! Reusable, entity-agnostic list-query primitives shared across contexts.
//!
//! These helpers give every list endpoint a single, SQL-injection-safe way to
//! express:
//!
//! - **Multi-value `IN` filters** — comma-separated query values parsed into a
//!   deduplicated, capped list ([`parse_csv_uuids`] / [`parse_csv_strings`]).
//!   Repositories then emit `col IN ($1, $2, …)` via parameterized binds.
//! - **Allowlisted sorting** — a [`SortSpec`] carrying a stable logical column
//!   key plus direction, resolved to a literal SQL column expression through a
//!   per-entity allowlist function. Unknown keys never reach SQL.
//!
//! The pattern is intentionally entity-agnostic: the parsers and [`SortSpec`]
//! are shared, and each entity contributes only its own column allowlist.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::contexts::custom_fields::domain::{
    CustomFieldDefinition, CustomFieldType, FilterLogic, is_valid_custom_field_key,
};
use crate::contexts::secrets::application::INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE;
use crate::errors::AppError;

/// Maximum number of values accepted in a single multi-value filter list.
///
/// Caps the size of any generated `IN (…)` clause to bound the parameter count
/// and keep query plans sane.
pub const MAX_FILTER_VALUES: usize = 200;

/// Maximum number of identifiers accepted by the cross-collection `ids` filter.
///
/// UUIDs plus separators consume about 37 bytes each, so 100 values keep the
/// filter near 3.7 KiB and leave headroom in an 8 KiB request line for the path,
/// pagination, and the collection's other filters.
pub const MAX_ID_FILTER_VALUES: usize = 100;

/// Filter value type for a declarative list column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    /// Comma-separated UUID list, emitted with [`push_uuid_in`].
    Uuid,
    /// Comma-separated string/enum list, emitted with [`push_string_in`].
    String,
}

/// Declarative filter binding for one list column.
#[derive(Debug, Clone, Copy)]
pub struct ListColumnFilter {
    /// Query parameter name accepted by the endpoint.
    pub param: &'static str,
    /// SQL column expression used inside the `IN (...)` predicate.
    pub sql: &'static str,
    /// Parser/binder kind for incoming values.
    pub kind: FilterKind,
}

/// Declarative list-column contract local to one repository SELECT.
///
/// `sort_sql` and `filter.sql` are trusted compile-time literals. They are
/// intentionally stored next to the repository query that defines table aliases,
/// not in a global runtime SQL builder.
#[derive(Debug, Clone, Copy)]
pub struct ListColumn {
    /// Stable logical key used by API `sort` and frontend table columns.
    pub key: &'static str,
    /// SQL expression used for `ORDER BY` when sortable.
    pub sort_sql: Option<&'static str>,
    /// Optional checkbox/IN filter binding.
    pub filter: Option<ListColumnFilter>,
    /// Optional frontend FK option source, kept as descriptor metadata.
    pub fk_entity: Option<&'static str>,
    /// Optional i18n label metadata for future descriptor emission.
    pub label_key: &'static str,
}

impl ListColumn {
    /// Shorthand for a sortable scalar column with no checkbox filter.
    #[must_use]
    pub const fn sortable(
        key: &'static str,
        sort_sql: &'static str,
        label_key: &'static str,
    ) -> Self {
        Self {
            key,
            sort_sql: Some(sort_sql),
            filter: None,
            fk_entity: None,
            label_key,
        }
    }

    /// Shorthand for a sortable enum/string-filtered column.
    #[must_use]
    pub const fn enum_filter(
        key: &'static str,
        sort_sql: &'static str,
        filter_sql: &'static str,
        label_key: &'static str,
    ) -> Self {
        Self::enum_filter_with_param(key, sort_sql, key, filter_sql, label_key)
    }

    /// Shorthand for a sortable enum/string-filtered column where the stable
    /// table column key differs from the HTTP query parameter.
    #[must_use]
    pub const fn enum_filter_with_param(
        key: &'static str,
        sort_sql: &'static str,
        param: &'static str,
        filter_sql: &'static str,
        label_key: &'static str,
    ) -> Self {
        Self {
            key,
            sort_sql: Some(sort_sql),
            filter: Some(ListColumnFilter {
                param,
                sql: filter_sql,
                kind: FilterKind::String,
            }),
            fk_entity: None,
            label_key,
        }
    }

    /// Shorthand for a sortable FK-filtered column.
    #[must_use]
    pub const fn fk_filter(
        key: &'static str,
        sort_sql: &'static str,
        param: &'static str,
        filter_sql: &'static str,
        fk_entity: &'static str,
        label_key: &'static str,
    ) -> Self {
        Self {
            key,
            sort_sql: Some(sort_sql),
            filter: Some(ListColumnFilter {
                param,
                sql: filter_sql,
                kind: FilterKind::Uuid,
            }),
            fk_entity: Some(fk_entity),
            label_key,
        }
    }
}

/// Resolve a filter parameter to its SQL expression through a declarative
/// list-column registry.
///
/// The registry is the repository's own compile-time `&[ListColumn]`, so a
/// missing entry is a programming error in that table's metadata rather than
/// bad client input — hence [`AppError::Database`] naming the offending
/// parameter, not a validation error.
///
/// # Errors
///
/// Returns [`AppError::Database`] when `columns` declares no filter for
/// `param`.
pub fn filter_column(
    columns: &[ListColumn],
    entity: &str,
    param: &str,
) -> Result<ListColumnFilter, AppError> {
    columns
        .iter()
        .filter_map(|column| column.filter)
        .find(|filter| filter.param == param)
        .ok_or_else(|| AppError::Database(format!("Missing {entity} filter metadata: {param}")))
}

/// Resolve a stable sort key through a declarative list-column registry.
#[must_use]
pub fn list_sort_column(columns: &[ListColumn], key: &str) -> Option<&'static str> {
    columns
        .iter()
        .find(|column| column.key == key)
        .and_then(|column| column.sort_sql)
}

/// Emit a safe `ORDER BY` clause for an optional validated sort spec.
///
/// `default_sql` is a trusted literal fallback used when no `sort` query param
/// is present. User input never reaches this function as SQL: [`SortSpec`]
/// validates keys through [`list_sort_column`] before repositories call here.
pub fn push_order_by(
    builder: &mut QueryBuilder<'_, Postgres>,
    columns: &[ListColumn],
    sort: Option<&SortSpec>,
    default_sql: &'static str,
) {
    builder.push(" ORDER BY ");
    match sort {
        Some(sort) => {
            let sql = list_sort_column(columns, &sort.key).unwrap_or(default_sql);
            builder.push(sql);
            builder.push(sort.sql_direction());
        }
        None => {
            builder.push(default_sql);
        }
    }
}

/// Parsed, validated sort directive for a list endpoint.
///
/// `key` is the resolved logical column key (already confirmed against a
/// per-entity allowlist by [`SortSpec::parse`]); `descending` selects the SQL
/// direction. Repositories map `key` to a literal column expression via the
/// entity's allowlist function and emit `ORDER BY <col> ASC|DESC`.
#[derive(Debug, Clone)]
pub struct SortSpec {
    pub key: String,
    pub descending: bool,
}

impl SortSpec {
    /// Parse a `sort`/`order` query pair against a per-entity column allowlist.
    ///
    /// - `sort` is the logical column key; it is validated against `allowlist`
    ///   (a function returning the safe literal SQL column for a key, or `None`
    ///   for an unknown key). An unknown key is a client error.
    /// - `order` accepts only `asc` (default) or `desc`, case-insensitively.
    /// - When `sort` is absent the endpoint keeps its default ordering, so this
    ///   returns `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for an unknown sort key or an `order`
    /// value other than `asc`/`desc`.
    pub fn parse(
        sort: Option<&str>,
        order: Option<&str>,
        allowlist: fn(&str) -> Option<&'static str>,
    ) -> Result<Option<Self>, AppError> {
        let Some(raw_key) = sort.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if allowlist(raw_key).is_none() {
            return Err(AppError::Validation(format!(
                "Invalid sort key {raw_key:?}"
            )));
        }
        let descending = match order.map(str::trim).filter(|s| !s.is_empty()) {
            None => false,
            Some(value) if value.eq_ignore_ascii_case("asc") => false,
            Some(value) if value.eq_ignore_ascii_case("desc") => true,
            Some(value) => {
                return Err(AppError::Validation(format!(
                    "Invalid sort order {value:?}: expected one of: asc, desc"
                )));
            }
        };
        Ok(Some(Self {
            key: raw_key.to_string(),
            descending,
        }))
    }

    /// Parse a `sort`/`order` query pair against a declarative column registry.
    ///
    /// This is equivalent to [`Self::parse`] but avoids one-off wrapper
    /// functions once a repository owns a `const &[ListColumn]` registry.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for an unknown sort key or an `order`
    /// value other than `asc`/`desc`.
    pub fn parse_columns(
        sort: Option<&str>,
        order: Option<&str>,
        columns: &[ListColumn],
    ) -> Result<Option<Self>, AppError> {
        let Some(raw_key) = sort.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if list_sort_column(columns, raw_key).is_none() {
            return Err(AppError::Validation(format!(
                "Invalid sort key {raw_key:?}"
            )));
        }
        let descending = match order.map(str::trim).filter(|s| !s.is_empty()) {
            None => false,
            Some(value) if value.eq_ignore_ascii_case("asc") => false,
            Some(value) if value.eq_ignore_ascii_case("desc") => true,
            Some(value) => {
                return Err(AppError::Validation(format!(
                    "Invalid sort order {value:?}: expected one of: asc, desc"
                )));
            }
        };
        Ok(Some(Self {
            key: raw_key.to_string(),
            descending,
        }))
    }

    /// SQL direction keyword for this spec (` ASC` or ` DESC`).
    #[must_use]
    pub fn sql_direction(&self) -> &'static str {
        if self.descending { " DESC" } else { " ASC" }
    }
}

/// Parse a comma-separated list of UUIDs into a deduplicated, capped vector.
///
/// Empty/whitespace-only entries are skipped. The result preserves first-seen
/// order, drops duplicates, and is capped at [`MAX_FILTER_VALUES`]. `None` or an
/// all-empty input yields an empty vector (no filter).
///
/// # Errors
///
/// Returns [`AppError::Validation`] if any non-empty entry is not a valid UUID
/// or if the input exceeds [`MAX_FILTER_VALUES`] distinct values.
pub fn parse_csv_uuids(raw: Option<&str>, label: &str) -> Result<Vec<Uuid>, AppError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out: Vec<Uuid> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = Uuid::parse_str(trimmed)
            .map_err(|error| AppError::Validation(format!("Invalid {label}: {error}")))?;
        if !out.contains(&value) {
            out.push(value);
        }
        if out.len() > MAX_FILTER_VALUES {
            return Err(AppError::Validation(format!(
                "{label} accepts at most {MAX_FILTER_VALUES} values"
            )));
        }
    }
    Ok(out)
}

/// Parse the optional, comma-separated collection identifier filter.
///
/// Unlike the older FK parsers, this deliberately preserves presence:
/// `None` means that no identifier predicate was requested, while `Some("")`
/// becomes `Some(vec![])` and therefore represents an explicitly empty set.
/// Values are deduplicated after the raw identifier count has been capped so
/// repeated identifiers cannot bypass the request-size bound.
///
/// # Errors
///
/// Returns [`AppError::Validation`] when a value is not a UUID or more than
/// [`MAX_ID_FILTER_VALUES`] non-empty identifiers are supplied.
pub fn parse_id_filter(raw: Option<&str>) -> Result<Option<Vec<Uuid>>, AppError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let tokens = raw
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.len() > MAX_ID_FILTER_VALUES {
        return Err(AppError::Validation(format!(
            "ids accepts at most {MAX_ID_FILTER_VALUES} identifiers"
        )));
    }

    let mut ids = Vec::with_capacity(tokens.len());
    for token in tokens {
        let id = Uuid::parse_str(token)
            .map_err(|error| AppError::Validation(format!("Invalid ids value: {error}")))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(Some(ids))
}

/// Parse a comma-separated list of strings into a trimmed, deduplicated, capped
/// vector.
///
/// Empty/whitespace-only entries are skipped. The result preserves first-seen
/// order, drops duplicates, and is capped at [`MAX_FILTER_VALUES`] (excess
/// values are dropped). `None` or an all-empty input yields an empty vector.
#[must_use]
pub fn parse_csv_strings(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        if out.iter().any(|existing| existing == trimmed) {
            continue;
        }
        out.push(trimmed.to_string());
        if out.len() >= MAX_FILTER_VALUES {
            break;
        }
    }
    out
}

/// Tracks where a filtered query is inside its `WHERE` clause.
///
/// Predicates are emitted conditionally — an empty filter list pushes nothing —
/// so the separator cannot be decided up front: the first predicate that fires
/// needs ` WHERE `, and every later one needs ` AND `. Every repository used to
/// carry this as a local flag plus a `push_and` closure: ten identical lines
/// apiece, and one more place to get the sequencing wrong.
///
/// There are three genuinely different starting points, and collapsing them is
/// how `WHERE AND …` gets generated. They are separate constructors on purpose:
/// [`WhereState::new`] owns the keyword, [`WhereState::after_where`] joins a
/// keyword the caller already pushed, and [`WhereState::after_predicate`] joins
/// a predicate the caller already pushed.
///
/// Pass `&mut WhereState` to the `push_*` helpers, or call [`WhereState::and`]
/// directly when a repository emits a bespoke predicate between the shared ones.
#[derive(Debug, Clone)]
pub struct WhereState {
    /// Whether the next predicate must be preceded by ` AND `.
    needs_separator: bool,
    /// Whether this clause is responsible for emitting the ` WHERE ` keyword.
    owns_where: bool,
}

impl Default for WhereState {
    fn default() -> Self {
        Self::new()
    }
}

impl WhereState {
    /// The query has no `WHERE` yet: the first predicate emits ` WHERE `, later
    /// ones ` AND `. Nothing is emitted if no predicate ever fires, so an empty
    /// filter set cannot produce a dangling `WHERE`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            needs_separator: false,
            owns_where: true,
        }
    }

    /// The caller has already pushed ` WHERE ` but no predicate yet: the first
    /// predicate emits NOTHING, later ones ` AND `.
    ///
    /// Used where a query decides up front that it has filters and pushes the
    /// keyword itself — see the device and IP-address list queries.
    #[must_use]
    pub fn after_where() -> Self {
        Self {
            needs_separator: false,
            owns_where: false,
        }
    }

    /// The caller has already pushed both ` WHERE ` and a predicate: every
    /// predicate from here on is joined with ` AND `.
    #[must_use]
    pub fn after_predicate() -> Self {
        Self {
            needs_separator: true,
            owns_where: false,
        }
    }

    /// Open (or continue) the `WHERE` clause, leaving the builder positioned for
    /// the caller's predicate.
    pub fn and(&mut self, builder: &mut QueryBuilder<'_, Postgres>) {
        if self.needs_separator {
            builder.push(" AND ");
        } else if self.owns_where {
            builder.push(" WHERE ");
        }
        self.needs_separator = true;
    }

    /// Whether any predicate has been emitted into the clause.
    #[must_use]
    pub fn has_predicates(&self) -> bool {
        self.needs_separator
    }
}

/// Emit `<column> IN ($1, $2, …)` for a UUID filter list, parameterized.
///
/// `column` is a trusted compile-time literal (table-qualified where needed);
/// the values are always bound, never interpolated. No-op for an empty list.
/// `state` injects the leading `WHERE`/`AND` separator only when a predicate is
/// actually emitted.
pub fn push_uuid_in(
    builder: &mut QueryBuilder<'_, Postgres>,
    state: &mut WhereState,
    column: &str,
    values: &[Uuid],
) {
    if values.is_empty() {
        return;
    }
    state.and(builder);
    builder.push(column);
    builder.push(" IN (");
    let mut separated = builder.separated(", ");
    for value in values {
        separated.push_bind(*value);
    }
    builder.push(")");
}

/// Emit the optional collection-id predicate while preserving an empty set.
///
/// An absent filter is a no-op. A present non-empty filter is the usual bound
/// `IN` predicate. A present empty filter emits `FALSE`, which makes `ids=` an
/// empty result instead of accidentally widening the request to the full
/// collection. The supplied [`WhereState`] composes this predicate with search,
/// resource filters, pagination counts, and row-level visibility conditions.
pub fn push_id_filter(
    builder: &mut QueryBuilder<'_, Postgres>,
    state: &mut WhereState,
    column: &str,
    ids: Option<&[Uuid]>,
) {
    let Some(ids) = ids else {
        return;
    };
    if ids.is_empty() {
        state.and(builder);
        builder.push("FALSE");
        return;
    }
    push_uuid_in(builder, state, column, ids);
}

/// Emit `<column> IN ($1, $2, …)` for a string filter list, parameterized.
///
/// `column` is a trusted compile-time literal (table-qualified where needed);
/// the values are always bound, never interpolated. No-op for an empty list.
pub fn push_string_in(
    builder: &mut QueryBuilder<'_, Postgres>,
    state: &mut WhereState,
    column: &str,
    values: &[String],
) {
    if values.is_empty() {
        return;
    }
    state.and(builder);
    builder.push(column);
    builder.push(" IN (");
    let mut separated = builder.separated(", ");
    for value in values {
        separated.push_bind(value.clone());
    }
    builder.push(")");
}

/// Normalize a free-text list search query into the bound pattern used by SQL
/// `ILIKE` predicates. Empty and whitespace-only input is treated as absent.
#[must_use]
pub fn search_pattern(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{value}%"))
}

/// Emit a parameterized `ILIKE` search predicate over trusted SQL expressions.
///
/// `columns` are compile-time SQL snippets owned by the repository query. User
/// input is always bound, never interpolated. The predicate is skipped for an
/// absent search pattern or an empty column list.
pub fn push_search_ilike(
    builder: &mut QueryBuilder<'_, Postgres>,
    state: &mut WhereState,
    columns: &[&str],
    search: Option<&str>,
) {
    let Some(search) = search else {
        return;
    };
    if columns.is_empty() {
        return;
    }

    state.and(builder);
    builder.push("(");
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder.push("COALESCE((");
        builder.push(*column);
        builder.push(")::text, '') ILIKE ");
        builder.push_bind(search.to_string());
    }
    builder.push(")");
}

const CUSTOM_FIELD_PARAM_PREFIX: &str = "cf_";

/// A validated, definition-driven custom-field filter set for an inventory list.
#[derive(Debug, Clone, Default)]
pub struct CustomFieldListFilter {
    predicates: Vec<CustomFieldPredicate>,
}

impl CustomFieldListFilter {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.predicates.is_empty()
    }
}

#[derive(Debug, Clone)]
enum CustomFieldPredicate {
    Text {
        name: String,
        value: String,
        mode: TextMatchMode,
    },
    Numeric {
        name: String,
        value: f64,
        op: RangeOperator,
    },
    Temporal {
        name: String,
        value: String,
        op: RangeOperator,
        kind: TemporalKind,
    },
    Boolean {
        name: String,
        value: bool,
    },
    ExactOne {
        name: String,
        values: Vec<String>,
    },
    ArrayContainsAny {
        name: String,
        values: Vec<String>,
    },
    SecretPresence {
        name: String,
        present: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextMatchMode {
    Contains,
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeOperator {
    Exact,
    Gte,
    Lte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemporalKind {
    Date,
    Datetime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedCustomFieldOperator {
    Default,
    Exact,
    Contains,
    Gte,
    Lte,
    Present,
}

/// Build custom-field list filters from flattened query params.
///
/// Grammar:
/// - `cf_<name>=value` for the default operator of the field type.
/// - `cf_<name>__exact=value`, `__gte`, `__lte` for supported typed operators.
/// - `cf_<name>__present=true|false` for secret fields only.
/// - `json` fields and `filter_logic=disabled` fields are rejected.
///
/// Values are validated and bound later as SQL parameters; cleartext secret
/// values are never accepted.
pub fn parse_custom_field_filters(
    raw: &BTreeMap<String, String>,
    definitions: &[CustomFieldDefinition],
) -> Result<CustomFieldListFilter, AppError> {
    let mut predicates = Vec::new();
    for (param, value) in raw {
        if !param.starts_with(CUSTOM_FIELD_PARAM_PREFIX) {
            return Err(AppError::Validation(format!(
                "Unsupported query parameter {param:?}"
            )));
        }
        let (name, operator) = parse_custom_field_param(param)?;
        let definition = definitions
            .iter()
            .find(|definition| definition.name == name)
            .ok_or_else(|| {
                AppError::Validation(format!("Unknown custom-field filter {param:?}"))
            })?;
        if definition.filter_logic == FilterLogic::Disabled {
            return Err(AppError::Validation(format!(
                "Custom-field filter {param:?} is disabled"
            )));
        }
        predicates.push(parse_custom_field_predicate(
            param, value, definition, operator,
        )?);
    }
    Ok(CustomFieldListFilter { predicates })
}

fn parse_custom_field_param(param: &str) -> Result<(&str, ParsedCustomFieldOperator), AppError> {
    let rest = &param[CUSTOM_FIELD_PARAM_PREFIX.len()..];
    let (name, operator) = match rest.rsplit_once("__") {
        Some((name, "exact")) => (name, ParsedCustomFieldOperator::Exact),
        Some((name, "contains")) => (name, ParsedCustomFieldOperator::Contains),
        Some((name, "gte")) => (name, ParsedCustomFieldOperator::Gte),
        Some((name, "lte")) => (name, ParsedCustomFieldOperator::Lte),
        Some((name, "present")) => (name, ParsedCustomFieldOperator::Present),
        Some((_name, op)) => {
            return Err(AppError::Validation(format!(
                "Unsupported custom-field filter operator {op:?}"
            )));
        }
        None => (rest, ParsedCustomFieldOperator::Default),
    };
    if !is_valid_custom_field_key(name) {
        return Err(AppError::Validation(format!(
            "Invalid custom-field filter key {param:?}"
        )));
    }
    Ok((name, operator))
}

fn parse_custom_field_predicate(
    param: &str,
    raw_value: &str,
    definition: &CustomFieldDefinition,
    operator: ParsedCustomFieldOperator,
) -> Result<CustomFieldPredicate, AppError> {
    let value = raw_value.trim();
    if value.is_empty() {
        return Err(AppError::Validation(format!(
            "Custom-field filter {param:?} requires a value"
        )));
    }

    if definition.is_secret {
        if operator != ParsedCustomFieldOperator::Present {
            return Err(AppError::Validation(format!(
                "Secret custom-field filter {param:?} supports presence only"
            )));
        }
        return Ok(CustomFieldPredicate::SecretPresence {
            name: definition.name.clone(),
            present: parse_bool_filter(value, param)?,
        });
    }

    match definition.field_type {
        CustomFieldType::Text | CustomFieldType::Longtext | CustomFieldType::Url => {
            let mode = match (definition.filter_logic, operator) {
                (
                    FilterLogic::Loose,
                    ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Contains,
                ) => TextMatchMode::Contains,
                (
                    FilterLogic::Exact,
                    ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Exact,
                ) => TextMatchMode::Exact,
                (_, ParsedCustomFieldOperator::Exact) => TextMatchMode::Exact,
                (_, ParsedCustomFieldOperator::Contains) => TextMatchMode::Contains,
                _ => return Err(unsupported_custom_field_operator(param)),
            };
            Ok(CustomFieldPredicate::Text {
                name: definition.name.clone(),
                value: value.to_string(),
                mode,
            })
        }
        CustomFieldType::Integer | CustomFieldType::Decimal => {
            let op = parse_range_operator(operator, param)?;
            let value = value.parse::<f64>().map_err(|_| {
                AppError::Validation(format!(
                    "Custom-field filter {param:?} requires a numeric value"
                ))
            })?;
            Ok(CustomFieldPredicate::Numeric {
                name: definition.name.clone(),
                value,
                op,
            })
        }
        CustomFieldType::Date => {
            let op = parse_range_operator(operator, param)?;
            NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
                AppError::Validation(format!(
                    "Custom-field filter {param:?} requires a YYYY-MM-DD date"
                ))
            })?;
            Ok(CustomFieldPredicate::Temporal {
                name: definition.name.clone(),
                value: value.to_string(),
                op,
                kind: TemporalKind::Date,
            })
        }
        CustomFieldType::Datetime => {
            let op = parse_range_operator(operator, param)?;
            DateTime::parse_from_rfc3339(value).map_err(|_| {
                AppError::Validation(format!(
                    "Custom-field filter {param:?} requires an RFC3339 datetime"
                ))
            })?;
            Ok(CustomFieldPredicate::Temporal {
                name: definition.name.clone(),
                value: value.to_string(),
                op,
                kind: TemporalKind::Datetime,
            })
        }
        CustomFieldType::Boolean => {
            if !matches!(
                operator,
                ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Exact
            ) {
                return Err(unsupported_custom_field_operator(param));
            }
            Ok(CustomFieldPredicate::Boolean {
                name: definition.name.clone(),
                value: parse_bool_filter(value, param)?,
            })
        }
        CustomFieldType::Select => {
            if !matches!(
                operator,
                ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Exact
            ) {
                return Err(unsupported_custom_field_operator(param));
            }
            Ok(CustomFieldPredicate::ExactOne {
                name: definition.name.clone(),
                values: parse_custom_field_csv(value, param)?,
            })
        }
        CustomFieldType::Object => {
            if !matches!(
                operator,
                ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Exact
            ) {
                return Err(unsupported_custom_field_operator(param));
            }
            Ok(CustomFieldPredicate::ExactOne {
                name: definition.name.clone(),
                values: parse_custom_field_uuid_csv(value, param)?,
            })
        }
        CustomFieldType::Multiselect => {
            if !matches!(
                operator,
                ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Exact
            ) {
                return Err(unsupported_custom_field_operator(param));
            }
            Ok(CustomFieldPredicate::ArrayContainsAny {
                name: definition.name.clone(),
                values: parse_custom_field_csv(value, param)?,
            })
        }
        CustomFieldType::Multiobject => {
            if !matches!(
                operator,
                ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Exact
            ) {
                return Err(unsupported_custom_field_operator(param));
            }
            Ok(CustomFieldPredicate::ArrayContainsAny {
                name: definition.name.clone(),
                values: parse_custom_field_uuid_csv(value, param)?,
            })
        }
        CustomFieldType::Json => Err(AppError::Validation(format!(
            "Custom-field filter {param:?} is not supported for json fields"
        ))),
    }
}

fn parse_range_operator(
    operator: ParsedCustomFieldOperator,
    param: &str,
) -> Result<RangeOperator, AppError> {
    match operator {
        ParsedCustomFieldOperator::Default | ParsedCustomFieldOperator::Exact => {
            Ok(RangeOperator::Exact)
        }
        ParsedCustomFieldOperator::Gte => Ok(RangeOperator::Gte),
        ParsedCustomFieldOperator::Lte => Ok(RangeOperator::Lte),
        _ => Err(unsupported_custom_field_operator(param)),
    }
}

fn parse_bool_filter(raw: &str, param: &str) -> Result<bool, AppError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(AppError::Validation(format!(
            "Custom-field filter {param:?} requires true or false"
        ))),
    }
}

fn parse_custom_field_csv(raw: &str, param: &str) -> Result<Vec<String>, AppError> {
    let values = parse_csv_strings(Some(raw));
    if values.is_empty() {
        return Err(AppError::Validation(format!(
            "Custom-field filter {param:?} requires at least one value"
        )));
    }
    Ok(values)
}

fn parse_custom_field_uuid_csv(raw: &str, param: &str) -> Result<Vec<String>, AppError> {
    Ok(parse_csv_uuids(Some(raw), param)?
        .into_iter()
        .map(|value| value.to_string())
        .collect())
}

fn unsupported_custom_field_operator(param: &str) -> AppError {
    AppError::Validation(format!(
        "Unsupported custom-field filter operator for {param:?}"
    ))
}

/// Emit SQL predicates for validated custom-field filters.
///
/// `jsonb_column` and `id_column` are repository-owned SQL aliases. Custom-field
/// names and values are always bound; no user input is interpolated as SQL.
pub fn push_custom_field_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    state: &mut WhereState,
    jsonb_column: &str,
    id_column: &str,
    filters: &CustomFieldListFilter,
) {
    for predicate in &filters.predicates {
        push_custom_field_predicate(builder, state, jsonb_column, id_column, predicate);
    }
}

fn push_custom_field_predicate(
    builder: &mut QueryBuilder<'_, Postgres>,
    state: &mut WhereState,
    jsonb_column: &str,
    id_column: &str,
    predicate: &CustomFieldPredicate,
) {
    state.and(builder);
    match predicate {
        CustomFieldPredicate::Text { name, value, mode } => {
            builder.push("(");
            builder.push(jsonb_column);
            builder.push(" ->> ");
            builder.push_bind(name.clone());
            match mode {
                TextMatchMode::Contains => {
                    builder.push(") ILIKE ");
                    builder.push_bind(format!("%{value}%"));
                }
                TextMatchMode::Exact => {
                    builder.push(") = ");
                    builder.push_bind(value.clone());
                }
            }
        }
        CustomFieldPredicate::Numeric { name, value, op } => {
            builder.push("((");
            builder.push(jsonb_column);
            builder.push(" ->> ");
            builder.push_bind(name.clone());
            builder.push(")::numeric");
            push_range_sql(builder, *op);
            builder.push_bind(*value);
            builder.push(")");
        }
        CustomFieldPredicate::Temporal {
            name,
            value,
            op,
            kind,
        } => {
            builder.push("((");
            builder.push(jsonb_column);
            builder.push(" ->> ");
            builder.push_bind(name.clone());
            match kind {
                TemporalKind::Date => builder.push(")::date"),
                TemporalKind::Datetime => builder.push(")::timestamptz"),
            };
            push_range_sql(builder, *op);
            builder.push_bind(value.clone());
            match kind {
                TemporalKind::Date => builder.push("::date"),
                TemporalKind::Datetime => builder.push("::timestamptz"),
            };
            builder.push(")");
        }
        CustomFieldPredicate::Boolean { name, value } => {
            builder.push("((");
            builder.push(jsonb_column);
            builder.push(" ->> ");
            builder.push_bind(name.clone());
            builder.push(")::boolean = ");
            builder.push_bind(*value);
            builder.push(")");
        }
        CustomFieldPredicate::ExactOne { name, values } => {
            builder.push("(");
            builder.push(jsonb_column);
            builder.push(" ->> ");
            builder.push_bind(name.clone());
            builder.push(" IN (");
            let mut separated = builder.separated(", ");
            for value in values {
                separated.push_bind(value.clone());
            }
            builder.push("))");
        }
        CustomFieldPredicate::ArrayContainsAny { name, values } => {
            builder.push("(");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    builder.push(" OR ");
                }
                builder.push("(");
                builder.push(jsonb_column);
                builder.push(" -> ");
                builder.push_bind(name.clone());
                builder.push(") ? ");
                builder.push_bind(value.clone());
            }
            builder.push(")");
        }
        CustomFieldPredicate::SecretPresence { name, present } => {
            if !present {
                builder.push("NOT ");
            }
            builder.push(
                "EXISTS (SELECT 1 FROM secret_usage_binding sub \
                 WHERE sub.consumer_type = ",
            );
            builder.push_bind(INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE);
            builder.push(" AND sub.consumer_id = ");
            builder.push(id_column);
            builder.push(" AND sub.field_key = ");
            builder.push_bind(name.clone());
            builder.push(")");
        }
    }
}

fn push_range_sql(builder: &mut QueryBuilder<'_, Postgres>, op: RangeOperator) {
    match op {
        RangeOperator::Exact => builder.push(" = "),
        RangeOperator::Gte => builder.push(" >= "),
        RangeOperator::Lte => builder.push(" <= "),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::custom_fields::domain::{UiEditable, UiVisible};
    use chrono::Utc;

    /// Minimal self-contained allowlist so the shared `SortSpec` tests do not
    /// depend on any per-entity (e.g. inventory) column map.
    fn test_sort_column(key: &str) -> Option<&'static str> {
        match key {
            "name" => Some("t.name"),
            _ => None,
        }
    }

    #[test]
    fn parse_csv_uuids_dedupes_and_skips_blanks() {
        let id = Uuid::now_v7();
        let raw = format!("{id}, ,{id}");
        let parsed = parse_csv_uuids(Some(&raw), "site_id").expect("valid uuids");
        assert_eq!(parsed, vec![id]);
    }

    const TEST_COLUMNS: &[ListColumn] = &[
        ListColumn::sortable("name", "t.name", "name"),
        ListColumn::enum_filter("status", "t.status", "t.status", "status"),
        ListColumn::enum_filter_with_param(
            "active",
            "t.is_active",
            "is_active",
            "t.is_active::text",
            "active",
        ),
        ListColumn::fk_filter("site", "s.name", "t.site_id", "t.site_id", "Sites", "site"),
    ];

    #[test]
    fn list_sort_column_resolves_only_sortable_columns() {
        assert_eq!(list_sort_column(TEST_COLUMNS, "name"), Some("t.name"));
        assert_eq!(list_sort_column(TEST_COLUMNS, "site"), Some("s.name"));
        assert_eq!(list_sort_column(TEST_COLUMNS, "missing"), None);
    }

    #[test]
    fn sort_spec_parse_columns_rejects_unknown_keys() {
        let err = SortSpec::parse_columns(Some("missing"), None, TEST_COLUMNS);
        assert!(matches!(err, Err(AppError::Validation(_))));
    }

    #[test]
    fn enum_filter_with_param_preserves_distinct_column_key_and_query_param() {
        let filter = TEST_COLUMNS
            .iter()
            .find(|column| column.key == "active")
            .and_then(|column| column.filter)
            .expect("filter metadata");
        assert_eq!(filter.param, "is_active");
        assert_eq!(filter.sql, "t.is_active::text");
        assert_eq!(
            list_sort_column(TEST_COLUMNS, "active"),
            Some("t.is_active")
        );
    }

    #[test]
    fn parse_csv_uuids_rejects_garbage() {
        let err = parse_csv_uuids(Some("not-a-uuid"), "site_id");
        assert!(matches!(err, Err(AppError::Validation(_))));
    }

    #[test]
    fn parse_csv_uuids_none_is_empty() {
        assert!(parse_csv_uuids(None, "site_id").unwrap().is_empty());
    }

    #[test]
    fn id_filter_distinguishes_absent_empty_and_deduplicated_values() {
        let id = Uuid::now_v7();
        let values = format!("{id},{id}");

        assert_eq!(parse_id_filter(None).expect("absent filter"), None);
        assert_eq!(
            parse_id_filter(Some("")).expect("empty filter"),
            Some(Vec::new())
        );
        assert_eq!(
            parse_id_filter(Some(&values)).expect("id filter"),
            Some(vec![id])
        );
    }

    #[test]
    fn id_filter_rejects_more_than_one_hundred_identifiers() {
        let raw = (0..=MAX_ID_FILTER_VALUES)
            .map(|_| Uuid::now_v7().to_string())
            .collect::<Vec<_>>()
            .join(",");

        let error = parse_id_filter(Some(&raw)).expect_err("oversized id filter");
        assert!(
            matches!(error, AppError::Validation(message) if message == "ids accepts at most 100 identifiers")
        );
    }

    #[test]
    fn id_filter_predicate_composes_and_keeps_empty_set_false() {
        let id = Uuid::now_v7();

        let mut absent = QueryBuilder::<Postgres>::new("SELECT id FROM records");
        push_id_filter(&mut absent, &mut WhereState::new(), "id", None);
        assert_eq!(absent.sql(), "SELECT id FROM records");

        let mut empty = QueryBuilder::<Postgres>::new("SELECT id FROM records");
        let mut empty_where = WhereState::new();
        push_id_filter(&mut empty, &mut empty_where, "id", Some(&[]));
        assert_eq!(empty.sql(), "SELECT id FROM records WHERE FALSE");

        let mut combined = QueryBuilder::<Postgres>::new("SELECT id FROM records");
        let mut combined_where = WhereState::new();
        combined_where.and(&mut combined);
        combined.push("visible = TRUE");
        push_id_filter(&mut combined, &mut combined_where, "id", Some(&[id]));
        assert_eq!(
            combined.sql(),
            "SELECT id FROM records WHERE visible = TRUE AND id IN ($1)"
        );
    }

    #[test]
    fn parse_csv_strings_trims_dedupes_and_caps() {
        let parsed = parse_csv_strings(Some("active, planned ,active, "));
        assert_eq!(parsed, vec!["active".to_string(), "planned".to_string()]);
    }

    #[test]
    fn search_pattern_trims_and_wraps_non_empty_input() {
        assert_eq!(search_pattern(Some("  core  ")).as_deref(), Some("%core%"));
        assert_eq!(search_pattern(Some("   ")), None);
        assert_eq!(search_pattern(None), None);
    }

    #[test]
    fn sort_spec_absent_key_is_none() {
        let spec = SortSpec::parse(None, Some("desc"), test_sort_column).unwrap();
        assert!(spec.is_none());
    }

    #[test]
    fn sort_spec_rejects_unknown_key() {
        let err = SortSpec::parse(Some("DROP TABLE"), None, test_sort_column);
        assert!(matches!(err, Err(AppError::Validation(_))));
    }

    #[test]
    fn sort_spec_rejects_bad_order() {
        let err = SortSpec::parse(Some("name"), Some("sideways"), test_sort_column);
        assert!(matches!(err, Err(AppError::Validation(_))));
    }

    #[test]
    fn sort_spec_parses_desc() {
        let spec = SortSpec::parse(Some("name"), Some("DESC"), test_sort_column)
            .unwrap()
            .expect("some spec");
        assert_eq!(spec.key, "name");
        assert!(spec.descending);
        assert_eq!(spec.sql_direction(), " DESC");
    }

    fn custom_field_definition(
        name: &str,
        field_type: CustomFieldType,
        filter_logic: FilterLogic,
    ) -> CustomFieldDefinition {
        CustomFieldDefinition {
            id: Uuid::now_v7(),
            name: name.to_string(),
            label: name.to_string(),
            field_type,
            description: String::new(),
            group_name: String::new(),
            required: false,
            enforce_unique: false,
            is_secret: false,
            default_value: None,
            weight: 0,
            validation_minimum: None,
            validation_maximum: None,
            validation_regex: String::new(),
            validation_schema: None,
            choice_set_id: None,
            related_object_type_id: None,
            related_object_filter: None,
            ui_visible: UiVisible::Always,
            ui_editable: UiEditable::Yes,
            is_cloneable: true,
            search_weight: 0,
            filter_logic,
            object_types: vec![Uuid::now_v7()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn secret_definition(name: &str, filter_logic: FilterLogic) -> CustomFieldDefinition {
        CustomFieldDefinition {
            is_secret: true,
            ..custom_field_definition(name, CustomFieldType::Text, filter_logic)
        }
    }

    fn custom_field_params(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn validation_message(result: Result<CustomFieldListFilter, AppError>) -> String {
        match result {
            Ok(_) => panic!("expected validation error"),
            Err(AppError::Validation(message)) => message,
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    }

    #[test]
    fn where_state_opens_once_then_ands() {
        let mut builder = QueryBuilder::new("SELECT 1 FROM entity");
        let mut state = WhereState::new();
        assert!(!state.has_predicates());

        state.and(&mut builder);
        builder.push("a = 1");
        assert!(state.has_predicates());
        state.and(&mut builder);
        builder.push("b = 2");
        state.and(&mut builder);
        builder.push("c = 3");

        assert_eq!(
            builder.sql(),
            "SELECT 1 FROM entity WHERE a = 1 AND b = 2 AND c = 3"
        );
    }

    /// The caller already pushed ` WHERE `, so the FIRST predicate must add no
    /// separator at all. Emitting one here produces `WHERE AND …`, which is a
    /// syntax error — this is the exact regression that collapsing
    /// `after_where` and `after_predicate` into one constructor caused.
    #[test]
    fn where_state_after_where_skips_the_separator_only_once() {
        let mut builder = QueryBuilder::new("SELECT 1 FROM entity WHERE ");
        let mut state = WhereState::after_where();

        state.and(&mut builder);
        builder.push("a = 1");
        state.and(&mut builder);
        builder.push("b = 2");

        assert_eq!(builder.sql(), "SELECT 1 FROM entity WHERE a = 1 AND b = 2");
    }

    /// The caller already pushed a predicate, so EVERY predicate from here needs
    /// its own ` AND ` — including the first.
    #[test]
    fn where_state_after_predicate_separates_from_the_first_call() {
        let mut builder = QueryBuilder::new("SELECT 1 FROM entity WHERE a = 1");
        let mut state = WhereState::after_predicate();

        state.and(&mut builder);
        builder.push("b = 2");
        state.and(&mut builder);
        builder.push("c = 3");

        assert_eq!(
            builder.sql(),
            "SELECT 1 FROM entity WHERE a = 1 AND b = 2 AND c = 3"
        );
    }

    /// A filter list that is entirely empty must leave the query untouched —
    /// emitting a bare `WHERE` would be a syntax error, which is exactly what a
    /// separator decided up front rather than on first use would produce.
    #[test]
    fn where_state_emits_nothing_when_no_predicate_fires() {
        let mut builder = QueryBuilder::new("SELECT 1 FROM entity");
        let mut state = WhereState::new();
        push_uuid_in(&mut builder, &mut state, "entity.id", &[]);
        push_string_in(&mut builder, &mut state, "entity.name", &[]);
        push_search_ilike(&mut builder, &mut state, &["entity.name"], None);

        assert_eq!(builder.sql(), "SELECT 1 FROM entity");
        assert!(!state.has_predicates());
    }

    /// The first predicate that actually fires owns the `WHERE`, no matter how
    /// many skipped filters precede it.
    #[test]
    fn where_state_lets_the_first_firing_filter_open_the_clause() {
        let mut builder = QueryBuilder::new("SELECT 1 FROM entity");
        let mut state = WhereState::new();
        push_uuid_in(&mut builder, &mut state, "entity.id", &[]);
        push_string_in(
            &mut builder,
            &mut state,
            "entity.name",
            &["a".to_owned(), "b".to_owned()],
        );

        assert_eq!(
            builder.sql(),
            "SELECT 1 FROM entity WHERE entity.name IN ($1, $2)"
        );
    }

    fn custom_field_filter_sql(filters: &CustomFieldListFilter) -> String {
        let mut builder = QueryBuilder::new("SELECT 1 FROM entity");
        let mut state = WhereState::new();
        push_custom_field_filters(
            &mut builder,
            &mut state,
            "entity.custom_field_data",
            "entity.id",
            filters,
        );
        builder.sql().to_string()
    }

    #[test]
    fn parse_custom_field_filters_reject_unknown_or_malformed_params() {
        let definitions = [custom_field_definition(
            "asset_tag",
            CustomFieldType::Text,
            FilterLogic::Loose,
        )];
        let unsupported = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("asset_tag", "value")]),
            &definitions,
        ));
        assert!(unsupported.contains("Unsupported query parameter"));

        let stale = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_removed", "value")]),
            &definitions,
        ));
        assert!(stale.contains("Unknown custom-field filter"));
    }

    #[test]
    fn parse_custom_field_text_default_operator_follows_filter_logic() {
        let loose = [custom_field_definition(
            "label",
            CustomFieldType::Text,
            FilterLogic::Loose,
        )];
        let exact = [custom_field_definition(
            "label",
            CustomFieldType::Text,
            FilterLogic::Exact,
        )];

        assert!(
            parse_custom_field_filters(
                &custom_field_params(&[
                    ("cf_label", "needle"),
                    ("cf_label__contains", "needle"),
                    ("cf_label__exact", "needle"),
                ]),
                &loose,
            )
            .is_ok()
        );
        assert!(
            parse_custom_field_filters(
                &custom_field_params(&[
                    ("cf_label", "needle"),
                    ("cf_label__exact", "needle"),
                    ("cf_label__contains", "needle"),
                ]),
                &exact,
            )
            .is_ok()
        );

        let err = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_label__gte", "needle")]),
            &loose,
        ));
        assert!(err.contains("Unsupported custom-field filter operator"));
    }

    #[test]
    fn parse_custom_field_range_and_boolean_values_are_type_checked() {
        let definitions = [
            custom_field_definition("score", CustomFieldType::Integer, FilterLogic::Exact),
            custom_field_definition("starts", CustomFieldType::Date, FilterLogic::Exact),
            custom_field_definition("seen_at", CustomFieldType::Datetime, FilterLogic::Exact),
            custom_field_definition("active", CustomFieldType::Boolean, FilterLogic::Exact),
        ];

        assert!(
            parse_custom_field_filters(
                &custom_field_params(&[
                    ("cf_score__gte", "10"),
                    ("cf_score__lte", "20"),
                    ("cf_starts", "2026-07-06"),
                    ("cf_starts__gte", "2026-01-01"),
                    ("cf_seen_at__lte", "2026-07-06T12:34:56Z"),
                    ("cf_active", "true"),
                ]),
                &definitions,
            )
            .is_ok()
        );

        let invalid_numeric_value = "risky-number-input";
        let numeric = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_score", invalid_numeric_value)]),
            &definitions,
        ));
        assert!(numeric.contains("requires a numeric value"));
        assert!(!numeric.contains(invalid_numeric_value));

        let invalid_date_value = "risky-date-input";
        let date = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_starts", invalid_date_value)]),
            &definitions,
        ));
        assert!(date.contains("requires a YYYY-MM-DD date"));
        assert!(!date.contains(invalid_date_value));

        let invalid_datetime_value = "risky-datetime-input";
        let datetime = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_seen_at", invalid_datetime_value)]),
            &definitions,
        ));
        assert!(datetime.contains("requires an RFC3339 datetime"));
        assert!(!datetime.contains(invalid_datetime_value));

        let boolean = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_active", "yes")]),
            &definitions,
        ));
        assert!(boolean.contains("requires true or false"));
    }

    #[test]
    fn parse_custom_field_datetime_sql_uses_timestamptz_for_mixed_offsets() {
        let definitions = [
            custom_field_definition("starts", CustomFieldType::Date, FilterLogic::Exact),
            custom_field_definition("seen_at", CustomFieldType::Datetime, FilterLogic::Exact),
        ];
        let filters = parse_custom_field_filters(
            &custom_field_params(&[
                ("cf_starts__gte", "2026-07-06"),
                ("cf_seen_at__gte", "2026-07-06T08:00:00-04:00"),
                ("cf_seen_at__lte", "2026-07-06T15:00:00+03:00"),
            ]),
            &definitions,
        )
        .expect("valid temporal custom-field filters");

        let sql = custom_field_filter_sql(&filters);
        assert!(
            sql.contains(")::date >= ") && sql.contains("::date"),
            "date custom-field predicates must use date semantics, got: {sql}",
        );
        let timestamptz_casts = sql.matches("::timestamptz").count();
        assert_eq!(
            timestamptz_casts, 4,
            "two datetime range predicates must cast both sides to timestamptz, got: {sql}",
        );
        assert!(
            sql.contains(")::timestamptz >= ") && sql.contains("::timestamptz"),
            "datetime lower-bound custom-field predicates must use timestamptz semantics, got: {sql}",
        );
        assert!(
            sql.contains(")::timestamptz <= ") && sql.contains("::timestamptz"),
            "datetime upper-bound custom-field predicates must use timestamptz semantics, got: {sql}",
        );
    }

    #[test]
    fn parse_custom_field_choice_and_object_operators_are_supported() {
        let definitions = [
            custom_field_definition("priority", CustomFieldType::Select, FilterLogic::Exact),
            custom_field_definition("tags", CustomFieldType::Multiselect, FilterLogic::Exact),
            custom_field_definition("owner", CustomFieldType::Object, FilterLogic::Exact),
            custom_field_definition("peers", CustomFieldType::Multiobject, FilterLogic::Exact),
        ];

        assert!(
            parse_custom_field_filters(
                &custom_field_params(&[
                    ("cf_priority", "high"),
                    ("cf_priority__exact", "low"),
                    ("cf_tags", "a,b"),
                    ("cf_owner", "11111111-1111-1111-1111-111111111111"),
                    ("cf_peers__exact", "22222222-2222-2222-2222-222222222222"),
                ]),
                &definitions,
            )
            .is_ok()
        );

        let err = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_priority__contains", "high")]),
            &definitions,
        ));
        assert!(err.contains("Unsupported custom-field filter operator"));

        let invalid_ref = "not-a-uuid-reference";
        let object = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_owner", invalid_ref)]),
            &definitions,
        ));
        assert!(object.contains("Invalid cf_owner"));
        assert!(!object.contains(invalid_ref));
    }

    #[test]
    fn parse_custom_field_secret_filters_are_presence_only_and_do_not_echo_values() {
        let definitions = [secret_definition("token", FilterLogic::Loose)];
        let secret_value = "cleartext-secret-value";

        assert!(
            parse_custom_field_filters(
                &custom_field_params(&[("cf_token__present", "true")]),
                &definitions,
            )
            .is_ok()
        );

        let exact = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_token__exact", secret_value)]),
            &definitions,
        ));
        assert!(exact.contains("presence only"));
        assert!(!exact.contains(secret_value));

        let default = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_token", secret_value)]),
            &definitions,
        ));
        assert!(default.contains("presence only"));
        assert!(!default.contains(secret_value));
    }

    #[test]
    fn parse_custom_field_json_and_disabled_filters_are_rejected_before_sql() {
        let definitions = [
            custom_field_definition("payload", CustomFieldType::Json, FilterLogic::Loose),
            custom_field_definition("internal", CustomFieldType::Text, FilterLogic::Disabled),
            secret_definition("secret_internal", FilterLogic::Disabled),
        ];

        let json = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_payload", "{\"path\":true}")]),
            &definitions,
        ));
        assert!(json.contains("not supported for json"));

        let disabled = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_internal", "value")]),
            &definitions,
        ));
        assert!(disabled.contains("is disabled"));

        let disabled_secret = validation_message(parse_custom_field_filters(
            &custom_field_params(&[("cf_secret_internal__present", "true")]),
            &definitions,
        ));
        assert!(disabled_secret.contains("is disabled"));
    }
}
