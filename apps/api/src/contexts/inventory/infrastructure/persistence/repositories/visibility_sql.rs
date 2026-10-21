//! SQL rendering of a connection-target [`VisibilityPredicate`].
//!
//! Walks the pure predicate AST compiled in the access-control domain and emits
//! a parenthesized boolean SQL expression over a read-model's facet columns, so
//! the database filters rows exactly as the policy engine would decide. Every
//! value is bound (never interpolated).
//!
//! Three-valued-logic faithfulness (the one way SQL can diverge from the pure
//! `matches`): negation is pushed to the leaves (De Morgan) and a negated
//! equality is emitted as `col IS DISTINCT FROM $`, never `NOT (col = $)`. A
//! positive equality is `col = $`: on a NULL column that is UNKNOWN → the row is
//! excluded, matching the pure evaluator's "absent facet → no match". Because
//! negation never wraps a NULL-producing `col = $`, the SQL row-selection equals
//! the two-valued `matches` for every NULL/value combination. The row id column
//! is never NULL, so id-pattern negation uses ordinary `<>` / `NOT LIKE` / `!~`.

use sqlx::{Postgres, QueryBuilder};

use crate::contexts::access_control::domain::{Facet, VisibilityPredicate};

/// The read-model's SQL column expressions for each facet + the row id. Lets one
/// predicate render against any table alias (`connection_targets.`, `d.`, `v.`,
/// `h.` …). `status` is compared as text; id columns are cast to text so a
/// string-form policy value binds uniformly and matches the canonical lowercase
/// UUID rendering.
#[derive(Debug, Clone, Copy)]
pub struct VisibilityColumns {
    pub id: &'static str,
    pub role_id: &'static str,
    pub site_id: &'static str,
    /// Not a physical row column: a scalar subquery resolving the row's site's
    /// direct region, e.g. `(SELECT s.region_id FROM dcim_site s WHERE s.id =
    /// d.site_id)`. NULL when the row has no site or the site has no region —
    /// the same 3-valued handling as every other facet column.
    pub region_id: &'static str,
    pub tenant_id: &'static str,
    pub platform_id: &'static str,
    pub cluster_id: &'static str,
    pub status: &'static str,
}

impl VisibilityColumns {
    fn column(&self, facet: Facet) -> &'static str {
        match facet {
            Facet::RoleId => self.role_id,
            Facet::SiteId => self.site_id,
            Facet::RegionId => self.region_id,
            Facet::TenantId => self.tenant_id,
            Facet::PlatformId => self.platform_id,
            Facet::ClusterId => self.cluster_id,
            Facet::Status => self.status,
        }
    }
}

/// Append `" AND (<predicate>)"` to a filter builder. `Always` appends nothing
/// (all rows already pass); `Never` appends `" AND (FALSE)"`. Call only after a
/// `WHERE` clause already exists (e.g. the connections read-model opens with
/// `WHERE …`).
pub fn push_visibility_and(
    builder: &mut QueryBuilder<'_, Postgres>,
    predicate: &VisibilityPredicate,
    columns: &VisibilityColumns,
) {
    if predicate.is_always() {
        return;
    }
    builder.push(" AND ");
    push_visibility_expr(builder, predicate, columns);
}

/// Push the predicate as a parenthesized boolean expression `(<predicate>)` with
/// no leading connector — for read-models that manage their own `AND`/`WHERE`
/// via a predicate-joining closure (devices/VMs `list_hydrated`). `Always` still
/// emits `(TRUE)` here (unlike [`push_visibility_and`]); callers that can skip it
/// should check [`VisibilityPredicate::is_always`] first.
pub fn push_visibility_expr(
    builder: &mut QueryBuilder<'_, Postgres>,
    predicate: &VisibilityPredicate,
    columns: &VisibilityColumns,
) {
    builder.push("(");
    render(builder, predicate, columns, false);
    builder.push(")");
}

/// Render `predicate` (optionally negated, De Morgan pushed to leaves) into the
/// builder as a boolean SQL expression.
fn render(
    builder: &mut QueryBuilder<'_, Postgres>,
    predicate: &VisibilityPredicate,
    columns: &VisibilityColumns,
    negated: bool,
) {
    match predicate {
        VisibilityPredicate::Always => render_bool_const(builder, true, negated),
        VisibilityPredicate::Never => render_bool_const(builder, false, negated),
        VisibilityPredicate::Eq { facet, value } => {
            render_eq(builder, columns.column(*facet), value, negated);
        }
        VisibilityPredicate::IdMatch { pattern } => {
            render_id_match(builder, columns.id, pattern, negated);
        }
        VisibilityPredicate::And(items) => {
            // De Morgan: NOT(a AND b) = (NOT a) OR (NOT b).
            render_join(
                builder,
                items,
                columns,
                negated,
                if negated { " OR " } else { " AND " },
            );
        }
        VisibilityPredicate::Or(items) => {
            render_join(
                builder,
                items,
                columns,
                negated,
                if negated { " AND " } else { " OR " },
            );
        }
        VisibilityPredicate::Not(inner) => {
            render(builder, inner, columns, !negated);
        }
    }
}

/// Push a boolean literal, flipping under negation. `base` is the un-negated
/// truth value (`Always` → `true`, `Never` → `false`).
fn render_bool_const(builder: &mut QueryBuilder<'_, Postgres>, base: bool, negated: bool) {
    builder.push(if base ^ negated { "TRUE" } else { "FALSE" });
}

/// Render a facet equality leaf. Compare as text so a string-form policy value
/// binds uniformly against uuid or text columns; a negated equality uses `IS
/// DISTINCT FROM` for three-valued-logic faithfulness on NULL columns.
fn render_eq(
    builder: &mut QueryBuilder<'_, Postgres>,
    column: &'static str,
    value: &str,
    negated: bool,
) {
    builder.push(column);
    builder.push("::text ");
    builder.push(if negated { "IS DISTINCT FROM " } else { "= " });
    builder.push_bind(value.to_string());
}

fn render_join(
    builder: &mut QueryBuilder<'_, Postgres>,
    items: &[VisibilityPredicate],
    columns: &VisibilityColumns,
    negated: bool,
    separator: &str,
) {
    // Empty AND = TRUE, empty OR = FALSE (mirrors `matches` all()/any()); under
    // negation those constants flip.
    if items.is_empty() {
        // AND-join (separator " AND ") of nothing is TRUE; OR-join is FALSE.
        let is_and_join = separator.contains("AND");
        let value = is_and_join ^ negated;
        builder.push(if value { "TRUE" } else { "FALSE" });
        return;
    }
    builder.push("(");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            builder.push(separator);
        }
        render(builder, item, columns, negated);
    }
    builder.push(")");
}

/// Render an id-pattern match faithfully to `matches_resource_pattern` (bare row
/// id): `*` handled upstream (compiled to `Always`); an exact literal is string
/// equality; a glob (`*`/`?`) becomes anchored `LIKE`; a `re:` pattern becomes an
/// anchored POSIX regex. The id column is never NULL, so negation is ordinary.
fn render_id_match(
    builder: &mut QueryBuilder<'_, Postgres>,
    id_column: &'static str,
    pattern: &str,
    negated: bool,
) {
    if let Some(regex) = pattern.strip_prefix("re:") {
        builder.push(id_column);
        builder.push("::text ");
        builder.push(if negated { "!~ " } else { "~ " });
        builder.push_bind(format!("^{regex}$"));
        return;
    }
    if pattern.contains('*') || pattern.contains('?') {
        // Glob → anchored LIKE. Escape LIKE metacharacters first, then map the
        // glob wildcards. `LIKE` is implicitly whole-string anchored, matching
        // the engine's `^…$` glob semantics.
        let like = glob_to_like(pattern);
        builder.push(id_column);
        builder.push("::text ");
        builder.push(if negated { "NOT LIKE " } else { "LIKE " });
        builder.push_bind(like);
        builder.push(" ESCAPE '\\'");
        return;
    }
    builder.push(id_column);
    builder.push("::text ");
    builder.push(if negated { "<> " } else { "= " });
    builder.push_bind(pattern.to_string());
}

/// Translate a `matches_resource_pattern` glob into a `LIKE` pattern: escape the
/// LIKE metacharacters `\`, `%`, `_`, then map glob `*`→`%` and `?`→`_`.
fn glob_to_like(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    for ch in pattern.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            '*' => out.push('%'),
            '?' => out.push('_'),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_translation_escapes_like_metacharacters() {
        assert_eq!(glob_to_like("dev-*"), "dev-%");
        assert_eq!(glob_to_like("host_?"), "host\\__");
        assert_eq!(glob_to_like("a%b*"), "a\\%b%");
    }
}
