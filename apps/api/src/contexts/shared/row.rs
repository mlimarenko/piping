//! Typed row reads for sqlx repositories.
//!
//! Row parsers used to read columns as `row.try_get(...).ok()` (or
//! `.ok().flatten()`), which conflates two very different situations:
//!
//! * the column is nullable and the value is absent — a normal state the
//!   domain models as `Option`;
//! * the column is missing from the SELECT, or its type no longer matches —
//!   a programming error that should fail the request loudly.
//!
//! The 2026-07-27 audit measured the damage: of 199 `.ok()` reads, 163 sat on
//! columns that exist in every calling projection, so the `.ok()` could only
//! ever swallow real decode errors. This module is the canonical replacement:
//! nullability is expressed in the requested type (`col::<Option<String>>`),
//! and every failure carries the column name instead of vanishing.

use sqlx::Row;
use sqlx::postgres::PgRow;

use crate::errors::AppError;

/// Read a column by name, failing with the column name in the error.
///
/// Nullability belongs in `T`: request `Option<...>` for nullable columns and
/// the bare type for `NOT NULL` ones. A `NULL` in a non-`Option` read, a
/// missing column, and a type mismatch all surface as errors — none of them
/// are silently `None`.
pub fn col<'r, T>(row: &'r PgRow, name: &str) -> Result<T, AppError>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<T, _>(name)
        .map_err(|error| AppError::Database(format!("Failed to read column `{name}`: {error}")))
}

#[cfg(test)]
mod tests {
    // `col` is exercised end to end by every repository that uses it against a
    // live database; a unit test would need a fabricated PgRow, which sqlx does
    // not offer. The compliance suite guards the call-site pattern instead.
}
